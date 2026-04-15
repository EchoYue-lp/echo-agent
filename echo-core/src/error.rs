//! 统一错误类型
//!
//! 所有公共 API 返回 [`Result<T>`]，底层错误通过 `From` 自动转换为 [`ReactError`]。

use std::fmt;

/// 框架顶层错误，聚合所有子系统错误
#[derive(Debug)]
pub enum ReactError {
    /// LLM 相关错误
    Llm(LlmError),
    /// 工具执行错误
    Tool(ToolError),
    /// 解析错误
    Parse(ParseError),
    /// Agent 执行错误
    Agent(AgentError),
    /// 配置错误
    Config(ConfigError),
    /// MCP 相关错误
    Mcp(McpError),
    /// 记忆系统错误
    Memory(MemoryError),
    /// 沙箱错误
    Sandbox(SandboxError),
    /// Channel / IM 集成错误
    Channel(ChannelError),
    /// IO 错误
    Io(std::io::Error),
    /// 其他错误
    Other(String),
}

/// 记忆系统错误
#[derive(Debug)]
pub enum MemoryError {
    IoError(String),
    SerializationError(String),
    NotFound(String),
    Unsupported(String),
}

/// LLM 相关错误
#[derive(Debug)]
pub enum LlmError {
    NetworkError(String),
    ApiError { status: u16, message: String },
    InvalidResponse(String),
    EmptyResponse,
    SerializationError(String),
}

/// 工具执行错误
#[derive(Debug)]
pub enum ToolError {
    NotFound(String),
    MissingParameter(String),
    InvalidParameter {
        name: String,
        message: String,
    },
    ExecutionFailed {
        tool: String,
        message: String,
    },
    Timeout(String),
    /// 无效路径（路径遍历攻击检测）
    InvalidPath {
        path: String,
        reason: String,
    },
    /// 访问被拒绝（不在允许目录范围内）
    AccessDenied {
        path: String,
        reason: String,
    },
    /// 文件过大
    FileTooLarge {
        size: u64,
        max: u64,
    },
}

/// 解析错误
#[derive(Debug)]
pub enum ParseError {
    InvalidThought(String),
    InvalidAction(String),
    InvalidActionInput(String),
    JsonError(String),
    UnexpectedFormat(String),
}

/// Agent 执行错误
#[derive(Debug)]
pub enum AgentError {
    MaxIterationsExceeded(usize),
    NoToolsAvailable,
    InitializationFailed(String),
    Interrupted,
    NoResponse,
    TokenLimitExceeded,
}

/// MCP 相关错误
#[derive(Debug)]
pub enum McpError {
    ConnectionFailed(String),
    InitializationFailed(String),
    ProtocolError(String),
    ToolCallFailed(String),
    TransportClosed,
}

/// 沙箱错误
#[derive(Debug)]
pub enum SandboxError {
    /// 沙箱不可用（未安装 Docker、无 K8s 集群等）
    Unavailable(String),
    /// 沙箱启动失败
    StartFailed(String),
    /// 执行超时
    Timeout(String),
    /// 资源限制超出
    ResourceExceeded(String),
    /// 权限被拒绝
    PermissionDenied(String),
    /// IO 错误
    IoError(String),
}

/// Channel / IM 集成错误
#[derive(Debug)]
pub enum ChannelError {
    NetworkError(String),
    ApiError { status: u16, message: String },
    AuthError(String),
    ConnectionError(String),
    SendError(String),
    InvalidConfig(String),
    Other(String),
}

/// 配置错误
#[derive(Debug)]
pub enum ConfigError {
    EnvParseError(String),
    MissingConfig(String, String),
    EnvFormatError(String),
    UnMatchConfigError(String, String),
    NotFindModelError(String),
    ConfigFileError(String),
}

// ── Display impls ────────────────────────────────────────────────────────────

impl fmt::Display for ReactError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReactError::Llm(e) => write!(f, "LLM Error: {}", e),
            ReactError::Tool(e) => write!(f, "Tool Error: {}", e),
            ReactError::Parse(e) => write!(f, "Parse Error: {}", e),
            ReactError::Agent(e) => write!(f, "Agent Error: {}", e),
            ReactError::Config(e) => write!(f, "Config Error: {}", e),
            ReactError::Mcp(e) => write!(f, "MCP Error: {}", e),
            ReactError::Memory(e) => write!(f, "Memory Error: {}", e),
            ReactError::Sandbox(e) => write!(f, "Sandbox Error: {}", e),
            ReactError::Channel(e) => write!(f, "Channel Error: {}", e),
            ReactError::Io(e) => write!(f, "IO Error: {}", e),
            ReactError::Other(msg) => write!(f, "Error: {}", msg),
        }
    }
}

