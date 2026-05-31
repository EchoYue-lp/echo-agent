//! LSP client trait — interface for communicating with language servers.

use crate::lsp::types::*;
use futures::future::BoxFuture;

/// Errors that can occur during LSP operations.
#[derive(Debug, thiserror::Error)]
pub enum LspError {
    #[error("Server not initialized")]
    NotInitialized,

    #[error("Server not running for language: {0}")]
    NotRunning(String),

    #[error("Request timed out")]
    Timeout,

    #[error("Server returned error: {0}")]
    ServerError(String),

    #[error("Failed to spawn server process: {0}")]
    SpawnError(String),

    #[error("Communication error: {0}")]
    CommunicationError(String),

    #[error("Invalid URI: {0}")]
    InvalidUri(String),
}

pub type LspResult<T> = Result<T, LspError>;

/// Trait for communicating with an LSP language server.
///
/// Implementations manage the lifecycle of a single language server
/// process and translate between EchoAgent's simplified types and
/// the full LSP JSON-RPC protocol.
///
/// ## Lifecycle
///
/// ```text
/// new() → initialize() → [did_open/did_change/did_save]* → shutdown()
/// ```
///
/// ## Thread safety
///
/// Implementations must be `Send + Sync` since LSP operations may be
/// called from multiple async tasks concurrently.
///
/// ## Object safety
///
/// This trait uses `BoxFuture` return types to remain object-safe,
/// allowing it to be used as `dyn LspClient`.
pub trait LspClient: Send + Sync {
    /// Language identifier (e.g., "python", "typescript", "rust").
    fn language(&self) -> &str;

    /// Whether the server process is currently running.
    fn is_running(&self) -> bool;

    /// Whether the server has completed the `initialize` handshake.
    fn is_initialized(&self) -> bool;

    /// Start the server process and perform the initialize handshake.
    ///
    /// `root_uri` is the workspace root (e.g., `file:///path/to/project`).
    fn initialize<'a>(&'a mut self, root_uri: &'a str) -> BoxFuture<'a, LspResult<()>>;

    /// Gracefully shut down the server.
    fn shutdown(&mut self) -> BoxFuture<'_, LspResult<()>>;

    /// Get diagnostics for a file.
    ///
    /// Returns errors, warnings, and other diagnostics reported by the server.
    fn diagnostics<'a>(&'a self, uri: &'a str) -> BoxFuture<'a, LspResult<Vec<Diagnostic>>>;

    /// Find the definition of the symbol at the given position.
    fn goto_definition<'a>(
        &'a self,
        uri: &'a str,
        position: Position,
    ) -> BoxFuture<'a, LspResult<Vec<Location>>>;

    /// Find all references to the symbol at the given position.
    fn find_references<'a>(
        &'a self,
        uri: &'a str,
        position: Position,
    ) -> BoxFuture<'a, LspResult<Vec<Location>>>;

    /// Get hover information for the given position.
    fn hover<'a>(
        &'a self,
        uri: &'a str,
        position: Position,
    ) -> BoxFuture<'a, LspResult<Option<HoverInfo>>>;

    /// Get completion suggestions at the given position.
    fn completion<'a>(
        &'a self,
        uri: &'a str,
        position: Position,
    ) -> BoxFuture<'a, LspResult<Vec<CompletionItem>>>;

    /// Notify the server that a file has been opened.
    fn did_open<'a>(
        &'a self,
        uri: &'a str,
        language_id: &'a str,
        text: &'a str,
    ) -> BoxFuture<'a, LspResult<()>>;

    /// Notify the server that a file has been changed.
    fn did_change<'a>(
        &'a self,
        uri: &'a str,
        changes: Vec<TextChange>,
    ) -> BoxFuture<'a, LspResult<()>>;

    /// Notify the server that a file has been saved.
    fn did_save<'a>(&'a self, uri: &'a str) -> BoxFuture<'a, LspResult<()>>;

    /// Notify the server that a file has been closed.
    fn did_close<'a>(&'a self, uri: &'a str) -> BoxFuture<'a, LspResult<()>>;

    /// Get the current server status.
    fn status(&self) -> LspServerStatus;
}
