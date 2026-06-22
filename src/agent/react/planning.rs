use crate::agent::config::AgentRole;
use crate::agent::react::{ReactAgent, StepType};
use crate::error::{AgentError, ReactError};
use crate::llm::types::Message;
use crate::tasks::executor::{TaskContext, TaskExecuteFn, TaskExecutor, TaskExecutorConfig};
use std::sync::Arc;
use tracing::{debug, info, warn};

impl ReactAgent {
    /// Task planning mode: three-phase execution (Plan → Framework-driven parallel execution → Summarize)
    ///
    /// Unlike traditional LLM-driven execution, this method completely decouples from the LLM during the execution phase,
    /// using `TaskExecutor` to automatically schedule parallel execution based on DAG dependencies.
    ///
    /// # Three-phase workflow
    ///
    /// 1. **Planning phase** (LLM): Agent uses tools to create a sub-task DAG
    /// 2. **Execution phase** (Framework): TaskExecutor schedules ready tasks in parallel, using custom execution functions
    /// 3. **Summary phase** (LLM): All task results are fed to the LLM to generate the final answer
    pub async fn execute_with_planning(&self, task: &str) -> crate::error::Result<String> {
        // ★ Serialize execution — planning mutates context and calls LLM directly
        let _execution_guard = self.execution_mutex.lock().await;

        let agent = self.config.agent_name.clone();

        // Reset message history and task manager to ensure a clean session for each planning run
        self.reset_messages().await;
        self.tools.task_manager.clear();

        info!(agent = %agent, "🎯 Starting task planning mode");
        info!(agent = %agent, task = %task, "📋 User task");

        // When planning capability is not enabled or planning tools are not registered, fall back to normal execution
        if !self.has_planning_tools() {
            warn!(
                agent = %agent,
                "⚠️ Current agent does not have planning enabled or lacks complete planning toolset, automatically falling back to normal execution mode"
            );
            return self.run_direct(task).await;
        }

        // ════════════════════════════════════════════════════════════════════
        // Phase 1: Planning (LLM-driven)
        // ════════════════════════════════════════════════════════════════════
        info!(agent = %agent, phase = "planning", "📐 Phase 1: Creating plan");

        self.plan_tasks(task).await?;

        // If no tasks were created after the planning phase, fall back to normal execution
        if self.tools.task_manager.get_all_tasks().is_empty() {
            warn!(
                agent = %agent,
                "⚠️ No tasks created during planning phase, automatically falling back to normal execution mode"
            );
            return self.run_direct(task).await;
        }

        // ════════════════════════════════════════════════════════════════════
        // Phase 2: Execution (framework-driven, no LLM involvement)
        // ════════════════════════════════════════════════════════════════════
        info!(agent = %agent, phase = "execution", "🚀 Phase 2: Executing tasks in parallel");

        let config = TaskExecutorConfig {
            max_concurrent: 5,
            // max_iterations == 0 means unlimited; use 4 hours as a reasonable default timeout
            default_timeout_secs: if self.config.max_iterations == 0 {
                4 * 3600
            } else {
                self.config.max_iterations as u64 * 30
            },
            enable_hooks: true,
            ..Default::default()
        };

        // Build execution function: use LLM to execute each task
        let execute_fn = self.build_execute_fn();
        let executor =
            TaskExecutor::new(self.tools.task_manager.clone(), config).with_execute_fn(execute_fn);

        // Execute all tasks (event-driven scheduling, auto wake_dependents)
        let results = executor.execute_all().await?;

        for result in &results {
            info!(
                agent = %agent,
                task_id = %result.task_id,
                status = ?result.status,
                duration_ms = result.duration.as_millis(),
                "  ✓ Task [{}] execution completed ({:?})",
                result.task_id,
                result.status
            );
        }

        let (completed, total) = executor.get_progress();
        info!(
            agent = %agent,
            "📊 Execution complete: {}/{} tasks succeeded",
            completed,
            total
        );

        // ════════════════════════════════════════════════════════════════════
        // Phase 3: Summarize results (LLM-driven)
        // ════════════════════════════════════════════════════════════════════
        info!(agent = %agent, phase = "summary", "📝 Phase 3: Generating final answer");

        let task_results_summary = self
            .tools
            .task_manager
            .get_all_tasks()
            .iter()
            .map(|t| {
                let result_str = t.result.as_deref().unwrap_or("No result");
                format!(
                    "  - [{}] {:?}: {} → {}",
                    t.id, t.status, t.description, result_str
                )
            })
            .collect::<Vec<_>>()
            .join("\n");

        self.memory.context.lock().await.push(Message::user(format!(
            "All tasks have been completed. Below are the execution results for each task:\n{}\n\n\
            Based on the above results, use the final_answer tool to provide the final answer.\n\
            **Note**: Do not create new tasks or perform other operations; provide the final answer directly.",
            task_results_summary
        )));

        let max_iters = self.config.max_iterations;
        let unlimited = max_iters == 0;
        let mut iter_count = 0usize;
        loop {
            if !unlimited && iter_count >= max_iters {
                break;
            }
            let steps = self.think().await?;
            if let Some(answer) = self.process_steps(steps).await? {
                info!(agent = %agent, "🏁 Task planning mode execution complete");
                return Ok(answer);
            }
            iter_count += 1;
        }

        warn!(agent = %agent, max = self.config.max_iterations, "Reached maximum iteration count");
        Err(ReactError::Agent(Box::new(
            AgentError::MaxIterationsExceeded(self.config.max_iterations),
        )))
    }

