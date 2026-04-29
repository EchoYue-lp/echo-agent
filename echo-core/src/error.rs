//! 统一错误类型
//!
//! 所有公共 API 返回 [`Result<T>`]，底层错误通过 `From` 自动转换为 [`ReactError`]。

use thiserror::Error;

/// 框架顶层错误，聚合所有子系统错误
#[derive(Debug, Error)]
pub enum ReactError {
    /// LLM 相关错误
    #[error("LLM Error: {0}")]
    Llm(Box<LlmError>),
    /// 工具执行错误
    #[error("Tool Error: {0}")]
    Tool(#[from] ToolError),
    /// 解析错误
    #[error("Parse Error: {0}")]
    Parse(#[from] ParseError),
    /// Agent 执行错误
    #[error("Agent Error: {0}")]
    Agent(#[from] AgentError),
    /// 配置错误
    #[error("Config Error: {0}")]
    Config(Box<ConfigError>),
    /// MCP 相关错误
    #[error("MCP Error: {0}")]
    Mcp(#[from] McpError),
    /// 记忆系统错误
    #[error("Memory Error: {0}")]
    Memory(Box<MemoryError>),
    /// 沙箱错误
    #[error("Sandbox Error: {0}")]
    Sandbox(#[from] SandboxError),
    /// Channel / IM 集成错误
    #[error("Channel Error: {0}")]
    Channel(#[from] ChannelError),
    /// IO 错误
    #[error("IO Error: {0}")]
    Io(#[from] std::io::Error),
    /// 其他错误
    #[error("{0}")]
    Other(String),
}

/// 记忆系统错误
#[derive(Debug, Error)]
pub enum MemoryError {
    /// I/O 错误
    #[error("IO error: {0}")]
    IoError(String),
    /// 序列化错误
    #[error("Serialization error: {0}")]
    SerializationError(String),
    /// 记忆未找到
    #[error("Memory '{0}' not found")]
    NotFound(String),
    /// 不支持的操作
    #[error("Unsupported operation: {0}")]
    Unsupported(String),
}

impl From<std::io::Error> for MemoryError {
    fn from(err: std::io::Error) -> Self {
        MemoryError::IoError(err.to_string())
    }
}

/// LLM 相关错误
#[derive(Debug, Error)]
pub enum LlmError {
    /// 网络错误
    #[error("Network error: {0}")]
    NetworkError(String),
    /// API 错误（状态码和消息）
    #[error("API error (status {status}): {message}")]
    ApiError {
        /// HTTP 状态码
        status: u16,
        /// 错误消息
        message: String,
    },
    /// 无效响应
    #[error("Invalid response: {0}")]
    InvalidResponse(String),
    /// 空响应
    #[error("Empty response from LLM")]
    EmptyResponse,
    /// 序列化错误
    #[error("Serialization error: {0}")]
    SerializationError(String),
}

/// 工具执行错误
#[derive(Debug, Error)]
pub enum ToolError {
    /// 工具未找到
    #[error("Tool '{0}' not found")]
    NotFound(String),
    /// 缺少参数
    #[error("Missing parameter: {0}")]
    MissingParameter(String),
    /// 无效参数
    #[error("Invalid parameter '{name}': {message}")]
    InvalidParameter {
        /// 参数名称
        name: String,
        /// 错误消息
        message: String,
    },
    /// 工具执行失败
    #[error("Tool '{tool}' execution failed: {message}")]
    ExecutionFailed {
        /// 工具名称
        tool: String,
        /// 错误消息
        message: String,
    },
    /// 执行超时
    #[error("Tool '{0}' execution timed out")]
    Timeout(String),
    /// 无效路径（路径遍历攻击检测）
    #[error("Invalid path: {path} ({reason})")]
    InvalidPath {
        /// 被拒绝的路径
        path: String,
        /// 拒绝原因
        reason: String,
    },
    /// 访问被拒绝（不在允许目录范围内）
    #[error("Access denied: {path} ({reason})")]
    AccessDenied {
        /// 被拒绝的路径
        path: String,
        /// 拒绝原因
        reason: String,
    },
    /// 文件过大
    #[error("File too large: {size} bytes (max: {max} bytes)")]
    FileTooLarge {
        /// 文件大小（字节）
        size: u64,
        /// 允许的最大文件大小（字节）
        max: u64,
    },
}

/// 解析错误
#[derive(Debug, Error)]
pub enum ParseError {
    /// 无效的 Thought 格式
    #[error("Invalid Thought: {0}")]
    InvalidThought(String),
    /// 无效的 Action 格式
    #[error("Invalid Action: {0}")]
    InvalidAction(String),
    /// 无效的 Action 输入
    #[error("Invalid Action Input: {0}")]
    InvalidActionInput(String),
    /// JSON 解析错误
    #[error("JSON parse error: {0}")]
    JsonError(#[from] serde_json::Error),
    /// 意外的格式
    #[error("Unexpected format: {0}")]
    UnexpectedFormat(String),
}

/// Agent 执行错误
#[derive(Debug, Error)]
pub enum AgentError {
    /// 超过最大迭代次数
    #[error("Max iterations exceeded: {0}")]
    MaxIterationsExceeded(usize),
    /// 无可用工具
    #[error("No tools available")]
    NoToolsAvailable,
    /// 初始化失败
    #[error("Initialization failed: {0}")]
    InitializationFailed(String),
    /// 执行被中断
    #[error("Execution interrupted")]
    Interrupted,
    /// LLM 无响应
    #[error("No response from LLM (model: {model}, agent: {agent})")]
    NoResponse {
        /// 使用的模型名称
        model: String,
        /// Agent 名称
        agent: String,
    },
    /// Token 数量超限
    #[error("Token limit exceeded")]
    TokenLimitExceeded,
    /// 权限被拒绝
    #[error("Permission denied: {0}")]
    PermissionDenied(String),
    /// Hook 执行错误
    #[error("Hook error: {0}")]
    HookError(String),
    /// 子代理执行错误
    #[error("Subagent error: {0}")]
    SubagentError(String),
    /// 执行超时
    #[error("Timeout: {0}")]
    Timeout(String),
    /// 上下文限制（如 delegation depth, memory limit 等）
    #[error("Context limit exceeded: {0}")]
    ContextLimitExceeded(String),
}

/// MCP 相关错误
#[derive(Debug, Error)]
pub enum McpError {
    /// 连接失败
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),
    /// 初始化失败
    #[error("Initialization failed: {0}")]
    InitializationFailed(String),
    /// 协议错误
    #[error("Protocol error: {0}")]
    ProtocolError(String),
    /// 工具调用失败
    #[error("Tool call failed: {0}")]
    ToolCallFailed(String),
    /// 传输通道关闭
    #[error("MCP transport closed unexpectedly")]
    TransportClosed,
}

/// 沙箱错误
#[derive(Debug, Error)]
pub enum SandboxError {
    /// 沙箱不可用（未安装 Docker、无 K8s 集群等）
    #[error("Sandbox unavailable: {0}")]
    Unavailable(String),
    /// 沙箱启动失败
    #[error("Sandbox start failed: {0}")]
    StartFailed(String),
    /// 执行超时
    #[error("Sandbox timeout: {0}")]
    Timeout(String),
    /// 资源限制超出
    #[error("Resource exceeded: {0}")]
    ResourceExceeded(String),
    /// 权限被拒绝
    #[error("Permission denied: {0}")]
    PermissionDenied(String),
    /// IO 错误
    #[error("IO error: {0}")]
    IoError(String),
}

/// Channel / IM 集成错误
#[derive(Debug, Error)]
pub enum ChannelError {
    /// 网络错误
    #[error("Network error: {0}")]
    NetworkError(String),
    /// API 错误（状态码和消息）
    #[error("API error (status {status}): {message}")]
    ApiError {
        /// HTTP 状态码
        status: u16,
        /// 错误消息
        message: String,
    },
    /// 认证错误
    #[error("Auth error: {0}")]
    AuthError(String),
    /// 连接错误
    #[error("Connection error: {0}")]
    ConnectionError(String),
    /// 发送错误
    #[error("Send error: {0}")]
    SendError(String),
    /// 无效配置
    #[error("Invalid config: {0}")]
    InvalidConfig(String),
    /// 其他错误
    #[error("Channel error: {0}")]
    Other(String),
}

/// 配置错误
#[derive(Debug, Error)]
pub enum ConfigError {
    /// 环境变量解析错误
    #[error("Failed to parse environment variable: {0}")]
    EnvParseError(String),
    /// 缺少配置项
    #[error("Model '{0}' missing required config: {1}")]
    MissingConfig(String, String),
    /// 环境变量格式错误
    #[error("Invalid environment variable format: {0}")]
    EnvFormatError(String),
    /// 配置不匹配
    #[error("Model '{0}' mismatched config error: {1}")]
    UnMatchConfigError(String, String),
    /// 未找到模型配置
    #[error("No configuration found for model: {0}")]
    NotFindModelError(String),
    /// 配置文件错误
    #[error("Config file error: {0}")]
    ConfigFileError(String),
}

// ── From 转换实现（Box 包装 + 自定义转换） ────────────────────────────────────

impl From<LlmError> for ReactError {
    fn from(err: LlmError) -> Self {
        ReactError::Llm(Box::new(err))
    }
}

impl From<ConfigError> for ReactError {
    fn from(err: ConfigError) -> Self {
        ReactError::Config(Box::new(err))
    }
}

impl From<MemoryError> for ReactError {
    fn from(err: MemoryError) -> Self {
        ReactError::Memory(Box::new(err))
    }
}

impl From<serde_json::Error> for ReactError {
    fn from(err: serde_json::Error) -> Self {
        ReactError::Parse(ParseError::JsonError(err))
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

/// 便捷 Result 别名
pub type Result<T> = std::result::Result<T, ReactError>;
