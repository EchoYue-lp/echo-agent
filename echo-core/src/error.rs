//! Unified error types
//!
//! All public APIs return [`Result<T>`]; underlying errors are automatically converted
//! to [`ReactError`] through `From`.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Stable failure domain carried across Agent event and adapter boundaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentFailureCategory {
    Llm,
    Tool,
    Parse,
    Agent,
    Config,
    Mcp,
    Memory,
    Sandbox,
    RuntimeState,
    Channel,
    Io,
    Other,
}

/// Terminal lifecycle class independent of display text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentTerminalKind {
    Failed,
    Cancelled,
    TimedOut,
    PermissionDenied,
}

/// Serializable non-Tool failure contract for Agent event consumers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentFailure {
    pub category: AgentFailureCategory,
    pub terminal_kind: AgentTerminalKind,
    pub retryable: bool,
    pub code: String,
    pub http_status: Option<u16>,
    pub message: String,
}

impl AgentFailure {
    pub fn from_react_error(error: &ReactError) -> Self {
        let (category, terminal_kind, retryable, code, http_status) = match error {
            ReactError::Llm(inner) => match inner.as_ref() {
                LlmError::NetworkError(_) => (
                    AgentFailureCategory::Llm,
                    AgentTerminalKind::Failed,
                    true,
                    "llm_network",
                    None,
                ),
                LlmError::ApiError { status, .. } => (
                    AgentFailureCategory::Llm,
                    AgentTerminalKind::Failed,
                    *status == 408 || *status == 429 || *status >= 500,
                    "llm_api",
                    Some(*status),
                ),
                LlmError::InvalidResponse(_) => (
                    AgentFailureCategory::Llm,
                    AgentTerminalKind::Failed,
                    false,
                    "llm_invalid_response",
                    None,
                ),
                LlmError::EmptyResponse => (
                    AgentFailureCategory::Llm,
                    AgentTerminalKind::Failed,
                    true,
                    "llm_empty_response",
                    None,
                ),
                LlmError::SerializationError(_) => (
                    AgentFailureCategory::Llm,
                    AgentTerminalKind::Failed,
                    false,
                    "llm_serialization",
                    None,
                ),
            },
            ReactError::Tool(_) => (
                AgentFailureCategory::Tool,
                AgentTerminalKind::Failed,
                false,
                "tool",
                None,
            ),
            ReactError::Parse(_) => (
                AgentFailureCategory::Parse,
                AgentTerminalKind::Failed,
                false,
                "parse",
                None,
            ),
            ReactError::Agent(inner) => match inner.as_ref() {
                AgentError::Interrupted | AgentError::Cancelled(_) => (
                    AgentFailureCategory::Agent,
                    AgentTerminalKind::Cancelled,
                    false,
                    "agent_cancelled",
                    None,
                ),
                AgentError::Timeout(_) => (
                    AgentFailureCategory::Agent,
                    AgentTerminalKind::TimedOut,
                    true,
                    "agent_timeout",
                    None,
                ),
                AgentError::PermissionDenied(_) => (
                    AgentFailureCategory::Agent,
                    AgentTerminalKind::PermissionDenied,
                    false,
                    "agent_permission_denied",
                    None,
                ),
                _ => (
                    AgentFailureCategory::Agent,
                    AgentTerminalKind::Failed,
                    false,
                    "agent",
                    None,
                ),
            },
            ReactError::Config(_) => (
                AgentFailureCategory::Config,
                AgentTerminalKind::Failed,
                false,
                "config",
                None,
            ),
            #[cfg(feature = "mcp")]
            ReactError::Mcp(_) => (
                AgentFailureCategory::Mcp,
                AgentTerminalKind::Failed,
                true,
                "mcp",
                None,
            ),
            ReactError::Memory(_) => (
                AgentFailureCategory::Memory,
                AgentTerminalKind::Failed,
                false,
                "memory",
                None,
            ),
            ReactError::Sandbox(inner) => match inner.as_ref() {
                SandboxError::Timeout(_) => (
                    AgentFailureCategory::Sandbox,
                    AgentTerminalKind::TimedOut,
                    true,
                    "sandbox_timeout",
                    None,
                ),
                SandboxError::Cancelled(_) => (
                    AgentFailureCategory::Sandbox,
                    AgentTerminalKind::Cancelled,
                    false,
                    "sandbox_cancelled",
                    None,
                ),
                SandboxError::PermissionDenied(_) => (
                    AgentFailureCategory::Sandbox,
                    AgentTerminalKind::PermissionDenied,
                    false,
                    "sandbox_permission_denied",
                    None,
                ),
                _ => (
                    AgentFailureCategory::Sandbox,
                    AgentTerminalKind::Failed,
                    false,
                    "sandbox",
                    None,
                ),
            },
            ReactError::RuntimeState(_) => (
                AgentFailureCategory::RuntimeState,
                AgentTerminalKind::Failed,
                false,
                "runtime_state",
                None,
            ),
            #[cfg(feature = "channels")]
            ReactError::Channel(_) => (
                AgentFailureCategory::Channel,
                AgentTerminalKind::Failed,
                true,
                "channel",
                None,
            ),
            ReactError::Io(_) => (
                AgentFailureCategory::Io,
                AgentTerminalKind::Failed,
                true,
                "io",
                None,
            ),
            ReactError::Other(_) => (
                AgentFailureCategory::Other,
                AgentTerminalKind::Failed,
                false,
                "other",
                None,
            ),
        };
        Self {
            category,
            terminal_kind,
            retryable,
            code: code.to_string(),
            http_status,
            message: error.to_string(),
        }
    }

