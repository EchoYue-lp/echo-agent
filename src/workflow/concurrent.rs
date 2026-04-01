//! 并发工作流：所有 Agent 并行执行同一输入，结果通过合并函数聚合。

use super::{SharedAgent, StepOutput, Workflow, WorkflowOutput, shared_agent};
use crate::agent::Agent;
use crate::error::Result;
use futures::future::BoxFuture;
use std::time::Instant;
use tracing::{debug, info};

/// 结果合并函数
type MergeFn = Box<dyn Fn(Vec<String>) -> String + Send + Sync>;

fn default_merge(results: Vec<String>) -> String {
    results.join("\n---\n")
}

/// 并发工作流：注册的 Agent 全部并行执行，结果通过 `merge` 函数合并。
///
/// # 示例
///
/// ```rust,no_run
/// use echo_agent::prelude::*;
/// use echo_agent::workflow::{ConcurrentWorkflow, Workflow};
///
/// # async fn example() -> echo_agent::error::Result<()> {
/// let agent_x = ReactAgentBuilder::simple("qwen3-max", "你从技术角度分析问题")?;
/// let agent_y = ReactAgentBuilder::simple("qwen3-max", "你从商业角度分析问题")?;
///
/// let mut wf = ConcurrentWorkflow::builder()
///     .agent(agent_x)
///     .agent(agent_y)
///     .merge(|results| {
///         format!("综合分析：\n{}", results.join("\n\n"))
///     })
///     .build();
///
/// let output = wf.run("分析 AI Agent 的发展趋势").await?;
/// println!("{}", output.result);
/// # Ok(())
/// # }
/// ```
pub struct ConcurrentWorkflow {
    agents: Vec<SharedAgent>,
    merge: MergeFn,
}

impl ConcurrentWorkflow {
    pub fn builder() -> ConcurrentWorkflowBuilder {
        ConcurrentWorkflowBuilder {
            agents: Vec::new(),
            merge: None,
        }
    }
}

impl Workflow for ConcurrentWorkflow {
    fn run<'a>(&'a mut self, input: &'a str) -> BoxFuture<'a, Result<WorkflowOutput>> {
        Box::pin(async move {
            let total_start = Instant::now();
            let agent_count = self.agents.len();

            info!(
                workflow = "concurrent",
                agents = agent_count,
                "⚡ 并发执行 {} 个 Agent",
                agent_count
            );

            let mut handles = Vec::with_capacity(agent_count);

            for agent_handle in &self.agents {
                let agent_handle = agent_handle.clone();
                let input = input.to_string();
                handles.push(tokio::spawn(async move {
                    let step_start = Instant::now();
                    let mut agent = agent_handle.lock().await;
                    let agent_name = agent.name().to_string();
                    debug!(workflow = "concurrent", agent = %agent_name, "▶ 开始执行");
                    let result = agent.execute(&input).await;
                    let elapsed = step_start.elapsed();
                    (agent_name, input, result, elapsed)
                }));
            }

            let mut step_outputs = Vec::with_capacity(agent_count);
            let mut results = Vec::with_capacity(agent_count);

            for handle in handles {
                let (agent_name, step_input, result, elapsed) = handle.await.map_err(|e| {
                    crate::error::ReactError::Other(format!("task join error: {e}"))
                })?;

                let output = result?;
                info!(
                    workflow = "concurrent",
                    agent = %agent_name,
                    elapsed_ms = elapsed.as_millis(),
                    "✓ Agent 完成"
                );

                step_outputs.push(StepOutput {
                    agent_name,
                    input: step_input,
                    output: output.clone(),
                    elapsed,
                });
                results.push(output);
            }

            let merged = (self.merge)(results);

            Ok(WorkflowOutput {
                result: merged,
                steps: step_outputs,
                elapsed: total_start.elapsed(),
            })
        })
    }
}

/// [`ConcurrentWorkflow`] 构建器
pub struct ConcurrentWorkflowBuilder {
    agents: Vec<SharedAgent>,
    merge: Option<MergeFn>,
}

impl ConcurrentWorkflowBuilder {
    /// 添加一个并发执行的 Agent
    pub fn agent(mut self, agent: impl Agent + 'static) -> Self {
        self.agents.push(shared_agent(agent));
        self
    }

    /// 添加一个已包装的 SharedAgent
    pub fn agent_shared(mut self, agent: SharedAgent) -> Self {
        self.agents.push(agent);
        self
    }

    /// 设置结果合并函数（默认以 `\n---\n` 连接）
    pub fn merge(mut self, f: impl Fn(Vec<String>) -> String + Send + Sync + 'static) -> Self {
        self.merge = Some(Box::new(f));
        self
    }

    pub fn build(self) -> ConcurrentWorkflow {
        ConcurrentWorkflow {
            agents: self.agents,
            merge: self.merge.unwrap_or_else(|| Box::new(default_merge)),
        }
    }
}