impl fmt::Display for MemoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MemoryError::IoError(msg) => write!(f, "IO error: {}", msg),
            MemoryError::SerializationError(msg) => write!(f, "Serialization error: {}", msg),
            MemoryError::NotFound(id) => write!(f, "Memory '{}' not found", id),
            MemoryError::Unsupported(op) => write!(f, "Unsupported operation: {}", op),
        }
    }
}

impl fmt::Display for McpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            McpError::ConnectionFailed(msg) => write!(f, "Connection failed: {}", msg),
            McpError::InitializationFailed(msg) => write!(f, "Initialization failed: {}", msg),
            McpError::ProtocolError(msg) => write!(f, "Protocol error: {}", msg),
            McpError::ToolCallFailed(msg) => write!(f, "Tool call failed: {}", msg),
            McpError::TransportClosed => write!(f, "MCP transport closed unexpectedly"),
        }
    }
}

impl fmt::Display for SandboxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SandboxError::Unavailable(msg) => write!(f, "Sandbox unavailable: {}", msg),
            SandboxError::StartFailed(msg) => write!(f, "Sandbox start failed: {}", msg),
            SandboxError::Timeout(msg) => write!(f, "Sandbox timeout: {}", msg),
            SandboxError::ResourceExceeded(msg) => write!(f, "Resource exceeded: {}", msg),
            SandboxError::PermissionDenied(msg) => write!(f, "Permission denied: {}", msg),
            SandboxError::IoError(msg) => write!(f, "IO error: {}", msg),
        }
    }
}

impl fmt::Display for LlmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LlmError::NetworkError(msg) => write!(f, "Network error: {}", msg),
            LlmError::ApiError { status, message } => {
                write!(f, "API error (status {}): {}", status, message)
            }
            LlmError::InvalidResponse(msg) => write!(f, "Invalid response: {}", msg),
            LlmError::EmptyResponse => write!(f, "Empty response from LLM"),
            LlmError::SerializationError(msg) => write!(f, "Serialization error: {}", msg),
        }
    }
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ToolError::NotFound(name) => write!(f, "Tool '{}' not found", name),
            ToolError::MissingParameter(name) => write!(f, "Missing parameter: {}", name),
            ToolError::InvalidParameter { name, message } => {
                write!(f, "Invalid parameter '{}': {}", name, message)
            }
            ToolError::ExecutionFailed { tool, message } => {
                write!(f, "Tool '{}' execution failed: {}", tool, message)
            }
            ToolError::Timeout(name) => write!(f, "Tool '{}' execution timed out", name),
            ToolError::InvalidPath { path, reason } => {
                write!(f, "Invalid path: {} ({})", path, reason)
            }
            ToolError::AccessDenied { path, reason } => {
                write!(f, "Access denied: {} ({})", path, reason)
            }
            ToolError::FileTooLarge { size, max } => {
                write!(f, "File too large: {} bytes (max: {} bytes)", size, max)
            }
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::InvalidThought(msg) => write!(f, "Invalid Thought: {}", msg),
            ParseError::InvalidAction(msg) => write!(f, "Invalid Action: {}", msg),
            ParseError::InvalidActionInput(msg) => write!(f, "Invalid Action Input: {}", msg),
            ParseError::JsonError(msg) => write!(f, "JSON parse error: {}", msg),
            ParseError::UnexpectedFormat(msg) => write!(f, "Unexpected format: {}", msg),
        }
    }
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentError::MaxIterationsExceeded(n) => {
                write!(f, "Max iterations exceeded: {}", n)
            }
            AgentError::NoToolsAvailable => write!(f, "No tools available"),
            AgentError::InitializationFailed(msg) => write!(f, "Initialization failed: {}", msg),
            AgentError::Interrupted => write!(f, "Execution interrupted"),
            AgentError::NoResponse => write!(f, "No response from LLM"),
            AgentError::TokenLimitExceeded => write!(f, "Token limit exceeded"),
        }
    }
}

