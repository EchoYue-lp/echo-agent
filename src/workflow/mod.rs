//! 通用 Workflow / Pipeline 抽象
//!
//! 提供三种编排模式，将多个 [`Agent`](crate::agent::Agent) 组合为可复用的执行管道：
//!
//! | 类型 | 说明 |
//! |------|------|
//! | [`SequentialWorkflow`] | 顺序管道，前一步输出作为后一步输入 |
//! | [`ConcurrentWorkflow`] | 并发管道，所有 Agent 并行执行后合并结果 |
//! | [`DagWorkflow`] | DAG 管道，按拓扑序执行，独立节点自动并发 |
//!
//! # 快速上手
//!
//! ```rust,no_run
//! use echo_agent::prelude::*;
//! use echo_agent::workflow::{SequentialWorkflow, ConcurrentWorkflow};
//!
//! # fn example() -> echo_agent::error::Result<()> {
//! let agent_a = ReactAgentBuilder::simple("qwen3-max", "你是翻译")?;
//! let agent_b = ReactAgentBuilder::simple("qwen3-max", "你是校对")?;
//!
//! let mut wf = SequentialWorkflow::builder()
//!     .step(agent_a)
//!     .step(agent_b)
//!     .build();
//! # Ok(())
//! # }
//! ```

mod concurrent;
mod dag;
mod sequential;

pub use concurrent::{ConcurrentWorkflow, ConcurrentWorkflowBuilder};
pub use dag::{DagEdge, DagNode, DagWorkflow, DagWorkflowBuilder};
pub use sequential::{SequentialWorkflow, SequentialWorkflowBuilder, WorkflowStep};

use crate::agent::Agent;
use crate::error::Result;
use futures::future::BoxFuture;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex as AsyncMutex;

/// 可共享的 Agent 句柄，支持跨异步任务安全访问
pub type SharedAgent = Arc<AsyncMutex<Box<dyn Agent>>>;

/// 将任意 `impl Agent` 包装为 [`SharedAgent`]
pub fn shared_agent(agent: impl Agent + 'static) -> SharedAgent {
    Arc::new(AsyncMutex::new(Box::new(agent)))
}

/// Workflow 统一执行接口
pub trait Workflow: Send + Sync {
    /// 以 `input` 为初始输入运行整个工作流
    fn run<'a>(&'a mut self, input: &'a str) -> BoxFuture<'a, Result<WorkflowOutput>>;
}

/// Workflow 执行的完整输出
#[derive(Debug, Clone)]
pub struct WorkflowOutput {
    /// 最终结果文本
    pub result: String,
    /// 每一步的详细输出
    pub steps: Vec<StepOutput>,
    /// 总耗时
    pub elapsed: Duration,
}

/// 单步执行的详细输出
#[derive(Debug, Clone)]
pub struct StepOutput {
    /// 执行该步的 Agent 名称
    pub agent_name: String,
    /// 该步接收到的输入
    pub input: String,
    /// 该步产出的输出
    pub output: String,
    /// 该步耗时
    pub elapsed: Duration,
}
