use crate::agent::config::AgentRole;
use crate::agent::react::{ReactAgent, StepType};
use crate::error::{AgentError, ReactError};
use crate::llm::types::Message;
use crate::tasks::executor::{TaskContext, TaskExecuteFn, TaskExecutor, TaskExecutorConfig};
use std::sync::Arc;
use tracing::{debug, info, warn};

impl ReactAgent {
    /// 任务规划模式：三阶段执行（规划 → 框架驱动并行执行 → 汇总）
    ///
    /// 与传统 LLM 驱动的执行不同，本方法在执行阶段完全脱离 LLM，
    /// 由 `TaskExecutor` 基于 DAG 依赖自动调度并行执行。
    ///
    /// # 三阶段流程
    ///
    /// 1. **规划阶段**（LLM）：Agent 使用工具创建子任务 DAG
    /// 2. **执行阶段**（框架）：TaskExecutor 并行调度就绪任务，使用自定义执行函数
    /// 3. **汇总阶段**（LLM）：将所有任务结果交给 LLM 生成最终答案
    pub async fn execute_with_planning(&mut self, task: &str) -> crate::error::Result<String> {
        let agent = self.config.agent_name.clone();

        // 重置消息历史和任务管理器，确保每次规划都是干净的 session
        self.reset_messages();
        self.task_manager.clear();

        info!(agent = %agent, "🎯 启动任务规划模式");
        info!(agent = %agent, task = %task, "📋 用户任务");

        // 未启用规划能力或未注册规划工具时，降级到普通执行
        if !self.has_planning_tools() {
            warn!(
                agent = %agent,
                "⚠️ 当前 agent 未启用规划能力或未注册完整规划工具集，自动降级为普通执行模式"
            );
            return self.run_direct(task).await;
        }

        // ════════════════════════════════════════════════════════════════════
        // 阶段 1：规划（LLM 驱动）
        // ════════════════════════════════════════════════════════════════════
        info!(agent = %agent, phase = "planning", "📐 阶段1: 制定计划");

        self.plan_tasks(task).await?;

        // 规划阶段结束后仍无任务，回退普通执行
        if self.task_manager.get_all_tasks().is_empty() {
            warn!(
                agent = %agent,
                "⚠️ 规划阶段未创建任务，自动降级为普通执行模式"
            );
            return self.run_direct(task).await;
        }

        // ════════════════════════════════════════════════════════════════════
        // 阶段 2：执行（框架驱动，不经过 LLM）
        // ════════════════════════════════════════════════════════════════════
        info!(agent = %agent, phase = "execution", "🚀 阶段2: 并行执行任务");

        let config = TaskExecutorConfig {
            max_concurrent: 5,
            default_timeout_secs: self.config.max_iterations as u64 * 30,
            enable_hooks: true,
            ..Default::default()
        };

        // 构建执行函数：使用 LLM 执行每个任务
        let execute_fn = self.build_execute_fn();
        let executor =
            TaskExecutor::new(self.task_manager.clone(), config).with_execute_fn(execute_fn);

        // 执行全部任务（事件驱动调度，自动 wake_dependents）
        let results = executor.execute_all().await?;

        for result in &results {
            info!(
                agent = %agent,
                task_id = %result.task_id,
                status = ?result.status,
                duration_ms = result.duration.as_millis(),
                "  ✓ 任务 [{}] 执行完成 ({:?})",
                result.task_id,
                result.status
            );
        }

        let (completed, total) = executor.get_progress();
        info!(
            agent = %agent,
            "📊 执行完成: {}/{} 任务成功",
            completed,
            total
        );

        // ════════════════════════════════════════════════════════════════════
        // 阶段 3：汇总结果（LLM 驱动）
        // ════════════════════════════════════════════════════════════════════
        info!(agent = %agent, phase = "summary", "📝 阶段3: 生成最终答案");

        let task_results_summary = self
            .task_manager
            .get_all_tasks()
            .iter()
            .map(|t| {
                let result_str = t.result.as_deref().unwrap_or("无结果");
                format!(
                    "  - [{}] {:?}: {} → {}",
                    t.id, t.status, t.description, result_str
                )
            })
            .collect::<Vec<_>>()
            .join("\n");

        self.context.push(Message::user(format!(
            "所有任务已完成。以下是各任务的执行结果：\n{}\n\n\
            请根据以上结果，使用 final_answer 工具给出最终答案。\n\
            **注意**：不要再创建新任务或执行其他操作，直接给出最终答案。",
            task_results_summary
        )));

        for _ in 0..self.config.max_iterations {
            let steps = self.think().await?;
            if let Some(answer) = self.process_steps(steps).await? {
                info!(agent = %agent, "🏁 任务规划模式执行完毕");
                return Ok(answer);
            }
        }

        warn!(agent = %agent, max = self.config.max_iterations, "达到最大迭代次数");
        Err(ReactError::Agent(AgentError::MaxIterationsExceeded(
            self.config.max_iterations,
        )))
    }