impl fmt::Display for ChannelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChannelError::NetworkError(msg) => write!(f, "Network error: {}", msg),
            ChannelError::ApiError { status, message } => {
                write!(f, "API error (status {}): {}", status, message)
            }
            ChannelError::AuthError(msg) => write!(f, "Auth error: {}", msg),
            ChannelError::ConnectionError(msg) => write!(f, "Connection error: {}", msg),
            ChannelError::SendError(msg) => write!(f, "Send error: {}", msg),
            ChannelError::InvalidConfig(msg) => write!(f, "Invalid config: {}", msg),
            ChannelError::Other(msg) => write!(f, "Channel error: {}", msg),
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::EnvParseError(env_config) => {
                write!(f, "环境变量解析失败: {}", env_config)
            }
            ConfigError::MissingConfig(model, param) => {
                write!(f, "模型 {} 缺少必要配置项: {}", model, param)
            }
            ConfigError::EnvFormatError(env_config) => {
                write!(f, "环境变量格式错误: {}", env_config)
            }
            ConfigError::UnMatchConfigError(model, param) => {
                write!(f, "模型 {} 不匹配的配置项错误: {}", model, param)
            }
            ConfigError::NotFindModelError(model) => {
                write!(f, "未找到该模型配置: {}", model)
            }
            ConfigError::ConfigFileError(msg) => {
                write!(f, "配置文件错误: {}", msg)
            }
        }
    }
}

// ── std::error::Error impls ──────────────────────────────────────────────────

impl std::error::Error for ReactError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ReactError::Llm(e) => Some(e),
            ReactError::Tool(e) => Some(e),
            ReactError::Parse(e) => Some(e),
            ReactError::Agent(e) => Some(e),
            ReactError::Config(e) => Some(e),
            ReactError::Mcp(e) => Some(e),
            ReactError::Memory(e) => Some(e),
            ReactError::Sandbox(e) => Some(e),
            ReactError::Channel(e) => Some(e),
            ReactError::Io(e) => Some(e),
            ReactError::Other(_) => None,
        }
    }
}

impl std::error::Error for LlmError {}
impl std::error::Error for ToolError {}
impl std::error::Error for ParseError {}
impl std::error::Error for AgentError {}
impl std::error::Error for ConfigError {}
impl std::error::Error for McpError {}
impl std::error::Error for MemoryError {}
impl std::error::Error for SandboxError {}
impl std::error::Error for ChannelError {}

// ── From 转换实现 ─────────────────────────────────────────────────────────────

impl From<std::io::Error> for ReactError {
    fn from(err: std::io::Error) -> Self {
        ReactError::Io(err)
    }
}

#[cfg(feature = "reqwest")]
impl From<reqwest::Error> for ReactError {
    fn from(err: reqwest::Error) -> Self {
        if err.is_timeout() {
            ReactError::Llm(LlmError::NetworkError("Request timeout".to_string()))
        } else if err.is_connect() {
            ReactError::Llm(LlmError::NetworkError(format!(
                "Connection failed: {}",
                err
            )))
        } else {
            ReactError::Llm(LlmError::NetworkError(err.to_string()))
        }
    }
}

impl From<serde_json::Error> for ReactError {
    fn from(err: serde_json::Error) -> Self {
        ReactError::Parse(ParseError::JsonError(err.to_string()))
    }
}

impl From<ConfigError> for ReactError {
    fn from(err: ConfigError) -> Self {
        ReactError::Config(err)
    }
}

impl From<LlmError> for ReactError {
    fn from(err: LlmError) -> Self {
        ReactError::Llm(err)
    }
}

impl From<ToolError> for ReactError {
    fn from(err: ToolError) -> Self {
        ReactError::Tool(err)
    }
}

impl From<ParseError> for ReactError {
    fn from(err: ParseError) -> Self {
        ReactError::Parse(err)
    }
}

impl From<AgentError> for ReactError {
    fn from(err: AgentError) -> Self {
        ReactError::Agent(err)
    }
}

impl From<McpError> for ReactError {
    fn from(err: McpError) -> Self {
        ReactError::Mcp(err)
    }
}

impl From<MemoryError> for ReactError {
    fn from(err: MemoryError) -> Self {
        ReactError::Memory(err)
    }
}

impl From<SandboxError> for ReactError {
    fn from(err: SandboxError) -> Self {
        ReactError::Sandbox(err)
    }
}

impl From<ChannelError> for ReactError {
    fn from(err: ChannelError) -> Self {
        ReactError::Channel(err)
    }
}

/// 便捷 Result 别名
pub type Result<T> = std::result::Result<T, ReactError>;