    pub fn message(source: &str, message: impl Into<String>) -> Self {
        Self {
            category: AgentFailureCategory::Other,
            terminal_kind: AgentTerminalKind::Failed,
            retryable: false,
            code: source.to_string(),
            http_status: None,
            message: message.into(),
        }
    }
}

/// Top-level framework error, aggregating all subsystem errors
///
/// # Box wrapping convention
///
/// All sub-errors are `Box`-wrapped to keep the enum size small (Rust enums
/// are sized by their largest variant). This avoids bloating the `Result<T>`
/// return type on every call site. `From` impls are provided manually so
/// callers can use `?` without explicit boxing.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ReactError {
    /// LLM-related error
    #[error("LLM Error: {0}")]
    Llm(Box<LlmError>),
    /// Tool execution error
    #[error("Tool Error: {0}")]
    Tool(Box<ToolError>),
    /// Parse error
    #[error("Parse Error: {0}")]
    Parse(Box<ParseError>),
    /// Agent execution error
    #[error("Agent Error: {0}")]
    Agent(Box<AgentError>),
    /// Configuration error
    #[error("Config Error: {0}")]
    Config(Box<ConfigError>),
    /// MCP-related error
    #[cfg(feature = "mcp")]
    #[error("MCP Error: {0}")]
    Mcp(Box<McpError>),
    /// Memory system error
    #[error("Memory Error: {0}")]
    Memory(Box<MemoryError>),
    /// Sandbox error
    #[error("Sandbox Error: {0}")]
    Sandbox(Box<SandboxError>),
    /// Runtime state error
    #[error("Runtime State Error: {0}")]
    RuntimeState(Box<RuntimeStateError>),
    /// Channel / IM integration error
    #[cfg(feature = "channels")]
    #[error("Channel Error: {0}")]
    Channel(Box<ChannelError>),
    /// IO error
    #[error("IO Error: {0}")]
    Io(#[from] std::io::Error),
    /// Other error
    #[error("{0}")]
    Other(String),
}

/// Memory system error
#[derive(Debug, Error)]
pub enum MemoryError {
    /// I/O error
    #[error("IO error: {0}")]
    IoError(String),
    /// Serialization error
    #[error("Serialization error: {0}")]
    SerializationError(String),
    /// Memory not found
    #[error("Memory '{0}' not found")]
    NotFound(String),
    /// Unsupported operation
    #[error("Unsupported operation: {0}")]
    Unsupported(String),
    /// A reviewed mutation proposal no longer matches current memory state.
    #[error("Stale proposal: {0}")]
    StaleProposal(String),
}

