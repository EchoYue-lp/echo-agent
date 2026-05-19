//! Plan-and-Execute engine
//!
//! An execution strategy at the same level as ReAct, explicitly separating **planning** and **execution** into two independent phases.
//!
//! # Unified execution model
//!
//! `Plan` is converted to a `Task` DAG via `to_task_dag()`, using `TaskManager` for dependency scheduling.
//! Supports incremental replanning: when a step fails, only the affected downstream subgraph is replanned.
//!
//! # Example
//!
//! ```rust,no_run
//! use echo_agent::agent::plan_execute::{PlanExecuteAgent, LlmPlanner, ReactExecutor};
//! use echo_agent::prelude::*;
//!
//! # #[tokio::main]
//! # async fn main() -> echo_agent::error::Result<()> {
//! let planner = LlmPlanner::new("qwen3-max");
//! let executor_agent = ReactAgentBuilder::new()
//!     .model("qwen3-max")
//!     .name("executor")
//!     .system_prompt("You are a task execution assistant")
//!     .enable_tools()
//!     .build()?;
//! let executor = ReactExecutor::new(executor_agent);
//!
//! let mut agent = PlanExecuteAgent::new("plan_agent", planner, executor);
//! let result = agent.execute("Help me analyze and optimize this code's performance").await?;
//! # Ok(())
//! # }
//! ```

mod planner;
mod store;

pub use echo_core::agent::{
    Executor, IssueSeverity, Plan, PlanOutput, PlanStep, PlanStepOutput, PlanStore, PlanSummary,
    PlanValidationIssue, Planner, ReactExecutor, SimpleExecutor, StaticPlanner, StepResult,
    StepStatus, plan_output_schema,
};
pub use planner::{LlmPlanner, PlannerOutputMode};
pub use store::{SqlitePlanStore, generate_plan_slug};

// ── PlanExt: to_task_dag() extension trait ──────────────────────────────────

/// Extension trait adding `to_task_dag()` to `Plan`.
///
/// This method depends on `crate::tasks::Task` (from echo-orchestration) and is
/// therefore defined in the facade rather than in echo-core alongside `Plan`.
trait PlanExt {
    fn to_task_dag(&self) -> Vec<crate::tasks::Task>;
}

impl PlanExt for Plan {
    fn to_task_dag(&self) -> Vec<crate::tasks::Task> {
        use tracing::warn;
        let now = echo_core::utils::time::now_secs();
        self.steps
            .iter()
            .enumerate()
            .map(|(i, step)| {
                let deps: Vec<String> = step
                    .dependencies
                    .iter()
                    .filter_map(|dep| {
                        let (step_idx, was_fuzzy) = self.resolve_dependency_with_fuzzy(dep);
                        if was_fuzzy {
                            warn!(
                                step = i,
                                dependency = %dep,
                                "Fuzzy dependency resolution used for step {}, dep '{}'. Prefer exact 'step_N' references.",
                                i, dep
                            );
                        }
                        step_idx.map(|idx| format!("plan_step_{}", idx))
                    })
                    .collect();

                crate::tasks::Task {
                    id: format!("plan_step_{}", i),
                    description: step.description.clone(),
                    subject: step.description.clone(),
                    status: crate::tasks::TaskStatus::Pending,
                    dependencies: deps,
                    priority: 5,
                    result: None,
                    reasoning: step.expected_output.clone(),
                    assigned_agent: None,
                    tags: vec![],
                    parent_id: self.id.clone(),
                    created_at: now,
                    updated_at: now,
                    timeout_secs: 0,
                    max_retries: 0,
                    retry_count: 0,
                }
            })
            .collect()
    }
}

