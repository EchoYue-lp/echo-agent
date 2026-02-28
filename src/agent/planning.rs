use crate::agent::config::AgentRole;
use crate::agent::react_agent::{ReactAgent, StepType};
use crate::error::{AgentError, ReactError};
use crate::llm::types::Message;
use crate::tasks::{TaskManager, TaskStatus};
use tracing::{debug, info, warn};

impl ReactAgent {
    pub async fn execute_with_planning(&mut self, task: &str) -> crate::error::Result<String> {
        let agent = self.config.agent_name.clone();

        // 重置消息历史和任务管理器，确保每次规划都是干净的 session
        self.reset_messages();
        *self
            .task_manager
            .write()
            .map_err(|e| ReactError::Other(format!("task_manager lock poisoned: {}", e)))? =
            TaskManager::default();

        info!(agent = %agent, "🎯 启动任务规划模式");
        info!(agent = %agent, task = %task, "📋 用户任务");

        // 未启用规划能力或未注册规划工具时，降级到普通执行，避免卡在规划流程
        if !self.has_planning_tools() {
            warn!(
                agent = %agent,
                "⚠️ 当前 agent 未启用规划能力或未注册完整规划工具集，自动降级为普通执行模式"
            );
            return self.run_direct(task).await;
        }

        // ── 第一阶段：让 Agent 制定计划 ──────────────────────
        info!(agent = %agent, phase = "planning", "📐 阶段1: 制定计划");

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

        // LLM 停止调用 create_task 时视为规划阶段结束
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
                        return Ok(result);
                    }
                    self.context
                        .push(Message::tool_result(tool_call_id, function_name, result));
                }
            }

            if created_task_this_round {
                has_created_tasks = true;
            }

            if has_created_tasks && !created_task_this_round {
                let manager = self
                    .task_manager
                    .read()
                    .map_err(|e| ReactError::Other(format!("Lock poisoned: {}", e)))?;
                let task_count = manager.get_all_tasks().len();
                info!(
                    agent = %agent,
                    task_count = task_count,
                    "📐 规划完成，共创建 {} 个子任务",
                    task_count
                );
                break;
            }
        }

        // 规划阶段结束后仍无任务，说明模型未按规划协议工作，回退普通执行
        let planned_task_count = self
            .task_manager
            .read()
            .map_err(|e| ReactError::Other(format!("Lock poisoned: {}", e)))?
            .get_all_tasks()
            .len();
        if planned_task_count == 0 {
            warn!(
                agent = %agent,
                "⚠️ 规划阶段未创建任务，自动降级为普通执行模式"
            );
            return self.run_direct(task).await;
        }

        // ── 第二阶段：并行执行就绪任务 ──────────────────────
        info!(agent = %agent, phase = "execution", "🚀 阶段2: 执行任务");

        loop {
            let ready_tasks = {
                let manager = self
                    .task_manager
                    .read()
                    .map_err(|e| ReactError::Other(format!("Lock poisoned: {}", e)))?;

                if manager.is_all_completed() {
                    info!(agent = %agent, "✅ 所有子任务已完成");
                    break;
                }

                manager
                    .get_ready_tasks()
                    .into_iter()
                    .cloned()
                    .collect::<Vec<_>>()
            };

            if ready_tasks.is_empty() {
                warn!(agent = %agent, "⏳ 没有可执行的任务，等待依赖完成");
                self.context.push(Message::user(
                    "没有可执行的任务。请检查任务状态并继续。".to_string(),
                ));
                self.think().await?;
                continue;
            }

            let task_list: Vec<String> = ready_tasks
                .iter()
                .map(|t| format!("  - [{}]: {}", t.id, t.description))
                .collect();

            let batch_ids: Vec<String> = ready_tasks.iter().map(|t| t.id.clone()).collect();

            info!(
                agent = %agent,
                tasks = ?batch_ids,
                "⚡ 开始执行 {} 个就绪任务",
                ready_tasks.len()
            );

            // 编排模式下提示 LLM 将任务分派给 SubAgent，而非自己执行
            let dispatch_hint = if self.config.role == AgentRole::Orchestrator
                && self.config.enable_subagent
            {
                let subagent_names: Vec<String> = self
                    .subagents
                    .read()
                    .map(|agents| agents.keys().cloned().collect())
                    .unwrap_or_default();
                if !subagent_names.is_empty() {
                    format!(
                        "\n\n**重要**：你是编排者，请使用 agent_tool 将任务分派给合适的 SubAgent 执行，\
                        不要自己直接计算或猜测结果。\n\
                        可用的 SubAgent: {}\n\
                        完成后使用 update_task 标记完成，并将 SubAgent 返回的结果写入 result 字段。",
                        subagent_names.join(", ")
                    )
                } else {
                    "\n完成后使用 update_task 标记完成。".to_string()
                }
            } else {
                "\n完成后使用 update_task 标记完成。".to_string()
            };

            if ready_tasks.len() == 1 {
                self.context.push(Message::user(format!(
                    "请执行任务 [{}]: {}{}",
                    ready_tasks[0].id, ready_tasks[0].description, dispatch_hint
                )));
            } else {
                self.context.push(Message::user(format!(
                    "以下 {} 个任务的依赖已全部满足，请**同时**执行所有任务：\n{}{}",
                    ready_tasks.len(),
                    task_list.join("\n"),
                    dispatch_hint
                )));
            }

            for iteration in 0..self.config.max_iterations {
                debug!(
                    agent = %agent,
                    tasks = ?batch_ids,
                    iteration = iteration + 1,
                    "任务批次迭代"
                );
                let steps = self.think().await?;
                if let Some(answer) = self.process_steps(steps).await? {
                    return Ok(answer);
                }

                let manager = self
                    .task_manager
                    .read()
                    .map_err(|e| ReactError::Other(format!("Lock poisoned: {}", e)))?;
                let batch_done = batch_ids.iter().all(|id| {
                    manager
                        .tasks
                        .get(id)
                        .map(|t| {
                            matches!(
                                t.status,
                                TaskStatus::Completed
                                    | TaskStatus::Cancelled
                                    | TaskStatus::Failed(_)
                            )
                        })
                        .unwrap_or(false)
                });
                if batch_done {
                    info!(agent = %agent, tasks = ?batch_ids, "✅ 任务批次执行完成");
                    break;
                }
            }
        }

        // ── 第三阶段：总结结果 ──────────────────────────────
        info!(agent = %agent, phase = "summary", "📝 阶段3: 生成最终答案");

        let task_results_summary = {
            let manager = self
                .task_manager
                .read()
                .map_err(|e| ReactError::Other(format!("Lock poisoned: {}", e)))?;
            manager
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
                .join("\n")
        };

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
}
