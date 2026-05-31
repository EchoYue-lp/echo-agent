//! LSP (Language Server Protocol) integration — re-exports.
//!
//! This module provides access to LSP types and the manager
//! for communicating with language servers.

// Re-export core types
pub use echo_core::lsp::*;

// Re-export integration layer
pub use echo_integration::lsp::{LspConfig, LspManager, StdioLspClient};
