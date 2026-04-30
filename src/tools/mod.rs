//! Tool system — define and register tools for agents to call.
//!
//! Tools are the primary way agents interact with the world: calling APIs, reading
//! files, executing code, searching the web, and more.
//!
//! # Defining a Tool — The `#[tool]` Macro
//!
//! The recommended way to define a tool is with the [`#[tool]`](macro@crate::tool) macro:
//!
//! ```rust,no_run
//! use echo_agent::{tool, prelude::*};
//!
//! #[tool(name = "add", description = "Add two numbers")]
//! async fn add(a: f64, b: f64) -> Result<ToolResult> {
//!     Ok(ToolResult::success(format!("{}", a + b)))
//! }
//! ```
//!
//! This generates `AddParams`, `AddTool`, and a full `Tool` trait implementation
//! with automatic JSON Schema generation.
//!
//! # Registering Tools
//!
//! ```rust,ignore
//! use echo_agent::prelude::*;
//!
//! # fn main() -> echo_agent::error::Result<()> {
//! let agent = ReactAgentBuilder::new()
//!     .model("qwen3-max")
//!     .register_tool(Arc::new(AddTool))
//!     .build()?;
//! # Ok(())
//! # }
//! ```
//!
//! # Built-in Tools
//!
//! | Module | Tools | Feature |
//! |--------|-------|---------|
//! | [`builtin`] | `ThinkTool`, `FinalAnswerTool`, memory tools, data tools, git, RAG, chart | various |
//! | [`web`] | `WebSearchTool`, `WebFetchTool` | `web` |
//! | [`media`] | `ImageFetchTool`, PDF/Excel/Word tools | `media` |
//! | [`files`] | File read/write/list/delete tools | default |
//! | [`shell`] | Shell command execution (sandboxed) | default |
//!
//! # Key Types
//!
//! | Type | Description |
//! |------|-------------|
//! | [`Tool`] | Trait — implement `name()`, `description()`, `parameters()`, `execute()` |
//! | [`ToolManager`] | Registry — register, list, execute tools with concurrency control |
//! | [`ToolResult`] | `ToolResult::success(...)` / `ToolResult::error(...)` |
//! | [`ToolExecutionConfig`] | Timeout, retry count, max concurrency per tool |

/// Built-in tools (security, think, etc.)
pub mod builtin;
/// File manipulation tools
pub mod files;
pub mod permission;
pub mod shell;

#[cfg(feature = "web")]
pub mod web;

#[cfg(feature = "media")]
pub mod media;

/// Direct re-exports from `echo_execution::tools`.
pub mod execution {
    pub use echo_execution::tools::*;
}

pub use echo_execution::tools::{
    Tool, ToolExecutionConfig, ToolManager, ToolParameters, ToolResult,
};