use crate::agent::{Agent, AgentEvent};
use crate::error::Result;
use crate::tasks::executor::{TaskExecuteFn, TaskExecutor, TaskExecutorConfig};
use crate::tasks::{TaskManager, TaskStatus};
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Execution mode for plan steps
#[derive(Clone, Default)]
pub enum ExecutionMode {
    /// Sequential execution via the `Executor` trait (default).
    /// Steps are executed one-by-one in dependency order.
    /// Supports incremental replanning on failure.
    #[default]
    Sequential,
    /// Parallel execution via `TaskExecutor` with a custom execution function.
    /// Independent steps run concurrently, bounded by `max_concurrent`.
    Parallel {
        /// Task execution function for executing plan steps in parallel
        execute_fn: TaskExecuteFn,
        /// Maximum concurrent task limit
        max_concurrent: usize,
    },
}

/// Plan-and-Execute Agent
///
/// Planner generates plan → `to_task_dag()` converts to Task DAG → `TaskManager` schedules → Executor executes step by step.
/// Supports incremental replanning (only replans the failed downstream subgraph).
pub struct PlanExecuteAgent {
    name: String,
    planner: RwLock<Box<dyn Planner>>,
    executor: RwLock<Box<dyn Executor>>,
    max_replans: usize,
    enable_replan: bool,
    execution_mode: ExecutionMode,
}

impl PlanExecuteAgent {
    /// Create a Plan-and-Execute Agent instance
    ///
    /// # Parameters
    /// * `name` - Agent name
    /// * `planner` - Planner instance implementing the `Planner` trait
    /// * `executor` - Executor instance implementing the `Executor` trait
    ///
    /// # Returns
    /// A new `PlanExecuteAgent` instance
    ///
    /// # Default configuration
    /// - Maximum replan count: 3
    /// - Replanning enabled: yes
    /// - Execution mode: Sequential (`ExecutionMode::Sequential`)
    ///
    /// # Example
    /// ```rust,no_run
    /// use echo_agent::agent::plan_execute::{PlanExecuteAgent, SimpleExecutor, StaticPlanner};
    /// use echo_agent::agent::{Agent, ReactAgentBuilder};
    /// use echo_agent::agent::plan_execute::Executor;
    ///
    /// # #[tokio::main]
    /// # async fn main() -> echo_agent::error::Result<()> {
    /// let planner = StaticPlanner::new(vec!["Analyze the problem", "Provide conclusion"]);
    /// let inner = ReactAgentBuilder::new()
    ///     .model("qwen3-max")
    ///     .name("executor")
    ///     .system_prompt("Execute a single step")
    ///     .build()?;
    /// let executor = SimpleExecutor::new(inner);
    /// let mut agent = PlanExecuteAgent::new("plan_agent", planner, executor);
    /// # Ok(())
    /// # }
    /// ```
    pub fn new(
        name: impl Into<String>,
        planner: impl Planner + 'static,
        executor: impl Executor + 'static,
    ) -> Self {
        Self {
            name: name.into(),
            planner: RwLock::new(Box::new(planner)),
            executor: RwLock::new(Box::new(executor)),
            max_replans: 3,
            enable_replan: true,
            execution_mode: ExecutionMode::default(),
        }
    }

    /// Set maximum replan count
    ///
    /// # Parameters
    /// * `n` - Maximum replan count
    ///
    /// # Returns
    /// Returns `Self` for chained calls
    ///
    /// # Description
    /// When a step execution fails, the Agent attempts to replan affected downstream steps.
    /// This method limits the maximum number of replans to avoid infinite loops.
    ///
    /// # Default
    /// Default maximum replan count is 3.
    pub fn max_replans(mut self, n: usize) -> Self {
        self.max_replans = n;
        self
    }

    /// Disable replanning
    ///
    /// # Returns
    /// Returns `Self` for chained calls
    ///
    /// # Description
    /// When replanning is disabled, the Agent will not attempt to replan on step failure,
    /// and will return an error directly. Suitable for scenarios requiring high execution stability.
    ///
    /// # Default
    /// Replanning is enabled by default.
    pub fn disable_replan(mut self) -> Self {
        self.enable_replan = false;
        self
    }

