//! Graph-based workflow engine (LangGraph-style state machines).
//!
//! Model agent execution as a **directed graph with shared state**.
//! Supports linear pipelines, conditional branches, loops, and parallel fan-out/fan-in.
//!
//! # Builders
//!
//! | Builder | Style | Best For |
//! |---------|-------|----------|
//! | [`GraphBuilder`] | Programmatic Rust API | Complex logic, type safety |
//! | [`WorkflowDefinition`] | YAML/JSON declaration | Simple pipelines, no-code config |
//!
//! # Quick Start (Programmatic)
//!
//! ```rust,no_run
//! use echo_agent::prelude::*;
//!
//! # fn main() -> echo_agent::error::Result<()> {
//! let graph = GraphBuilder::new("pipeline")
//!     .add_function_node("step_a", |state| Box::pin(async move {
//!         state.set("key", "value")?;
//!         Ok(())
//!     }))
//!     .add_edge("step_a", Graph::END)
//!     .build()?;
//! # Ok(())
//! # }
//! ```
//!
//! # Quick Start (Declarative YAML)
//!
//! ```rust,ignore
//! use echo_agent::prelude::*;
//!
//! # fn main() -> echo_agent::error::Result<()> {
//! let graph = Graph::from_yaml("workflow.yaml")?;
//! let result = graph.run(SharedState::new()).await?;
//! # Ok(())
//! # }
//! ```
//!
//! # Key Types
//!
//! | Type | Description |
//! |------|-------------|
//! | [`Graph`] / [`GraphBuilder`] | Build and run directed workflows |
//! | [`SequentialWorkflow`] | Simple linear step execution |
//! | [`ConcurrentWorkflow`] | Parallel step execution |
//! | [`DagWorkflow`] | Dependency-based scheduling |
//! | [`SharedState`] | State passed between nodes |
//! | [`WorkflowEvent`] | Streaming events per node |
//! | [`WorkflowDefinition`] | YAML/JSON workflow definition |
//!
//! # Pre-built Pipelines
//!
//! | Pipeline | Stages | Description |
//! |----------|--------|-------------|
//! | [`run_data_pipeline`] | load_data -> profile -> analyze -> visualize -> summarize | End-to-end data analysis |
//! | [`run_writing_pipeline`] | outline -> draft -> review -> revise (loop) -> finalize | Content creation with quality loop |

/// Direct re-exports from `echo_orchestration::workflow`.
pub mod orchestration {
    pub use echo_orchestration::workflow::*;
}

pub use echo_orchestration::workflow::*;

/// Root-crate declarative DSL built on top of the orchestration workflow types.
pub mod dsl;
/// Root-crate file/YAML loader built on top of the orchestration workflow types.
pub mod loader;

pub use loader::WorkflowDefinition;