    /// 规划阶段：LLM 通过工具调用创建任务 DAG
    async fn plan_tasks(&mut self, task: &str) -> crate::error::Result<()> {
        let agent = self.config.agent_name.clone();

        let planning_prompt = format!(
            "{}\n\n\
            请先使用 think 工具分析问题，然后用 plan 工具制定计划，最后用 create_task 逐个创建所有子任务。\n\n\
            **重要：任务拆分规则**\n\
            - 将问题拆分为尽可能细粒度的子任务，每个子任务只做一件事\n\
            - 互相独立的子任务不要设置依赖关系，让它们可以并行执行\n\
            - 只有当一个任务真正需要另一个任务的结果时，才设置 dependencies\n\
            - 尽量构建宽而浅的 DAG（有向无环图），而非线性链\n\
            - **必须创建全部子任务后规划才算完成，不要只创建部分就停止**",
            task
        );

        self.context.push(Message::user(planning_prompt));

        let planning_max_rounds = self.config.max_iterations;
        let mut has_created_tasks = false;

        for round in 0..planning_max_rounds {
            debug!(agent = %agent, round = round + 1, "📐 规划轮次");
            let steps = self.think().await?;
            let mut created_task_this_round = false;

            for step in steps {
                if let StepType::Call {
                    tool_call_id,
                    function_name,
                    arguments,
                } = step
                {
                    if function_name == "create_task" {
                        created_task_this_round = true;
                    }
                    let result = self.execute_tool(&function_name, &arguments).await?;
                    if function_name == "final_answer" {
                        info!(agent = %agent, "🏁 规划阶段已生成最终答案");
                        return Ok(());
                    }
                    self.context
                        .push(Message::tool_result(tool_call_id, function_name, result));
                }
            }

            if created_task_this_round {
                has_created_tasks = true;
            }

            if has_created_tasks && !created_task_this_round {
                let task_count = self.task_manager.get_all_tasks().len();
                info!(
                    agent = %agent,
                    task_count = task_count,
                    "📐 规划完成，共创建 {} 个子任务",
                    task_count
                );
                break;
            }
        }

        Ok(())
    }

    /// 构建任务执行函数
    ///
    /// 默认执行逻辑：将任务描述发送给 LLM 获取执行结果。
    /// 对于编排者角色，优先将任务分派给 SubAgent 执行。
    fn build_execute_fn(&self) -> TaskExecuteFn {
        let agent_name = self.config.agent_name.clone();
        let model = self.config.model_name.clone();
        let is_orchestrator = self.config.role == AgentRole::Orchestrator;
        let subagent_names: Vec<String> = if is_orchestrator {
            // Use blocking read from the registry since we're in sync context
            self.subagent_registry
                .agents_map()
                .try_read()
                .map(|agents| agents.keys().cloned().collect())
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        Arc::new(move |ctx: TaskContext| {
            let agent_name = agent_name.clone();
            let model = model.clone();
            let _subagent_names = subagent_names.clone();

            Box::pin(async move {
                debug!(
                    agent = %agent_name,
                    task_id = %ctx.task_id,
                    "Executing task via LLM"
                );

                // Build prompt with upstream context
                let upstream_context = ctx.format_upstream_context();
                let user_prompt = if upstream_context.is_empty() {
                    ctx.description.clone()
                } else {
                    format!("{}\n\n{}", upstream_context, ctx.description)
                };

                // 默认使用 LLM 执行任务
                let client = reqwest::Client::new();
                let messages = vec![
                    crate::llm::types::Message::system(
                        "你是任务执行助手。请完成以下任务，直接给出结果。".to_string(),
                    ),
                    crate::llm::types::Message::user(user_prompt),
                ];

                let response = crate::llm::chat(
                    Arc::new(client),
                    &model,
                    &messages,
                    Some(0.3),
                    Some(4096u32),
                    Some(false),
                    None,
                    None,
                    None,
                )
                .await
                .map_err(|e| ReactError::Other(format!("LLM execution failed: {}", e)))?;

                let content = response
                    .choices
                    .first()
                    .and_then(|c| c.message.content.as_text())
                    .unwrap_or_else(|| format!("Task [{}] completed (no output)", ctx.task_id));

                Ok(content)
            })
        })
    }

    /// 使用自定义执行函数的任务规划模式
    pub async fn execute_with_planning_fn(
        &mut self,
        task: &str,
        execute_fn: TaskExecuteFn,
    ) -> crate::error::Result<String> {
        let _agent = self.config.agent_name.clone();

        self.reset_messages();
        self.task_manager.clear();

        if !self.has_planning_tools() {
            return self.run_direct(task).await;
        }

        // Phase 1: Plan
        self.plan_tasks(task).await?;

        if self.task_manager.get_all_tasks().is_empty() {
            return self.run_direct(task).await;
        }

        // Phase 2: Execute with custom execute_fn
        let config = TaskExecutorConfig::default();
        let executor =
            TaskExecutor::new(self.task_manager.clone(), config).with_execute_fn(execute_fn);

        let _results = executor.execute_all().await?;

        // Phase 3: Summarize
        let task_results_summary = self
            .task_manager
            .get_all_tasks()
            .iter()
            .map(|t| {
                let result_str = t.result.as_deref().unwrap_or("无结果");
                format!(
                    "  - [{}] {:?}: {} → {}",
                    t.id, t.status, t.description, result_str
                )
            })
            .collect::<Vec<_>>()
            .join("\n");

        self.context.push(Message::user(format!(
            "所有任务已完成。以下是各任务的执行结果：\n{}\n\n\
            请根据以上结果，使用 final_answer 工具给出最终答案。",
            task_results_summary
        )));

        for _ in 0..self.config.max_iterations {
            let steps = self.think().await?;
            if let Some(answer) = self.process_steps(steps).await? {
                return Ok(answer);
            }
        }

        Err(ReactError::Agent(AgentError::MaxIterationsExceeded(
            self.config.max_iterations,
        )))
    }
}
