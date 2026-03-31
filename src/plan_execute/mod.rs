//! Plan-and-Execute 引擎
//!
//! 与 ReAct 平级的执行策略，将 **规划** 和 **执行** 显式分离为两个独立阶段：
//!
//! - **Planner**: 接收用户任务，生成结构化的执行计划（步骤列表）
//! - **Executor**: 逐步执行计划中的每个步骤
//!
//! # 与 ReAct 的区别
//!
//! | 特性 | ReAct | Plan-and-Execute |
//! |------|-------|-----------------|
//! | 决策方式 | 每步实时决策 | 先规划后执行 |
//! | 适用场景 | 探索式任务 | 目标明确的多步任务 |
//! | 可控性 | 较低 | 高（计划可审查/修改） |
//! | 效率 | 可能反复尝试 | 路径更直接 |
//!
//! # 示例
//!
//! ```rust,no_run
//! use echo_agent::plan_execute::{PlanExecuteAgent, LlmPlanner, ReactExecutor};
//! use echo_agent::prelude::*;
//!
//! # #[tokio::main]
//! # async fn main() -> echo_agent::error::Result<()> {
//! // 创建 Planner 和 Executor
//! let planner = LlmPlanner::new("qwen3-max");
//! let executor_agent = ReactAgentBuilder::new()
//!     .model("qwen3-max")
//!     .name("executor")
//!     .system_prompt("你是一个任务执行助手")
//!     .enable_tools()
//!     .build()?;
//! let executor = ReactExecutor::new(executor_agent);
//!
//! // 组装 Plan-and-Execute Agent
//! let mut agent = PlanExecuteAgent::new("plan_agent", planner, executor);
//! let result = agent.execute("帮我分析并优化这段代码的性能").await?;
//! # Ok(())
//! # }
//! ```

mod executor;
mod planner;
mod types;

pub use executor::{Executor, ReactExecutor};
pub use planner::{LlmPlanner, Planner};
pub use types::{Plan, PlanStep, StepResult, StepStatus};

use crate::agent::{Agent, AgentEvent};
use crate::error::Result;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use tracing::{debug, info, warn};

/// Plan-and-Execute Agent
///
/// 将任务分为规划和执行两个阶段。Planner 生成计划，Executor 逐步执行。
/// 支持执行过程中的计划动态调整（re-planning）。
pub struct PlanExecuteAgent<P: Planner, E: Executor> {
    name: String,
    planner: P,
    executor: E,
    /// 最大重新规划次数（防止无限循环）
    max_replans: usize,
    /// 是否启用自动重新规划（当步骤失败时）
    enable_replan: bool,
}

impl<P: Planner, E: Executor> PlanExecuteAgent<P, E> {
    pub fn new(name: impl Into<String>, planner: P, executor: E) -> Self {
        Self {
            name: name.into(),
            planner,
            executor,
            max_replans: 3,
            enable_replan: true,
        }
    }

    /// 设置最大重新规划次数
    pub fn max_replans(mut self, n: usize) -> Self {
        self.max_replans = n;
        self
    }

    /// 禁用自动重新规划
    pub fn disable_replan(mut self) -> Self {
        self.enable_replan = false;
        self
    }

