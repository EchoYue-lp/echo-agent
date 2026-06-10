//! Unified error types
//!
//! All public APIs return [`Result<T>`]; underlying errors are automatically converted
//! to [`ReactError`] through `From`.

use thiserror::Error;

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
    /// Tool call failed
    #[error("Tool call failed: {0}")]
    ToolCallFailed(String),
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