    /// Enable parallel execution with a custom execution function.
    ///
    /// When set, independent steps are executed concurrently via `TaskExecutor`,
    /// using the DAG dependency structure from the plan.
    /// When not set, steps are executed sequentially via the `Executor` trait.
    pub fn with_execute_fn(mut self, f: TaskExecuteFn) -> Self {
        self.execution_mode = ExecutionMode::Parallel {
            execute_fn: f,
            max_concurrent: 5,
        };
        self
    }

    /// Set max concurrent tasks for parallel execution (default: 5)
    pub fn max_concurrent(mut self, n: usize) -> Self {
        if let ExecutionMode::Parallel { execute_fn, .. } = self.execution_mode {
            self.execution_mode = ExecutionMode::Parallel {
                execute_fn,
                max_concurrent: n,
            };
        }
        self
    }

    /// Set the execution mode directly
    pub fn with_execution_mode(mut self, mode: ExecutionMode) -> Self {
        self.execution_mode = mode;
        self
    }

    /// Core execution loop
    async fn run_plan_execute(&self, task: &str) -> Result<String> {
        let agent = self.name.clone();

        // ── Phase 1: Planning ──────────────────────────────────────
        info!(agent = %agent, "📐 Plan-and-Execute: Phase 1 - Generate plan");
        let mut plan = self.planner.write().await.plan(task).await?;

        // Validate and auto-fix plan
        let issues = plan.validate();
        for issue in &issues {
            if matches!(issue.severity, IssueSeverity::Error) {
                warn!(agent = %agent, issue = %issue.message, "⚠️ Plan validation found issues");
            }
        }
        plan.auto_fix();

        // Re-validate after auto_fix to ensure fixes are effective
        let post_fix_issues = plan.validate();
        let remaining_errors: Vec<_> = post_fix_issues
            .iter()
            .filter(|i| matches!(i.severity, IssueSeverity::Error))
            .collect();
        if !remaining_errors.is_empty() {
            warn!(
                agent = %agent,
                count = remaining_errors.len(),
                "⚠️ {} unresolved issues remain after auto-fix",
                remaining_errors.len()
            );
        }

        info!(
            agent = %agent,
            steps = plan.steps.len(),
            "📋 Plan generation complete, {} total steps",
            plan.steps.len()
        );

        for (i, step) in plan.steps.iter().enumerate() {
            debug!(
                agent = %agent,
                step = i + 1,
                description = %step.description,
                "  Step {}: {}",
                i + 1,
                step.description
            );
        }

        // ── Phase 2: Execute (Plan → Task DAG → dependency scheduling) ────────
        match &self.execution_mode {
            ExecutionMode::Parallel { .. } => {
                self.run_parallel_execution(task, &agent, &plan).await
            }
            ExecutionMode::Sequential => self.run_sequential_execution(task, &agent, plan).await,
        }
    }

    /// Parallel execution path using TaskExecutor
    async fn run_parallel_execution(&self, task: &str, agent: &str, plan: &Plan) -> Result<String> {
        info!(agent = %agent, "🚀 Plan-and-Execute: Phase 2 - Execute plan in parallel");

        let (execute_fn, max_concurrent) = match &self.execution_mode {
            ExecutionMode::Parallel {
                execute_fn,
                max_concurrent,
            } => (execute_fn.clone(), *max_concurrent),
            ExecutionMode::Sequential => {
                unreachable!("run_parallel_execution called in Sequential mode")
            }
        };

        let task_manager = Arc::new(TaskManager::new());
        let dag_tasks = plan.to_task_dag();
        for t in dag_tasks {
            task_manager.add_task(t);
        }

        let config = TaskExecutorConfig {
            max_concurrent,
            ..Default::default()
        };

        let executor = TaskExecutor::new(task_manager.clone(), config).with_execute_fn(execute_fn);

        let _results = executor.execute_all().await?;

        let (completed, total) = executor.get_progress();
        info!(agent = %agent, "📊 Parallel execution complete: {}/{} tasks succeeded", completed, total);

        // ── Phase 3: Summarize results ─────────────────────────────────
        info!(agent = %agent, "📝 Plan-and-Execute: Phase 3 - Summarize results");

        let all_tasks = task_manager.get_all_tasks();
        let results: Vec<StepResult> = all_tasks
            .iter()
            .enumerate()
            .map(|(i, t)| StepResult {
                step_index: i,
                description: t.description.clone(),
                output: t.result.clone().unwrap_or_default(),
                success: t.status == TaskStatus::Completed,
            })
            .collect();

        let summary = self.summarize_results(task, &results).await?;
        info!(agent = %agent, "🏁 Plan-and-Execute execution complete");
        Ok(summary)
    }

