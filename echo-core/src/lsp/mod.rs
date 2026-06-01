//! LSP (Language Server Protocol) integration types.
//!
//! This module defines the core trait for LSP clients and the
//! simplified protocol types used by EchoAgent's code analysis tools.
//!
//! ## Architecture
//!
//! ```text
//! echo-core/src/lsp/        ← This module: traits + types
//! echo-integration/src/lsp/ ← LspManager, process spawning, JSON-RPC
//! echo-tools/src/lsp/       ← Tool implementations (diagnostics, goto-def, etc.)
//! ```
//!
//! ## Supported capabilities
//!
//! | Capability | Method | Use case |
//! |-----------|--------|----------|
//! | Diagnostics | `lsp_diagnostics` | Errors, warnings in a file |
//! | Go to definition | `lsp_goto_definition` | Jump to symbol definition |
//! | Find references | `lsp_find_references` | Find all usages |
//! | Hover | `lsp_hover` | Type info, docs on hover |
//! | Completion | `lsp_completions` | Auto-complete suggestions |

pub mod client;
pub mod types;

pub use client::{LspClient, LspError, LspResult};
pub use types::*;