    /// 核心执行循环
    async fn run_plan_execute(&mut self, task: &str) -> Result<String> {
        let agent = self.name.clone();

        // ── 阶段 1: 规划 ──────────────────────────────────────
        info!(agent = %agent, "📐 Plan-and-Execute: 阶段1 - 生成计划");
        let mut plan = self.planner.plan(task).await?;
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

        let mut replan_count = 0;
        let mut all_results: Vec<StepResult> = Vec::new();

        // ── 阶段 2: 逐步执行 ─────────────────────────────────
        info!(agent = %agent, "🚀 Plan-and-Execute: 阶段2 - 执行计划");

        loop {
            // 收集待执行步骤的索引和描述（避免借用冲突）
            let pending: Vec<(usize, String)> = plan
                .steps
                .iter()
                .enumerate()
                .filter(|(_, s)| s.status == StepStatus::Pending)
                .map(|(i, s)| (i, s.description.clone()))
                .collect();

            if pending.is_empty() {
                info!(agent = %agent, "✅ 所有步骤已执行完毕");
                break;
            }

            for (idx, step_desc) in &pending {
                let idx = *idx;
                info!(
                    agent = %agent,
                    step = idx + 1,
                    total = plan.steps.len(),
                    "⚡ 执行步骤 {}/{}: {}",
                    idx + 1,
                    plan.steps.len(),
                    step_desc
                );

                // 构建步骤上下文（包含之前步骤的结果）
                let context = self.build_step_context(&plan, idx, &all_results);
                plan.steps[idx].status = StepStatus::Running;

                match self
                    .executor
                    .execute_step(step_desc, &context)
                    .await
                {
                    Ok(output) => {
                        info!(
                            agent = %agent,
                            step = idx + 1,
                            "✅ 步骤 {} 执行成功",
                            idx + 1
                        );
                        plan.steps[idx].status = StepStatus::Completed;
                        all_results.push(StepResult {
                            step_index: idx,
                            description: step_desc.clone(),
                            output: output.clone(),
                            success: true,
                        });
                    }
                    Err(e) => {
                        warn!(
                            agent = %agent,
                            step = idx + 1,
                            error = %e,
                            "❌ 步骤 {} 执行失败",
                            idx + 1
                        );
                        plan.steps[idx].status = StepStatus::Failed;

                        all_results.push(StepResult {
                            step_index: idx,
                            description: step_desc.clone(),
                            output: format!("执行失败: {}", e),
                            success: false,
                        });

                        // 尝试重新规划
                        if self.enable_replan && replan_count < self.max_replans {
                            replan_count += 1;
                            info!(
                                agent = %agent,
                                replan = replan_count,
                                max = self.max_replans,
                                "🔄 触发重新规划 ({}/{})",
                                replan_count,
                                self.max_replans
                            );

                            let replan_prompt = format!(
                                "原始任务: {}\n\n已完成的步骤:\n{}\n\n失败的步骤: {}\n错误: {}\n\n请根据以上情况重新制定剩余计划。",
                                task,
                                self.format_completed_results(&all_results),
                                step_desc,
                                e
                            );

                            match self.planner.plan(&replan_prompt).await {
                                Ok(new_plan) => {
                                    // 替换剩余未执行的步骤
                                    let completed_count = plan
                                        .steps
                                        .iter()
                                        .filter(|s| s.status == StepStatus::Completed)
                                        .count();
                                    plan.steps.truncate(completed_count);
                                    plan.steps.extend(new_plan.steps);
                                    info!(
                                        agent = %agent,
                                        new_total = plan.steps.len(),
                                        "📋 重新规划完成"
                                    );
                                    break; // 跳出内层循环，重新开始执行
                                }
                                Err(replan_err) => {
                                    warn!(
                                        agent = %agent,
                                        error = %replan_err,
                                        "⚠️ 重新规划失败，继续执行剩余步骤"
                                    );
                                }
                            }
                        }
                    }
                }
            }

            // 检查是否所有步骤都已完成或失败
            let all_done = plan
                .steps
                .iter()
                .all(|s| s.status == StepStatus::Completed || s.status == StepStatus::Failed);
            if all_done {
                break;
            }
        }

        // ── 阶段 3: 汇总结果 ─────────────────────────────────
        info!(agent = %agent, "📝 Plan-and-Execute: 阶段3 - 汇总结果");

        let summary = self.summarize_results(task, &all_results).await?;
        info!(agent = %agent, "🏁 Plan-and-Execute 执行完毕");

        Ok(summary)
    }

    /// 构建单个步骤的执行上下文
    fn build_step_context(
        &self,
        plan: &Plan,
        current_idx: usize,
        results: &[StepResult],
    ) -> String {
        let mut parts = Vec::new();

        parts.push(format!("当前计划共 {} 个步骤：", plan.steps.len()));
        for (i, step) in plan.steps.iter().enumerate() {
            let status_icon = match step.status {
                StepStatus::Completed => "✅",
                StepStatus::Running => "▶️",
                StepStatus::Failed => "❌",
                StepStatus::Pending => "⏳",
            };
            parts.push(format!(
                "  {} 步骤 {}: {}",
                status_icon,
                i + 1,
                step.description
            ));
        }

        if !results.is_empty() {
            parts.push("\n已完成步骤的结果：".to_string());
            for r in results {
                parts.push(format!(
                    "  - 步骤 {}: {} → {}",
                    r.step_index + 1,
                    r.description,
                    if r.output.len() > 200 {
                        format!("{}...", &r.output[..200])
                    } else {
                        r.output.clone()
                    }
                ));
            }
        }

        parts.push(format!(
            "\n请执行步骤 {}: {}",
            current_idx + 1,
            plan.steps[current_idx].description
        ));

        parts.join("\n")
    }

    /// 格式化已完成的结果
    fn format_completed_results(&self, results: &[StepResult]) -> String {
        results
            .iter()
            .filter(|r| r.success)
            .map(|r| format!("  - {}: {}", r.description, r.output))
            .collect::<Vec<_>>()
            .join("\n")
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
            let result = self.run_plan_execute(task).await?;
            let stream = futures::stream::once(async move { Ok(AgentEvent::FinalAnswer(result)) });
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
}
