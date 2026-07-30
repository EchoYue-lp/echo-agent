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
/// File manipulation tools (re-export from echo_tools)
pub mod files {
    pub use echo_tools::files::*;
}
/// Path validation / confinement (re-export from echo_tools)
///
/// Re-exported here so downstream crates can depend on `echo_agent` alone
/// instead of adding a direct dep on `echo_tools` for `PathValidator`.
pub mod security {
    pub use echo_tools::security::*;
}
pub mod permission;
/// Complete oversized tool-output artifacts (re-export from echo_core).
pub mod artifact {
    pub use echo_core::tools::artifact::*;
}
/// Cursor pagination contract for collection-returning tools.
pub mod pagination {
    pub use echo_core::tools::pagination::*;
}
/// Shell tool (re-export from echo_tools)
pub mod shell {
    pub use echo_tools::shell::*;
}

/// Web tools (re-export from echo_tools)
#[cfg(feature = "web")]
pub mod web {
    pub use echo_tools::web::*;
}

/// Media tools (re-export from echo_tools)
#[cfg(feature = "media")]
pub mod media {
    pub use echo_tools::media::*;
}

/// Scholarly search and reference-manager clients (re-export from echo_tools).
#[cfg(feature = "research")]
pub mod research {
    pub use echo_tools::research::*;
}

/// LSP tools — language server integration
#[cfg(feature = "lsp")]
pub mod lsp;

/// Direct re-exports from `echo_execution::tools`.
pub mod execution {
    pub use echo_execution::tools::*;
}

pub use echo_execution::tools::{
    Tool, ToolBudgetMetricsSnapshot, ToolExecutionConfig, ToolFailure, ToolFailureCategory,
    ToolManager, ToolOutputChannel, ToolParameters, ToolRecoveryAction, ToolResult, ToolRiskLevel,
    ToolSchemaStats, ToolSearchTool, ToolSideEffect, ToolStreamEvent,
};

// ── Common file tool classification ──────────────────────────────────────────

/// Tools that modify files and should require a prior read.
pub const WRITE_TOOLS: &[&str] = &[
    "edit_file",
    "write_file",
    "append_file",
    "create_file",
    "delete_file",
    "update_file",
    "move_file",
];

/// Tools that read file content.
pub const READ_TOOLS: &[&str] = &["read_file", "read_artifact"];

/// Check if a tool name is a write tool.
pub fn is_write_tool(name: &str) -> bool {
    WRITE_TOOLS.contains(&name)
}

/// Check if a tool name is a read tool.
pub fn is_read_tool(name: &str) -> bool {
    READ_TOOLS.contains(&name)
}
