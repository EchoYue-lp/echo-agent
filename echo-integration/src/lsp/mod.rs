//! LSP (Language Server Protocol) integration layer.
//!
//! This module provides:
//! - `LspManager` — manages multiple language server processes
//! - `StdioLspClient` — JSON-RPC client over stdio for a single server
//! - Configuration loading from `.lsp.yaml` files
//!
//! ## Architecture
//!
//! ```text
//! LspManager
//!   ├── StdioLspClient (python/pyright)
//!   ├── StdioLspClient (typescript/tsserver)
//!   └── StdioLspClient (rust/rust-analyzer)
//! ```

pub mod client;
pub mod config;
pub mod jsonrpc;
pub mod manager;

pub use client::StdioLspClient;
pub use config::LspConfig;
pub use manager::LspManager;
