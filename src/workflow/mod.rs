//! 图工作流引擎 + 通用 Workflow / Pipeline
//!
//! 提供两套编排能力：
//!
//! ## 1. Graph 工作流（对标 LangGraph）
//!
//! 将 Agent 执行建模为**有向图 + 共享状态**，支持：
//! - 线性管道、条件分支、循环、并行 fan-out/fan-in
//!
//! | 概念 | 类型 | 说明 |
//! |------|------|------|
//! | 状态 | [`SharedState`] | 节点间共享的 KV store + 结构化消息历史 |
//! | 图 | [`Graph`] | 编译后的不可变工作流 |
//! | 构建器 | [`GraphBuilder`] | 链式 API 构建图 |
//!
//! ## 2. Pipeline 工作流（Sequential / Concurrent / DAG）
//!
//! | 类型 | 说明 |
//! |------|------|
//! | [`SequentialWorkflow`] | 顺序管道，前一步输出作为后一步输入 |
//! | [`ConcurrentWorkflow`] | 并发管道，所有 Agent 并行执行后合并结果 |
//! | [`DagWorkflow`] | DAG 管道，按拓扑序执行，独立节点自动并发 |
//!
//! ## 快速上手
//!
//! ```rust,no_run
//! use echo_agent::workflow::{GraphBuilder, SharedState};
//!
//! # async fn example() -> echo_agent::error::Result<()> {
//! let graph = GraphBuilder::new("my_flow")
//!     .add_function_node("greet", |state| Box::pin(async move {
//!         let name: String = state.get("name").unwrap_or_else(|| "World".to_string());
//!         state.set("greeting", format!("Hello, {name}!"));
//!         Ok(())
//!     }))
//!     .set_entry("greet")
//!     .add_edge("greet", "__end__")
//!     .build()?;
//!
//! let state = SharedState::new();
//! state.set("name", "Echo");
//! let result = graph.run(state).await?;
//! assert_eq!(result.state.get::<String>("greeting"), Some("Hello, Echo!".to_string()));
//! # Ok(())
//! # }
//! ```

// ── Graph 工作流 ────────────────────────────────────────────────────────────

mod graph;
mod node;
pub mod state;

pub use graph::{Graph, GraphBuilder, GraphResult};
pub use state::SharedState;

// ── Pipeline 工作流 ─────────────────────────────────────────────────────────

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
