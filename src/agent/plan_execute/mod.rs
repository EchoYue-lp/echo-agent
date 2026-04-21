//! Plan-and-Execute 引擎
//!
//! 与 ReAct 平级的执行策略，将 **规划** 和 **执行** 显式分离为两个独立阶段。
//!
//! # 统一执行模型
//!
//! `Plan` 通过 `to_task_dag()` 转换为 `Task` DAG，使用 `TaskManager` 进行依赖调度。
//! 支持增量重规划：当步骤失败时，仅重新规划受影响的下游子图。
//!
//! # 示例
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
//!     .system_prompt("你是一个任务执行助手")
//!     .enable_tools()
//!     .build()?;
//! let executor = ReactExecutor::new(executor_agent);
//!
//! let mut agent = PlanExecuteAgent::new("plan_agent", planner, executor);
//! let result = agent.execute("帮我分析并优化这段代码的性能").await?;
//! # Ok(())
//! # }
//! ```

mod executor;
mod planner;
mod store;
mod types;

pub use executor::{Executor, ReactExecutor, SimpleExecutor};
pub use planner::{LlmPlanner, Planner, PlannerOutputMode, StaticPlanner};
pub use store::{PlanStore, PlanSummary, SqlitePlanStore, generate_plan_slug};
pub use types::{
    IssueSeverity, Plan, PlanOutput, PlanStep, PlanStepOutput, PlanValidationIssue, StepResult,
    StepStatus, plan_output_schema,
};

use crate::agent::{Agent, AgentEvent};
use crate::error::Result;
use crate::tasks::executor::{TaskExecuteFn, TaskExecutor, TaskExecutorConfig};
use crate::tasks::{TaskManager, TaskStatus};
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use std::sync::Arc;
use tracing::{debug, info, warn};

#[allow(unused_imports)]
use futures::StreamExt;

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
        /// 任务执行函数，用于并行执行计划步骤
        execute_fn: TaskExecuteFn,
        /// 最大并发任务数量限制
        max_concurrent: usize,
    },
}

/// Plan-and-Execute Agent
///
/// Planner 生成计划 → `to_task_dag()` 转为 Task DAG → `TaskManager` 调度 → Executor 逐步执行。
/// 支持增量重规划（失败时仅重新规划下游子图）。
pub struct PlanExecuteAgent<P: Planner, E: Executor> {
    name: String,
    planner: P,
    executor: E,
    max_replans: usize,
    enable_replan: bool,
    execution_mode: ExecutionMode,
}

impl<P: Planner, E: Executor> PlanExecuteAgent<P, E> {
    /// 创建 Plan-and-Execute Agent 实例
    ///
    /// # 参数
    /// * `name` - Agent 名称
    /// * `planner` - 规划器实例，实现 `Planner` trait
    /// * `executor` - 执行器实例，实现 `Executor` trait
    ///
    /// # 返回值
    /// 新的 `PlanExecuteAgent` 实例
    ///
    /// # 默认配置
    /// - 最大重规划次数：3
    /// - 启用重规划：是
    /// - 执行模式：顺序执行（`ExecutionMode::Sequential`）
    ///
    /// # 示例
    /// ```rust
    /// use echo_agent::agent::plan_execute::{PlanExecuteAgent, SimpleExecutor, StaticPlanner};
    /// use echo_agent::agent::Agent;
    /// use echo_agent::testing::MockAgent;
    ///
    /// # #[tokio::main]
    /// # async fn main() -> echo_agent::error::Result<()> {
    /// let planner = StaticPlanner::new(vec!["分析问题", "给出结论"]);
    /// let executor = SimpleExecutor::new(
    ///     MockAgent::new("executor")
    ///         .with_response("已完成步骤 1")
    ///         .with_response("已完成步骤 2"),
    /// );
    /// let mut agent = PlanExecuteAgent::new("plan_agent", planner, executor);
    /// let result = agent.execute("帮我分析这个问题").await?;
    /// assert!(!result.trim().is_empty());
    /// # Ok(())
    /// # }
    /// ```
    pub fn new(name: impl Into<String>, planner: P, executor: E) -> Self {
        Self {
            name: name.into(),
            planner,
            executor,
            max_replans: 3,
            enable_replan: true,
            execution_mode: ExecutionMode::default(),
        }
    }

    /// 设置最大重规划次数
    ///
    /// # 参数
    /// * `n` - 最大重规划次数
    ///
    /// # 返回值
    /// 返回 `Self` 以便链式调用
    ///
    /// # 说明
    /// 当步骤执行失败时，Agent 会尝试重新规划受影响的下游步骤。
    /// 该方法限制重规划的最大次数，避免无限循环。
    ///
    /// # 默认值
    /// 默认最大重规划次数为 3。
    pub fn max_replans(mut self, n: usize) -> Self {
        self.max_replans = n;
        self
    }