impl From<std::io::Error> for MemoryError {
    fn from(err: std::io::Error) -> Self {
        MemoryError::IoError(err.to_string())
    }
}

/// LLM-related error
#[derive(Debug, Error)]
pub enum LlmError {
    /// Network error
    #[error("Network error: {0}")]
    NetworkError(String),
    /// API error (status code and message)
    #[error("API error (status {status}): {message}")]
    ApiError {
        /// HTTP status code
        status: u16,
        /// Error message
        message: String,
    },
    /// Invalid response
    #[error("Invalid response: {0}")]
    InvalidResponse(String),
    /// Empty response
    #[error("Empty response from LLM")]
    EmptyResponse,
    /// Serialization error
    #[error("Serialization error: {0}")]
    SerializationError(String),
}

/// Tool execution error
#[derive(Debug, Error)]
pub enum ToolError {
    /// Tool not found
    #[error("Tool '{0}' not found")]
    NotFound(String),
    /// Missing parameter
    #[error("Missing parameter: {0}")]
    MissingParameter(String),
    /// Invalid parameter
    #[error("Invalid parameter '{name}': {message}")]
    InvalidParameter {
        /// Parameter name
        name: String,
        /// Error message
        message: String,
    },
    /// Tool execution failed
    #[error("Tool '{tool}' execution failed: {message}")]
    ExecutionFailed {
        /// Tool name
        tool: String,
        /// Error message
        message: String,
    },
    /// Execution timed out
    #[error("Tool '{0}' execution timed out")]
    Timeout(String),
    /// Invalid path (path traversal attack detected)
    #[error("Invalid path: {path} ({reason})")]
    InvalidPath {
        /// Rejected path
        path: String,
        /// Rejection reason
        reason: String,
    },
    /// Access denied (outside allowed directory scope)
    #[error("Access denied: {path} ({reason})")]
    AccessDenied {
        /// Rejected path
        path: String,
        /// Rejection reason
        reason: String,
    },
    /// File too large
    #[error("File too large: {size} bytes (max: {max} bytes)")]
    FileTooLarge {
        /// File size (bytes)
        size: u64,
        /// Maximum allowed file size (bytes)
        max: u64,
    },
}

/// Parse error
#[derive(Debug, Error)]
pub enum ParseError {
    /// Invalid Thought format
    #[error("Invalid Thought: {0}")]
    InvalidThought(String),
    /// Invalid Action format
    #[error("Invalid Action: {0}")]
    InvalidAction(String),
    /// Invalid Action Input
    #[error("Invalid Action Input: {0}")]
    InvalidActionInput(String),
    /// JSON parse error
    #[error("JSON parse error: {0}")]
    JsonError(#[from] serde_json::Error),
    /// Unexpected format
    #[error("Unexpected format: {0}")]
    UnexpectedFormat(String),
}

/// Agent execution error
#[derive(Debug, Error)]
pub enum AgentError {
    /// Max iterations exceeded
    #[error("Max iterations exceeded: {0}")]
    MaxIterationsExceeded(usize),
    /// No tools available
    #[error("No tools available")]
    NoToolsAvailable,
    /// Initialization failed
    #[error("Initialization failed: {0}")]
    InitializationFailed(String),
    /// Execution interrupted
    #[error("Execution interrupted")]
    Interrupted,
    /// Execution cancelled via CancellationToken
    #[error("Cancelled: {0}")]
    Cancelled(String),
    /// No response from LLM
    #[error("No response from LLM (model: {model}, agent: {agent})")]
    NoResponse {
        /// Model name used
        model: String,
        /// Agent name
        agent: String,
    },
    /// Token limit exceeded
    #[error("Token limit exceeded")]
    TokenLimitExceeded,
    /// Permission denied
    #[error("Permission denied: {0}")]
    PermissionDenied(String),
    /// Hook execution error
    #[error("Hook error: {0}")]
    HookError(String),
    /// Subagent execution error
    #[error("Subagent error: {0}")]
    SubagentError(String),
    /// Execution timeout
    #[error("Timeout: {0}")]
    Timeout(String),
    /// Context limit exceeded (e.g. delegation depth, memory limit, etc.)
    #[error("Context limit exceeded: {0}")]
    ContextLimitExceeded(String),
}

/// MCP-related error
#[cfg(feature = "mcp")]
#[derive(Debug, Error)]
pub enum McpError {
    /// Connection failed
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),
    /// Initialization failed
    #[error("Initialization failed: {0}")]
    InitializationFailed(String),
    /// Protocol error
    #[error("Protocol error: {0}")]
    ProtocolError(String),
    /// Tool call failed with a JSON-RPC error code.
    #[error("Tool call failed ({code}): {message}")]
    ToolCallFailed { code: i32, message: String },
    /// Transport channel closed
    #[error("MCP transport closed unexpectedly")]
    TransportClosed,
}