    /// Sequential execution path via Executor trait (backward compatible)
    async fn run_sequential_execution(
        &self,
        task: &str,
        agent: &str,
        mut plan: Plan,
    ) -> Result<String> {
        let agent = agent.to_string();

        // ── Phase 2: Execute (Plan → Task DAG → dependency scheduling) ────────
        info!(agent = %agent, "🚀 Plan-and-Execute: Phase 2 - Execute plan sequentially");

        let task_manager = Arc::new(TaskManager::new());
        let dag_tasks = plan.to_task_dag();
        for t in dag_tasks {
            task_manager.add_task(t);
        }

        let mut replan_count = 0;
        let mut all_results: Vec<StepResult> = Vec::new();

        loop {
            let ready = task_manager.get_ready_tasks();
            if ready.is_empty() {
                if task_manager.is_all_completed() {
                    break;
                }
                warn!(agent = %agent, "⚠️ No executable tasks and not all completed, possible dependency deadlock");
                break;
            }

            for task_item in ready {
                let task_id = task_item.id.clone();
                let step_idx = task_id
                    .trim_start_matches("plan_step_")
                    .parse::<usize>()
                    .unwrap_or(0);

                // Update to InProgress
                let _ = task_manager.update_task(&task_id, TaskStatus::InProgress);

                // Build context from previous results
                let context = self.build_step_context_from_results(&all_results);

                info!(
                    agent = %agent,
                    step = step_idx + 1,
                    "⚡ Execute step {}: {}",
                    step_idx + 1,
                    task_item.description
                );

                match self
                    .executor
                    .write()
                    .await
                    .execute_step(&task_item.description, &context)
                    .await
                {
                    Ok(output) => {
                        info!(agent = %agent, step = step_idx + 1, "✅ Step {} executed successfully", step_idx + 1);
                        let _ = task_manager.update_task(&task_id, TaskStatus::Completed);
                        task_manager.set_task_result(&task_id, output.clone());
                        all_results.push(StepResult {
                            step_index: step_idx,
                            description: task_item.description.clone(),
                            output,
                            success: true,
                        });
                    }
                    Err(e) => {
                        warn!(agent = %agent, step = step_idx + 1, error = %e, "❌ Step {} execution failed", step_idx + 1);
                        let _ =
                            task_manager.update_task(&task_id, TaskStatus::Failed(e.to_string()));
                        task_manager.set_task_result(&task_id, format!("Execution failed: {}", e));
                        all_results.push(StepResult {
                            step_index: step_idx,
                            description: task_item.description.clone(),
                            output: format!("Execution failed: {}", e),
                            success: false,
                        });

                        // Incremental replanning: only replan the failed downstream subgraph
                        if self.enable_replan && replan_count < self.max_replans {
                            replan_count += 1;
                            info!(
                                agent = %agent,
                                replan = replan_count,
                                max = self.max_replans,
                                "🔄 Triggering incremental replan ({}/{})",
                                replan_count,
                                self.max_replans
                            );

                            // Use downstream_steps to identify affected downstream subgraph
                            let affected: Vec<String> = plan
                                .downstream_steps_recursive(step_idx)
                                .iter()
                                .map(|&idx| format!("plan_step_{}", idx))
                                .collect();
                            let affected_descs: Vec<String> = plan
                                .downstream_steps_recursive(step_idx)
                                .iter()
                                .map(|&idx| {
                                    plan.steps
                                        .get(idx)
                                        .map(|s| s.description.clone())
                                        .unwrap_or_default()
                                })
                                .collect();

                            info!(
                                agent = %agent,
                                affected = affected.len(),
                                "🔄 Affected downstream steps: {:?}",
                                affected_descs
                            );

                            let completed_steps: Vec<String> = all_results
                                .iter()
                                .filter(|r| r.success)
                                .map(|r| format!("  - {}: {}", r.description, r.output))
                                .collect();

                            let replan_prompt = format!(
                                "Original task: {}\n\nCompleted steps:\n{}\n\nFailed step: {}\nError: {}\n\n\
                                Affected downstream steps:\n{}\n\n\
                                Please re-plan the affected steps based on the above information.\n\
                                Note: only plan the affected steps, do not repeat completed steps.",
                                task,
                                completed_steps.join("\n"),
                                task_item.description,
                                e,
                                affected_descs
                                    .iter()
                                    .map(|d| format!("  - {}", d))
                                    .collect::<Vec<_>>()
                                    .join("\n")
                            );

                            match self.planner.write().await.plan(&replan_prompt).await {
                                Ok(new_plan) => {
                                    // Update the plan variable with the new plan for consistency
                                    plan = new_plan;

                                    // Only remove affected downstream tasks (not all non-terminal tasks)
                                    let to_remove: Vec<String> = task_manager
                                        .get_all_tasks()
                                        .iter()
                                        .filter(|t| affected.contains(&t.id))
                                        .map(|t| t.id.clone())
                                        .collect();
                                    for id in &to_remove {
                                        task_manager.delete_task(id);
                                    }

                                    // Regenerate task DAG with new plan to ensure dependency indices match
                                    let new_dag = plan.to_task_dag();
                                    let new_count = new_dag.len();
                                    for t in new_dag {
                                        task_manager.add_task(t);
                                    }
                                    info!(
                                        agent = %agent,
                                        removed = to_remove.len(),
                                        new_tasks = new_count,
                                        "📋 Incremental replan complete, removed {} affected tasks, added {} new tasks",
                                        to_remove.len(),
                                        new_count
                                    );
                                    break; // Restart execution loop
                                }
                                Err(replan_err) => {
                                    warn!(agent = %agent, error = %replan_err, "⚠️ Replan failed, continuing with remaining steps");
                                }
                            }
                        }
                    }
                }
            }

            // Check if all are complete
            if task_manager.is_all_completed() {
                break;
            }
        }

        // ── Phase 3: Summarize results ─────────────────────────────────
        info!(agent = %agent, "📝 Plan-and-Execute: Phase 3 - Summarize results");

        let (completed, total) = task_manager.get_progress();
        info!(agent = %agent, "📊 Execution complete: {}/{} tasks succeeded", completed, total);

        let summary = self.summarize_results(task, &all_results).await?;
        info!(agent = %agent, "🏁 Plan-and-Execute execution complete");

        Ok(summary)
    }