    /// Planning phase: LLM creates task DAG via tool calls
    async fn plan_tasks(&self, task: &str) -> crate::error::Result<()> {
        let agent = self.config.agent_name.clone();

        let planning_prompt = format!(
            "{}\n\n\
            Please first use the think tool to analyze the problem, then use the plan tool to create a plan, and finally use create_task to create each sub-task one by one.\n\n\
            **Important: Task decomposition rules**\n\
            - Break the problem down into the most granular sub-tasks possible; each sub-task should do only one thing\n\
            - Do not set dependencies for mutually independent sub-tasks; let them execute in parallel\n\
            - Only set dependencies when one task truly needs another task's result\n\
            - Build a wide and shallow DAG (directed acyclic graph) rather than a linear chain\n\
            - **Planning is only complete after all sub-tasks are created; do not stop after creating only a portion**",
            task
        );

        self.memory
            .context
            .lock()
            .await
            .push(Message::user(planning_prompt));

        // For planning phase, use max_iterations as safety limit; 0 = use reasonable default (50 rounds)
        let planning_max_rounds = if self.config.max_iterations == 0 {
            50
        } else {
            self.config.max_iterations
        };
        let mut has_created_tasks = false;

        for round in 0..planning_max_rounds {
            debug!(agent = %agent, round = round + 1, "📐 Planning round");
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
                        info!(agent = %agent, "🏁 Final answer generated during planning phase");
                        return Ok(());
                    }
                    self.memory.context.lock().await.push(Message::tool_result(
                        tool_call_id,
                        function_name,
                        result,
                    ));
                }
            }

            if created_task_this_round {
                has_created_tasks = true;
            }

            if has_created_tasks && !created_task_this_round {
                let task_count = self.tools.task_manager.get_all_tasks().len();
                info!(
                    agent = %agent,
                    task_count = task_count,
                    "📐 Planning complete, {} sub-tasks created",
                    task_count
                );
                break;
            }
        }

        Ok(())
    }

    /// Build the task execution function
    ///
    /// Default execution logic: send the task description to the LLM to obtain execution results.
    /// For orchestrator role, prioritize dispatching tasks to SubAgents for execution.
    fn build_execute_fn(&self) -> TaskExecuteFn {
        let agent_name = self.config.agent_name.clone();
        let model = self.config.model_name.clone();
        let client = self.client.clone(); // Reuse Agent's HTTP client
        let cache_user_id = self.config.cache_user_id.clone();
        let is_orchestrator = self.config.role == AgentRole::Orchestrator;
        let subagent_names: Vec<String> = if is_orchestrator {
            // Use blocking read from the registry since we're in sync context
            #[cfg(feature = "subagent")]
            {
                self.tools
                    .subagent_registry
                    .agents_map()
                    .try_read()
                    .map(|agents| agents.keys().cloned().collect())
                    .unwrap_or_default()
            }
            #[cfg(not(feature = "subagent"))]
            Vec::new()
        } else {
            Vec::new()
        };

        Arc::new(move |ctx: TaskContext| {
            let agent_name = agent_name.clone();
            let model = model.clone();
            let client = client.clone();
            let cache_user_id = cache_user_id.clone();
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

                // Use LLM to execute task (reusing Agent's HTTP client)
                let messages = vec![
                    crate::llm::types::Message::system(
                        "You are a task execution assistant. Please complete the following task and provide the result directly.".to_string(),
                    ),
                    crate::llm::types::Message::user(user_prompt),
                ];

                let response = crate::llm::chat(
                    client,
                    &model,
                    &messages,
                    Some(0.3),
                    Some(4096u32),
                    Some(false),
                    None,
                    None,
                    None,
                    cache_user_id.clone(),
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

    /// Task planning mode with custom execution function
    pub async fn execute_with_planning_fn(
        &self,
        task: &str,
        execute_fn: TaskExecuteFn,
    ) -> crate::error::Result<String> {
        // ★ Serialize execution — planning mutates context and calls LLM directly
        let _execution_guard = self.execution_mutex.lock().await;

        let _agent = self.config.agent_name.clone();

        self.reset_messages().await;
        self.tools.task_manager.clear();

        if !self.has_planning_tools() {
            return self.run_direct(task).await;
        }

        // Phase 1: Plan
        self.plan_tasks(task).await?;

        if self.tools.task_manager.get_all_tasks().is_empty() {
            return self.run_direct(task).await;
        }

        // Phase 2: Execute with custom execute_fn
        let config = TaskExecutorConfig::default();
        let executor =
            TaskExecutor::new(self.tools.task_manager.clone(), config).with_execute_fn(execute_fn);

        let _results = executor.execute_all().await?;

        // Phase 3: Summarize
        let task_results_summary = self
            .tools
            .task_manager
            .get_all_tasks()
            .iter()
            .map(|t| {
                let result_str = t.result.as_deref().unwrap_or("No result");
                format!(
                    "  - [{}] {:?}: {} → {}",
                    t.id, t.status, t.description, result_str
                )
            })
            .collect::<Vec<_>>()
            .join("\n");

        self.memory.context.lock().await.push(Message::user(format!(
            "All tasks have been completed. Below are the execution results for each task:\n{}\n\n\
            Based on the above results, use the final_answer tool to provide the final answer.",
            task_results_summary
        )));

        let max_iters = self.config.max_iterations;
        let unlimited = max_iters == 0;
        let mut iter_count = 0usize;
        loop {
            if !unlimited && iter_count >= max_iters {
                return Err(ReactError::Agent(Box::new(
                    AgentError::MaxIterationsExceeded(max_iters),
                )));
            }

            let steps = self.think().await?;
            if let Some(answer) = self.process_steps(steps).await? {
                return Ok(answer);
            }

            iter_count += 1;
        }
    }
}