/// Sandbox error
#[derive(Debug, Error)]
pub enum SandboxError {
    /// Sandbox unavailable (Docker not installed, no K8s cluster, etc.)
    #[error("Sandbox unavailable: {0}")]
    Unavailable(String),
    /// Sandbox start failed
    #[error("Sandbox start failed: {0}")]
    StartFailed(String),
    /// Execution timeout
    #[error("Sandbox timeout: {0}")]
    Timeout(String),
    /// Execution cancelled by the owning run.
    #[error("Sandbox execution cancelled: {0}")]
    Cancelled(String),
    /// Resource limit exceeded
    #[error("Resource exceeded: {0}")]
    ResourceExceeded(String),
    /// Permission denied
    #[error("Permission denied: {0}")]
    PermissionDenied(String),
    /// IO error
    #[error("IO error: {0}")]
    IoError(String),
}

/// Channel / IM integration error
#[cfg(feature = "channels")]
#[derive(Debug, Error)]
pub enum ChannelError {
    /// Network error
    #[error("Network error: {0}")]
    NetworkError(String),
    /// API error (status code and message)
    #[error("API error (status {status}): {message}")]
    ApiError {
        /// HTTP status code
        status: u16,
        /// Error message
        message: String,
    },
    /// Auth error
    #[error("Auth error: {0}")]
    AuthError(String),
    /// Connection error
    #[error("Connection error: {0}")]
    ConnectionError(String),
    /// Send error
    #[error("Send error: {0}")]
    SendError(String),
    /// Invalid config
    #[error("Invalid config: {0}")]
    InvalidConfig(String),
    /// Other error
    #[error("Channel error: {0}")]
    Other(String),
}

/// Configuration error
#[derive(Debug, Error)]
pub enum ConfigError {
    /// Environment variable parse error
    #[error("Failed to parse environment variable: {0}")]
    EnvParseError(String),
    /// Missing config entry
    #[error("Model '{0}' missing required config: {1}")]
    MissingConfig(String, String),
    /// Invalid environment variable format
    #[error("Invalid environment variable format: {0}")]
    EnvFormatError(String),
    /// Config mismatch
    #[error("Model '{0}' mismatched config error: {1}")]
    UnMatchConfigError(String, String),
    /// Model config not found
    #[error("No configuration found for model: {0}")]
    NotFindModelError(String),
    /// Config file error
    #[error("Config file error: {0}")]
    ConfigFileError(String),
}

// ── From implementation (Box wrapping + custom conversions) ────────────────────────────────────

impl From<LlmError> for ReactError {
    fn from(err: LlmError) -> Self {
        ReactError::Llm(Box::new(err))
    }
}

impl From<ToolError> for ReactError {
    fn from(err: ToolError) -> Self {
        ReactError::Tool(Box::new(err))
    }
}

impl From<ParseError> for ReactError {
    fn from(err: ParseError) -> Self {
        ReactError::Parse(Box::new(err))
    }
}

impl From<AgentError> for ReactError {
    fn from(err: AgentError) -> Self {
        ReactError::Agent(Box::new(err))
    }
}

impl From<ConfigError> for ReactError {
    fn from(err: ConfigError) -> Self {
        ReactError::Config(Box::new(err))
    }
}