    /// 禁用重规划功能
    ///
    /// # 返回值
    /// 返回 `Self` 以便链式调用
    ///
    /// # 说明
    /// 禁用重规划后，当步骤执行失败时，Agent 不会尝试重新规划，
    /// 而是直接返回错误。适用于对执行稳定性要求较高的场景。
    ///
    /// # 默认值
    /// 默认启用重规划功能。
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

    /// 核心执行循环
    async fn run_plan_execute(&mut self, task: &str) -> Result<String> {
        let agent = self.name.clone();

        // ── 阶段 1: 规划 ──────────────────────────────────────
        info!(agent = %agent, "📐 Plan-and-Execute: 阶段1 - 生成计划");
        let mut plan = self.planner.plan(task).await?;

        // 验证并自动修复计划
        let issues = plan.validate();
        for issue in &issues {
            if matches!(issue.severity, IssueSeverity::Error) {
                warn!(agent = %agent, issue = %issue.message, "⚠️ 计划验证发现问题");
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
                "⚠️ 自动修复后仍有 {} 个未解决的问题",
                remaining_errors.len()
            );
        }

        info!(
            agent = %agent,
            steps = plan.steps.len(),
            "📋 计划生成完成，共 {} 个步骤",
            plan.steps.len()
        );

        for (i, step) in plan.steps.iter().enumerate() {
            debug!(
                agent = %agent,
                step = i + 1,
                description = %step.description,
                "  步骤 {}: {}",
                i + 1,
                step.description
            );
        }

        // ── 阶段 2: 执行（Plan → Task DAG → 依赖调度） ────────
        match &self.execution_mode {
            ExecutionMode::Parallel { .. } => {
                self.run_parallel_execution(task, &agent, &plan).await
            }
            ExecutionMode::Sequential => self.run_sequential_execution(task, &agent, plan).await,
        }
    }

    /// Parallel execution path using TaskExecutor
    async fn run_parallel_execution(
        &mut self,
        task: &str,
        agent: &str,
        plan: &Plan,
    ) -> Result<String> {
        info!(agent = %agent, "🚀 Plan-and-Execute: 阶段2 - 并行执行计划");

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
        info!(agent = %agent, "📊 并行执行完成: {}/{} 任务成功", completed, total);

        // ── 阶段 3: 汇总结果 ─────────────────────────────────
        info!(agent = %agent, "📝 Plan-and-Execute: 阶段3 - 汇总结果");

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
        info!(agent = %agent, "🏁 Plan-and-Execute 执行完毕");
        Ok(summary)
    }

    /// Sequential execution path via Executor trait (backward compatible)
    async fn run_sequential_execution(
        &mut self,
        task: &str,
        agent: &str,
        mut plan: Plan,
    ) -> Result<String> {
        let agent = agent.to_string();

        // ── 阶段 2: 执行（Plan → Task DAG → 依赖调度） ────────
        info!(agent = %agent, "🚀 Plan-and-Execute: 阶段2 - 顺序执行计划");

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
                warn!(agent = %agent, "⚠️ 没有可执行任务且未全部完成，可能存在依赖死锁");
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
                    "⚡ 执行步骤 {}: {}",
                    step_idx + 1,
                    task_item.description
                );

