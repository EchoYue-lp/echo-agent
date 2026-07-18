//! Graph workflow engine + general Workflow / Pipeline
//!
//! Provides two orchestration capabilities:
//!
//! ## 1. Graph Workflow (analogous to LangGraph)
//!
//! Models agent execution as a **directed graph + shared state**, supporting:
//! - Linear pipelines, conditional branching, cycles, parallel fan-out/fan-in
//!
//! | Concept | Type | Description |
//! |------|------|------|
//! | State | [`SharedState`] | KV store shared between nodes + structured message history |
//! | Graph | [`Graph`] | Compiled immutable workflow |
//! | Builder | [`GraphBuilder`] | Chain API for building graphs |
//!
//! ## 2. Pipeline Workflow (Sequential / Concurrent / DAG)
//!
//! | Type | Description |
//! |------|------|
//! | [`SequentialWorkflow`] | Sequential pipeline: previous step output becomes next step input |
//! | [`ConcurrentWorkflow`] | Concurrent pipeline: all agents execute in parallel, then merge results |
//! | [`DagWorkflow`] | DAG pipeline: topological execution, independent nodes run concurrently |

// ── Graph Workflow ────────────────────────────────────────────────────────────

pub mod checkpoint_store;
mod graph;
mod node;
pub mod pipelines;
pub mod state;

pub use checkpoint_store::{
    Checkpoint, CheckpointInfo, CheckpointStore, FileCheckpointStore, InterruptType,
    MemoryCheckpointStore,
};

pub use graph::{
    Graph, GraphBuilder, GraphResult, InterruptConfig, InterruptState, RunUntilInterruptResult,
};
pub use state::SharedState;

// ── Pipeline Workflow ─────────────────────────────────────────────────────────

mod concurrent;
mod dag;
mod sequential;

pub use concurrent::{ConcurrentWorkflow, ConcurrentWorkflowBuilder};
pub use dag::{DagEdge, DagNode, DagWorkflow, DagWorkflowBuilder};
pub use sequential::{SequentialWorkflow, SequentialWorkflowBuilder, WorkflowStep};

// ── Pre-built Pipelines ──────────────────────────────────────────────────────

pub use pipelines::{
    DataPipelineConfig, DataPipelineLanguage, WritingPipelineConfig, run_data_pipeline,
    run_writing_pipeline,
};

use echo_core::agent::Agent;
use echo_core::error::Result;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use std::sync::Arc;
use std::time::Duration;

/// Shareable agent handle for safe access across async tasks.
///
/// Because the [`Agent`] trait takes `&self` for all methods and concrete
/// implementations (e.g. `ReactAgent`) use interior mutability for shared
/// state, the outer lock is unnecessary — `Arc<dyn Agent>` is sufficient.
pub type SharedAgent = Arc<dyn Agent>;

/// Wrap arbitrary `impl Agent` as a [`SharedAgent`]
pub fn shared_agent(agent: impl Agent + 'static) -> SharedAgent {
    Arc::new(agent)
}

/// Backward-compatible type alias for call sites that still reference the
/// old `AsyncMutex` pattern. Marked deprecated to encourage migration.
#[deprecated(note = "Use `SharedAgent` (now `Arc<dyn Agent>`) directly")]
pub type SharedAgentMutex = Arc<tokio::sync::Mutex<Box<dyn Agent>>>;

/// Step-by-step events emitted during workflow execution.
///
/// Obtain a `BoxStream<WorkflowEvent>` via [`Workflow::run_stream`]
/// for realtime UI updates, progress bars, logging, etc.
#[derive(Debug, Clone)]
pub enum WorkflowEvent {
    /// Node started execution
    NodeStart {
        node_name: String,
        step_index: usize,
    },
    /// Node finished execution
    NodeEnd {
        node_name: String,
        step_index: usize,
        elapsed: Duration,
    },
    /// Token produced by node (forwarded during streaming agent output)
    Token { node_name: String, token: String },
    /// Node execution error (non-fatal; error is recorded but stream continues)
    NodeError { node_name: String, error: String },
    /// Workflow execution completed
    Completed {
        result: String,
        total_steps: usize,
        elapsed: Duration,
    },
}

/// Unified workflow execution interface
pub trait Workflow: Send + Sync {
    /// Run the entire workflow with `input` as the initial input
    fn run<'a>(&'a mut self, input: &'a str) -> BoxFuture<'a, Result<WorkflowOutput>>;

    /// Run the entire workflow with `input` as initial input (streaming per-node events).
    ///
    /// Default implementation falls back to `run()` and only emits the `Completed` event.
    fn run_stream<'a>(
        &'a mut self,
        input: &'a str,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<WorkflowEvent>>>> {
        Box::pin(async move {
            let output = self.run(input).await?;
            let event = WorkflowEvent::Completed {
                result: output.result,
                total_steps: output.steps.len(),
                elapsed: output.elapsed,
            };
            let stream: BoxStream<'a, Result<WorkflowEvent>> =
                Box::pin(futures::stream::once(async { Ok(event) }));
            Ok(stream)
        })
    }
}

/// Complete output of a workflow execution
#[derive(Debug, Clone)]
pub struct WorkflowOutput {
    /// Final result text
    pub result: String,
    /// Detailed output for each step
    pub steps: Vec<StepOutput>,
    /// Total elapsed time
    pub elapsed: Duration,
}

/// Detailed output of a single step execution
#[derive(Debug, Clone)]
pub struct StepOutput {
    /// Name of the agent that executed this step
    pub agent_name: String,
    /// Input received by this step
    pub input: String,
    /// Output produced by this step
    pub output: String,
    /// Step elapsed time
    pub elapsed: Duration,
}
