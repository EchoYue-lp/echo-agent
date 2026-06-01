//! Simplified LSP protocol types.
//!
//! These are the types used by the `LspClient` trait and the LSP tools.
//! We define our own simplified versions rather than depending on the
//! full `lsp-types` crate to keep the core crate lightweight.

use serde::{Deserialize, Serialize};

/// A position in a text document (0-indexed line and character).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    /// Line number (0-indexed).
    pub line: u32,
    /// Character offset within the line (0-indexed).
    pub character: u32,
}

/// A range in a text document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Range {
    /// Start position (inclusive).
    pub start: Position,
    /// End position (exclusive).
    pub end: Position,
}

/// A location in a specific file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    /// File URI (e.g., `file:///path/to/file.rs`).
    pub uri: String,
    /// Range within the file.
    pub range: Range,
}

/// Diagnostic severity level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

/// A diagnostic (error, warning, etc.) reported by the language server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    /// The range in the document where the diagnostic applies.
    pub range: Range,
    /// Severity level.
    pub severity: DiagnosticSeverity,
    /// Human-readable message describing the issue.
    pub message: String,
    /// Source of the diagnostic (e.g., "rust-analyzer", "pyright").
    pub source: Option<String>,
    /// Optional diagnostic code (e.g., "E0308" for Rust type mismatch).
    pub code: Option<String>,
}

/// Hover information returned by the language server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HoverInfo {
    /// Markdown-formatted content.
    pub contents: String,
    /// Optional range that the hover applies to.
    pub range: Option<Range>,
}

/// Completion item kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompletionItemKind {
    Text,
    Method,
    Function,
    Constructor,
    Field,
    Variable,
    Class,
    Interface,
    Module,
    Property,
    Unit,
    Value,
    Enum,
    Keyword,
    Snippet,
    Color,
    File,
    Reference,
    Folder,
    EnumMember,
    Constant,
    Struct,
    Event,
    Operator,
    TypeParameter,
}

/// A completion suggestion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionItem {
    /// Label displayed in the completion list.
    pub label: String,
    /// Kind of completion item.
    pub kind: CompletionItemKind,
    /// Additional detail (e.g., function signature).
    pub detail: Option<String>,
    /// Text to insert (if different from label).
    pub insert_text: Option<String>,
    /// Markdown documentation.
    pub documentation: Option<String>,
}

/// A text change in a document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextChange {
    /// The range to replace.
    pub range: Range,
    /// The new text.
    pub text: String,
}

/// LSP server configuration loaded from `.lsp.yaml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspServerConfig {
    /// Language identifier (e.g., "python", "typescript", "rust").
    pub language: String,
    /// Command to start the language server.
    pub command: String,
    /// Command-line arguments.
    #[serde(default)]
    pub args: Vec<String>,
    /// File extensions this server handles (e.g., [".py", ".pyi"]).
    #[serde(default)]
    pub extensions: Vec<String>,
    /// Environment variables to set.
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
    /// Initialization options (JSON).
    #[serde(default)]
    pub initialization_options: Option<serde_json::Value>,
    /// Maximum restart attempts before giving up.
    #[serde(default = "default_max_restarts")]
    pub max_restarts: u32,
}

fn default_max_restarts() -> u32 {
    3
}

/// Top-level LSP configuration file format (`.lsp.yaml`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspConfigFile {
    /// Map from language name to server configuration.
    pub languages: std::collections::HashMap<String, LspServerConfig>,
}

/// Status of an LSP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspServerStatus {
    /// Language identifier.
    pub language: String,
    /// Whether the server is currently running.
    pub running: bool,
    /// Whether the server has finished initialization.
    pub initialized: bool,
    /// Number of restart attempts so far.
    pub restart_count: u32,
    /// Last error message, if any.
    pub last_error: Option<String>,
    /// Process ID if running.
    pub pid: Option<u32>,
}