                match self
                    .executor
                    .execute_step(&task_item.description, &context)
                    .await
                {
                    Ok(output) => {
                        info!(agent = %agent, step = step_idx + 1, "✅ 步骤 {} 执行成功", step_idx + 1);
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
                        warn!(agent = %agent, step = step_idx + 1, error = %e, "❌ 步骤 {} 执行失败", step_idx + 1);
                        let _ =
                            task_manager.update_task(&task_id, TaskStatus::Failed(e.to_string()));
                        task_manager.set_task_result(&task_id, format!("执行失败: {}", e));
                        all_results.push(StepResult {
                            step_index: step_idx,
                            description: task_item.description.clone(),
                            output: format!("执行失败: {}", e),
                            success: false,
                        });

                        // 增量重规划：只重新规划失败的下游子图
                        if self.enable_replan && replan_count < self.max_replans {
                            replan_count += 1;
                            info!(
                                agent = %agent,
                                replan = replan_count,
                                max = self.max_replans,
                                "🔄 触发增量重规划 ({}/{})",
                                replan_count,
                                self.max_replans
                            );

                            // 使用 downstream_steps 识别受影响的下游子图
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
                                "🔄 影响的下游步骤: {:?}",
                                affected_descs
                            );

                            let completed_steps: Vec<String> = all_results
                                .iter()
                                .filter(|r| r.success)
                                .map(|r| format!("  - {}: {}", r.description, r.output))
                                .collect();

                            let replan_prompt = format!(
                                "原始任务: {}\n\n已完成的步骤:\n{}\n\n失败的步骤: {}\n错误: {}\n\n\
                                受影响的下游步骤:\n{}\n\n\
                                请根据以上情况重新制定受影响步骤的计划。\n\
                                注意：只规划受影响的步骤，不要重复已完成的步骤。",
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

                            match self.planner.plan(&replan_prompt).await {
                                Ok(new_plan) => {
                                    // Update the plan variable with the new plan for consistency
                                    plan = new_plan;

                                    // 只移除受影响的下游任务（而非所有非终止任务）
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
                                        "📋 增量重规划完成，移除 {} 个受影响任务，新增 {} 个任务",
                                        to_remove.len(),
                                        new_count
                                    );
                                    break; // 重新开始执行循环
                                }
                                Err(replan_err) => {
                                    warn!(agent = %agent, error = %replan_err, "⚠️ 重新规划失败，继续执行剩余步骤");
                                }
                            }
                        }
                    }
                }
            }

            // 检查是否全部完成
            if task_manager.is_all_completed() {
                break;
            }
        }

        // ── 阶段 3: 汇总结果 ─────────────────────────────────
        info!(agent = %agent, "📝 Plan-and-Execute: 阶段3 - 汇总结果");

        let (completed, total) = task_manager.get_progress();
        info!(agent = %agent, "📊 执行完成: {}/{} 任务成功", completed, total);

        let summary = self.summarize_results(task, &all_results).await?;
        info!(agent = %agent, "🏁 Plan-and-Execute 执行完毕");

        Ok(summary)
    }

    /// 构建步骤上下文（从已有结果）
    fn build_step_context_from_results(&self, results: &[StepResult]) -> String {
        if results.is_empty() {
            return String::new();
        }

        let mut parts = vec!["已完成步骤的结果：".to_string()];
        for r in results {
            parts.push(format!(
                "  - 步骤 {}: {} → {}",
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

    /// 汇总所有步骤结果
    async fn summarize_results(&mut self, task: &str, results: &[StepResult]) -> Result<String> {
        let results_text: Vec<String> = results
            .iter()
            .map(|r| {
                let status = if r.success { "成功" } else { "失败" };
                format!(
                    "步骤 {} [{}]: {} → {}",
                    r.step_index + 1,
                    status,
                    r.description,
                    r.output
                )
            })
            .collect();

        let summary_prompt = format!(
            "原始任务: {}\n\n执行结果:\n{}\n\n请根据以上执行结果，给出最终的总结答案。",
            task,
            results_text.join("\n")
        );

        self.executor.execute_step(&summary_prompt, "").await
    }
}

// ── impl Agent ───────────────────────────────────────────────────────────────

impl<P: Planner + Send + Sync, E: Executor + Send + Sync> Agent for PlanExecuteAgent<P, E> {
    fn name(&self) -> &str {
        &self.name
    }

    fn model_name(&self) -> &str {
        "plan-and-execute"
    }

    fn system_prompt(&self) -> &str {
        ""
    }

    fn execute<'a>(&'a mut self, task: &'a str) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move { self.run_plan_execute(task).await })
    }

    fn execute_stream<'a>(
        &'a mut self,
        task: &'a str,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        Box::pin(async move {
            let task_owned = task.to_string();
            let stream = async_stream::try_stream! {
                let agent = self.name.clone();

                // ── Phase 1: Planning ──
                info!(agent = %agent, "📐 Plan-and-Execute (stream): 生成计划");
                let mut plan = self.planner.plan(&task_owned).await?;

                // Validate and auto-fix
                let issues = plan.validate();
                for issue in &issues {
                    if matches!(issue.severity, IssueSeverity::Error) {
                        warn!(agent = %agent, issue = %issue.message, "⚠️ 计划验证发现问题");
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
                        "⚠️ 自动修复后仍有 {} 个未解决的问题",
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

                        match self.executor.execute_step(&task_item.description, &context).await {
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
                                task_manager.set_task_result(&task_id, format!("执行失败: {}", e));
                                all_results.push(StepResult {
                                    step_index: step_idx,
                                    description: task_item.description.clone(),
                                    output: format!("执行失败: {}", e),
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
                                        "原始任务: {}\n\n已完成的步骤:\n{}\n\n失败的步骤: {}\n错误: {}\n\n请重新制定受影响步骤的计划。",
                                        task_owned, completed_steps.join("\n"), task_item.description, e
                                    );

                                    if let Ok(new_plan) = self.planner.plan(&replan_prompt).await {
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

    fn reset(&mut self) {}
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
        let planner = crate::agent::plan_execute::planner::StaticPlanner::new(vec!["A", "B", "C"]);
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
        let mut agent = PlanExecuteAgent::new("test_agent", planner, executor).disable_replan();

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

        let mut agent = PlanExecuteAgent::new("test_agent", planner, executor)
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

        let mut agent =
            PlanExecuteAgent::new("test_agent", planner, executor).with_execute_fn(execute_fn);

        let result = agent.execute("test deps").await.unwrap();
        assert!(!result.is_empty());
    }
}
