//! 统一错误类型
//!
//! 所有公共 API 返回 [`Result<T>`]，底层错误通过 `From` 自动转换为 [`ReactError`]。

use std::fmt;
use std::io;

/// 框架顶层错误，聚合所有子系统错误
#[derive(Debug)]
pub enum ReactError {
    /// LLM 相关错误
    Llm(Box<LlmError>),
    /// 工具执行错误
    Tool(ToolError),
    /// 解析错误
    Parse(ParseError),
    /// Agent 执行错误
    Agent(AgentError),
    /// 配置错误
    Config(Box<ConfigError>),
    /// MCP 相关错误
    Mcp(McpError),
    /// 记忆系统错误
    Memory(Box<MemoryError>),
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
    /// I/O 错误
    IoError(String),
    /// 序列化错误
    SerializationError(String),
    /// 记忆未找到
    NotFound(String),
    /// 不支持的操作
    Unsupported(String),
}

impl From<io::Error> for MemoryError {
    fn from(err: io::Error) -> Self {
        MemoryError::IoError(err.to_string())
    }
}

/// LLM 相关错误
#[derive(Debug)]
pub enum LlmError {
    /// 网络错误
    NetworkError(String),
    /// API 错误（状态码和消息）
    ApiError {
        /// HTTP 状态码
        status: u16,
        /// 错误消息
        message: String,
    },
    /// 无效响应
    InvalidResponse(String),
    /// 空响应
    EmptyResponse,
    /// 序列化错误
    SerializationError(String),
}

/// 工具执行错误
#[derive(Debug)]
pub enum ToolError {
    /// 工具未找到
    NotFound(String),
    /// 缺少参数
    MissingParameter(String),
    /// 无效参数
    InvalidParameter {
        /// 参数名称
        name: String,
        /// 错误消息
        message: String,
    },
    /// 工具执行失败
    ExecutionFailed {
        /// 工具名称
        tool: String,
        /// 错误消息
        message: String,
    },
    /// 执行超时
    Timeout(String),
    /// 无效路径（路径遍历攻击检测）
    InvalidPath {
        /// 被拒绝的路径
        path: String,
        /// 拒绝原因
        reason: String,
    },
    /// 访问被拒绝（不在允许目录范围内）
    AccessDenied {
        /// 被拒绝的路径
        path: String,
        /// 拒绝原因
        reason: String,
    },
    /// 文件过大
    FileTooLarge {
        /// 文件大小（字节）
        size: u64,
        /// 允许的最大文件大小（字节）
        max: u64,
    },
}

/// 解析错误
#[derive(Debug)]
pub enum ParseError {
    /// 无效的 Thought 格式
    InvalidThought(String),
    /// 无效的 Action 格式
    InvalidAction(String),
    /// 无效的 Action 输入
    InvalidActionInput(String),
    /// JSON 解析错误
    JsonError(serde_json::Error),
    /// 意外的格式
    UnexpectedFormat(String),
}

/// Agent 执行错误
#[derive(Debug)]
pub enum AgentError {
    /// 超过最大迭代次数
    MaxIterationsExceeded(usize),
    /// 无可用工具
    NoToolsAvailable,
    /// 初始化失败
    InitializationFailed(String),
    /// 执行被中断
    Interrupted,
    /// LLM 无响应
    NoResponse,
    /// Token 数量超限
    TokenLimitExceeded,
    /// 权限被拒绝
    PermissionDenied(String),
    /// Hook 执行错误
    HookError(String),
    /// 子代理执行错误
    SubagentError(String),
    /// 执行超时
    Timeout(String),
    /// 上下文限制（如 delegation depth, memory limit 等）
    ContextLimitExceeded(String),
}

/// MCP 相关错误
#[derive(Debug)]
pub enum McpError {
    /// 连接失败
    ConnectionFailed(String),
    /// 初始化失败
    InitializationFailed(String),
    /// 协议错误
    ProtocolError(String),
    /// 工具调用失败
    ToolCallFailed(String),
    /// 传输通道关闭
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
    /// 网络错误
    NetworkError(String),
    /// API 错误（状态码和消息）
    ApiError {
        /// HTTP 状态码
        status: u16,
        /// 错误消息
        message: String,
    },
    /// 认证错误
    AuthError(String),
    /// 连接错误
    ConnectionError(String),
    /// 发送错误
    SendError(String),
    /// 无效配置
    InvalidConfig(String),
    /// 其他错误
    Other(String),
}

/// 配置错误
#[derive(Debug)]
pub enum ConfigError {
    /// 环境变量解析错误
    EnvParseError(String),
    /// 缺少配置项
    MissingConfig(String, String),
    /// 环境变量格式错误
    EnvFormatError(String),
    /// 配置不匹配
    UnMatchConfigError(String, String),
    /// 未找到模型配置
    NotFindModelError(String),
    /// 配置文件错误
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
            MemoryError::IoError(err) => write!(f, "IO error: {}", err),
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
            ParseError::JsonError(err) => write!(f, "JSON parse error: {}", err),
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
            AgentError::PermissionDenied(msg) => write!(f, "Permission denied: {}", msg),
            AgentError::HookError(msg) => write!(f, "Hook error: {}", msg),
            AgentError::SubagentError(msg) => write!(f, "Subagent error: {}", msg),
            AgentError::Timeout(msg) => write!(f, "Timeout: {}", msg),
            AgentError::ContextLimitExceeded(msg) => write!(f, "Context limit exceeded: {}", msg),
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
                write!(f, "Failed to parse environment variable: {}", env_config)
            }
            ConfigError::MissingConfig(model, param) => {
                write!(f, "Model '{}' missing required config: {}", model, param)
            }
            ConfigError::EnvFormatError(env_config) => {
                write!(f, "Invalid environment variable format: {}", env_config)
            }
            ConfigError::UnMatchConfigError(model, param) => {
                write!(f, "Model '{}' mismatched config error: {}", model, param)
            }
            ConfigError::NotFindModelError(model) => {
                write!(f, "No configuration found for model: {}", model)
            }
            ConfigError::ConfigFileError(msg) => {
                write!(f, "Config file error: {}", msg)
            }
        }
    }
}

// ── std::error::Error impls ──────────────────────────────────────────────────

impl std::error::Error for ReactError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ReactError::Llm(e) => Some(e.as_ref()),
            ReactError::Tool(e) => Some(e),
            ReactError::Parse(e) => Some(e),
            ReactError::Agent(e) => Some(e),
            ReactError::Config(e) => Some(e.as_ref()),
            ReactError::Mcp(e) => Some(e),
            ReactError::Memory(e) => Some(e.as_ref()),
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

impl From<serde_json::Error> for ReactError {
    fn from(err: serde_json::Error) -> Self {
        ReactError::Parse(ParseError::JsonError(err))
    }
}

impl From<ConfigError> for ReactError {
    fn from(err: ConfigError) -> Self {
        ReactError::Config(Box::new(err))
    }
}

impl From<LlmError> for ReactError {
    fn from(err: LlmError) -> Self {
        ReactError::Llm(Box::new(err))
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
        ReactError::Memory(Box::new(err))
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