#[cfg(feature = "mcp")]
impl From<McpError> for ReactError {
    fn from(err: McpError) -> Self {
        ReactError::Mcp(Box::new(err))
    }
}

impl From<MemoryError> for ReactError {
    fn from(err: MemoryError) -> Self {
        ReactError::Memory(Box::new(err))
    }
}

impl From<SandboxError> for ReactError {
    fn from(err: SandboxError) -> Self {
        ReactError::Sandbox(Box::new(err))
    }
}

#[cfg(feature = "channels")]
impl From<ChannelError> for ReactError {
    fn from(err: ChannelError) -> Self {
        ReactError::Channel(Box::new(err))
    }
}

impl From<serde_json::Error> for ReactError {
    fn from(err: serde_json::Error) -> Self {
        ReactError::Parse(Box::new(ParseError::JsonError(err)))
    }
}

#[cfg(feature = "reqwest")]
impl From<reqwest::Error> for ReactError {
    fn from(err: reqwest::Error) -> Self {
        if err.is_timeout() {
            ReactError::Llm(Box::new(LlmError::NetworkError(
                "Request timeout".to_string(),
            )))
        } else if err.is_connect() {
            ReactError::Llm(Box::new(LlmError::NetworkError(format!(
                "Connection failed: {}",
                err
            ))))
        } else {
            ReactError::Llm(Box::new(LlmError::NetworkError(err.to_string())))
        }
    }
}

/// Convenience Result alias
pub type Result<T> = std::result::Result<T, ReactError>;

#[cfg(test)]
mod agent_failure_tests {
    use super::*;

    #[test]
    fn preserves_terminal_and_retry_facts() -> std::result::Result<(), serde_json::Error> {
        let cases = [
            (
                ReactError::Llm(Box::new(LlmError::ApiError {
                    status: 429,
                    message: "rate limited".to_string(),
                })),
                AgentFailureCategory::Llm,
                AgentTerminalKind::Failed,
                true,
                Some(429),
            ),
            (
                ReactError::Agent(Box::new(AgentError::Cancelled("stop".to_string()))),
                AgentFailureCategory::Agent,
                AgentTerminalKind::Cancelled,
                false,
                None,
            ),
            (
                ReactError::Agent(Box::new(AgentError::Timeout("late".to_string()))),
                AgentFailureCategory::Agent,
                AgentTerminalKind::TimedOut,
                true,
                None,
            ),
            (
                ReactError::Agent(Box::new(AgentError::PermissionDenied("no".to_string()))),
                AgentFailureCategory::Agent,
                AgentTerminalKind::PermissionDenied,
                false,
                None,
            ),
            (
                ReactError::Parse(Box::new(ParseError::UnexpectedFormat("bad".to_string()))),
                AgentFailureCategory::Parse,
                AgentTerminalKind::Failed,
                false,
                None,
            ),
            (
                ReactError::Io(std::io::Error::other("disk")),
                AgentFailureCategory::Io,
                AgentTerminalKind::Failed,
                true,
                None,
            ),
        ];
        for (error, category, terminal, retryable, status) in cases {
            let failure = AgentFailure::from_react_error(&error);
            assert_eq!(failure.category, category);
            assert_eq!(failure.terminal_kind, terminal);
            assert_eq!(failure.retryable, retryable);
            assert_eq!(failure.http_status, status);
            let json = serde_json::to_string(&failure)?;
            let decoded: AgentFailure = serde_json::from_str(&json)?;
            assert_eq!(decoded, failure);
        }
        Ok(())
    }
}

/// Runtime state error
#[derive(Debug, Error)]
pub enum RuntimeStateError {
    /// I/O error
    #[error("IO error: {0}")]
    Io(String),
    /// Serialization error
    #[error("Serialization error: {0}")]
    SerializationError(String),
    /// State not found
    #[error("State not found: {0}")]
    NotFound(String),
}

impl From<RuntimeStateError> for ReactError {
    fn from(err: RuntimeStateError) -> Self {
        ReactError::RuntimeState(Box::new(err))
    }
}