    /// Build step context from existing results
    fn build_step_context_from_results(&self, results: &[StepResult]) -> String {
        if results.is_empty() {
            return String::new();
        }

        let mut parts = vec!["Completed step results:".to_string()];
        for r in results {
            parts.push(format!(
                "  - Step {}: {} → {}",
                r.step_index + 1,
                r.description,
                if r.output.len() > 200 {
                    let end = r
                        .output
                        .char_indices()
                        .take_while(|(idx, _)| *idx < 200)
                        .last()
                        .map(|(idx, c)| idx + c.len_utf8())
                        .unwrap_or(0);
                    format!("{}...", &r.output[..end])
                } else {
                    r.output.clone()
                }
            ));
        }
        parts.join("\n")
    }

    /// Summarize all step results
    async fn summarize_results(&self, task: &str, results: &[StepResult]) -> Result<String> {
        let results_text: Vec<String> = results
            .iter()
            .map(|r| {
                let status = if r.success { "Success" } else { "Failed" };
                format!(
                    "Step {} [{}]: {} → {}",
                    r.step_index + 1,
                    status,
                    r.description,
                    r.output
                )
            })
            .collect();

        let summary_prompt = format!(
            "Original task: {}\n\nExecution results:\n{}\n\nPlease provide the final summary answer based on the above execution results.",
            task,
            results_text.join("\n")
        );

        self.executor
            .write()
            .await
            .execute_step(&summary_prompt, "")
            .await
    }
}

// ── impl Agent ───────────────────────────────────────────────────────────────

impl Agent for PlanExecuteAgent {
    fn name(&self) -> &str {
        &self.name
    }

    fn model_name(&self) -> &str {
        "plan-and-execute"
    }

    fn system_prompt(&self) -> &str {
        ""
    }

    fn execute<'a>(&'a self, task: &'a str) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move { self.run_plan_execute(task).await })
    }

    fn execute_stream<'a>(
        &'a self,
        task: &'a str,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        Box::pin(async move {
            let task_owned = task.to_string();
            let stream = async_stream::try_stream! {
                let agent = self.name.clone();

                // ── Phase 1: Planning ──
                info!(agent = %agent, "📐 Plan-and-Execute (stream): Generate plan");
                let mut plan = self.planner.write().await.plan(&task_owned).await?;

                // Validate and auto-fix
                let issues = plan.validate();
                for issue in &issues {
                    if matches!(issue.severity, IssueSeverity::Error) {
                        warn!(agent = %agent, issue = %issue.message, "⚠️ Plan validation found issues");
                    }
                }
                plan.auto_fix();

                // Re-validate after auto_fix to ensure fixes are effective
                let post_fix_issues = plan.validate();
                let remaining_errors: Vec<_> = post_fix_issues
                    .iter()
                    .filter(|i| matches!(i.severity, IssueSeverity::Error))
                    .collect();
                if !remaining_errors.is_empty() {
                    warn!(
                        agent = %agent,
                        count = remaining_errors.len(),
                        "⚠️ {} unresolved issues remain after auto-fix",
                        remaining_errors.len()
                    );
                }

                yield AgentEvent::PlanGenerated {
                    steps: plan.steps.iter().map(|s| s.description.clone()).collect(),
                };

                // ── Phase 2: Execute via TaskManager ──
                let task_manager = Arc::new(TaskManager::new());
                let dag_tasks = plan.to_task_dag();
                for t in dag_tasks {
                    task_manager.add_task(t);
                }

                let mut replan_count = 0;
                let mut all_results: Vec<StepResult> = Vec::new();

                loop {
                    let ready = task_manager.get_ready_tasks();
                    if ready.is_empty() {
                        if task_manager.is_all_completed() {
                            break;
                        }
                        break;
                    }

                    for task_item in ready {
                        let task_id = task_item.id.clone();
                        let step_idx = task_id
                            .trim_start_matches("plan_step_")
                            .parse::<usize>()
                            .unwrap_or(0);

                        yield AgentEvent::StepStart {
                            step_index: step_idx,
                            description: task_item.description.clone(),
                        };

                        let _ = task_manager.update_task(&task_id, TaskStatus::InProgress);

                        let context = self.build_step_context_from_results(&all_results);

                        match self.executor.write().await.execute_step(&task_item.description, &context).await {
                            Ok(output) => {
                                let _ = task_manager.update_task(&task_id, TaskStatus::Completed);
                                task_manager.set_task_result(&task_id, output.clone());
                                all_results.push(StepResult {
                                    step_index: step_idx,
                                    description: task_item.description.clone(),
                                    output,
                                    success: true,
                                });
                                yield AgentEvent::StepEnd { step_index: step_idx, success: true };
                            }
                            Err(e) => {
                                let _ = task_manager.update_task(&task_id, TaskStatus::Failed(e.to_string()));
                                task_manager.set_task_result(&task_id, format!("Execution failed: {}", e));
                                all_results.push(StepResult {
                                    step_index: step_idx,
                                    description: task_item.description.clone(),
                                    output: format!("Execution failed: {}", e),
                                    success: false,
                                });
                                yield AgentEvent::StepEnd { step_index: step_idx, success: false };

                                if self.enable_replan && replan_count < self.max_replans {
                                    replan_count += 1;

                                    // Use downstream_steps for incremental replanning
                                    let affected: Vec<String> = plan
                                        .downstream_steps_recursive(step_idx)
                                        .iter()
                                        .map(|&idx| format!("plan_step_{}", idx))
                                        .collect();

                                    let completed_steps: Vec<String> = all_results
                                        .iter()
                                        .filter(|r| r.success)
                                        .map(|r| format!("  - {}: {}", r.description, r.output))
                                        .collect();
                                    let replan_prompt = format!(
                                        "Original task: {}\n\nCompleted steps:\n{}\n\nFailed step: {}\nError: {}\n\nPlease re-plan the affected steps.",
                                        task_owned, completed_steps.join("\n"), task_item.description, e
                                    );

                                    if let Ok(new_plan) = self.planner.write().await.plan(&replan_prompt).await {
                                        // Only remove affected downstream tasks
                                        let to_remove: Vec<String> = task_manager
                                            .get_all_tasks()
                                            .iter()
                                            .filter(|t| affected.contains(&t.id))
                                            .map(|t| t.id.clone())
                                            .collect();
                                        for id in &to_remove {
                                            task_manager.delete_task(id);
                                        }
                                        let new_dag = new_plan.to_task_dag();
                                        plan = new_plan;
                                        for t in new_dag {
                                            task_manager.add_task(t);
                                        }
                                        yield AgentEvent::PlanGenerated {
                                            steps: task_manager
                                                .get_all_tasks()
                                                .iter()
                                                .filter(|t| t.status == TaskStatus::Pending)
                                                .map(|t| t.description.clone())
                                                .collect(),
                                        };
                                        break;
                                    }
                                }
                            }
                        }
                    }

                    if task_manager.is_all_completed() {
                        break;
                    }
                }

                // ── Phase 3: Summarize ──
                let summary = self.summarize_results(&task_owned, &all_results).await?;
                yield AgentEvent::FinalAnswer(summary);
            };
            Ok(Box::pin(stream) as BoxStream<'_, Result<AgentEvent>>)
        })
    }

    fn reset(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async {})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plan_step_status() {
        let step = PlanStep::new("test step");
        assert_eq!(step.status, StepStatus::Pending);
        assert_eq!(step.description, "test step");
    }

    #[test]
    fn test_plan() {
        let plan = Plan::new(vec![
            PlanStep::new("step 1"),
            PlanStep::new("step 2"),
            PlanStep::new("step 3"),
        ]);
        assert_eq!(plan.steps.len(), 3);
    }

    #[test]
    fn test_plan_to_task_dag_no_deps() {
        let plan = Plan::new(vec![PlanStep::new("step A"), PlanStep::new("step B")]);
        let tasks = plan.to_task_dag();
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].id, "plan_step_0");
        assert_eq!(tasks[1].id, "plan_step_1");
        assert!(tasks[0].dependencies.is_empty());
        assert!(tasks[1].dependencies.is_empty());
        assert_eq!(tasks[0].status, TaskStatus::Pending);
    }

    #[test]
    fn test_plan_to_task_dag_with_deps() {
        let plan = Plan::new(vec![
            PlanStep::new("step A"),
            PlanStep::new("step B").with_dependencies(vec!["step_0".to_string()]),
            PlanStep::new("step C")
                .with_dependencies(vec!["step_0".to_string(), "step_1".to_string()]),
        ]);
        let tasks = plan.to_task_dag();
        assert_eq!(tasks.len(), 3);
        assert_eq!(tasks[1].dependencies, vec!["plan_step_0"]);
        assert_eq!(tasks[2].dependencies, vec!["plan_step_0", "plan_step_1"]);
    }

    #[test]
    fn test_plan_to_task_dag_dag_properties() {
        let plan = Plan::new(vec![
            PlanStep::new("A"),
            PlanStep::new("B").with_dependencies(vec!["step_0".to_string()]),
        ]);
        let tasks = plan.to_task_dag();
        let manager = TaskManager::new();
        for t in tasks {
            manager.add_task(t);
        }
        assert!(!manager.has_circular_dependencies());

        let ready = manager.get_ready_tasks();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, "plan_step_0");

        let _ = manager.update_task("plan_step_0", TaskStatus::InProgress);
        let _ = manager.update_task("plan_step_0", TaskStatus::Completed);
        let ready = manager.get_ready_tasks();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, "plan_step_1");
    }

    #[tokio::test]
    async fn test_static_planner_to_dag() {
        let planner = StaticPlanner::new(vec!["A", "B", "C"]);
        let plan = planner.plan("test").await.unwrap();
        let tasks = plan.to_task_dag();
        assert_eq!(tasks.len(), 3);

        let manager = TaskManager::new();
        for t in tasks {
            manager.add_task(t);
        }
        assert_eq!(manager.get_ready_tasks().len(), 3);
    }

    #[tokio::test]
    async fn test_plan_execute_agent_sequential() {
        use crate::testing::MockAgent;

        let planner = StaticPlanner::new(vec!["step 1", "step 2"]);
        let mock_agent = MockAgent::new("mock").with_responses(vec![
            "step 1 done",
            "step 2 done",
            "final summary",
        ]);
        let executor = SimpleExecutor::new(mock_agent);
        let agent = PlanExecuteAgent::new("test_agent", planner, executor).disable_replan();

        let result = agent.execute("test task").await.unwrap();
        assert!(!result.is_empty());
    }

    #[tokio::test]
    async fn test_plan_execute_agent_parallel() {
        use crate::testing::MockAgent;

        let planner = StaticPlanner::new(vec!["step A", "step B", "step C"]);
        let mock_agent = MockAgent::new("mock").with_response("summary");
        let executor = SimpleExecutor::new(mock_agent);

        let execute_fn: TaskExecuteFn =
            Arc::new(|ctx| Box::pin(async move { Ok(format!("done: {}", ctx.task_id)) }));

        let agent = PlanExecuteAgent::new("test_agent", planner, executor)
            .with_execute_fn(execute_fn)
            .max_concurrent(3);

        let result = agent.execute("test parallel").await.unwrap();
        assert!(!result.is_empty());
    }

    #[tokio::test]
    async fn test_plan_execute_agent_with_deps() {
        use crate::testing::MockAgent;

        let planner = StaticPlanner::new(vec!["first", "second", "third"]);
        let mock_agent = MockAgent::new("mock").with_response("summary");
        let executor = SimpleExecutor::new(mock_agent);

        let execute_fn: TaskExecuteFn = Arc::new(|ctx| {
            Box::pin(async move {
                Ok(format!(
                    "result[{}] upstream={}",
                    ctx.task_id,
                    ctx.upstream_results.len()
                ))
            })
        });

        let agent =
            PlanExecuteAgent::new("test_agent", planner, executor).with_execute_fn(execute_fn);

        let result = agent.execute("test deps").await.unwrap();
        assert!(!result.is_empty());
    }
}
