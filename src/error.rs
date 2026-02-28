use std::fmt;

/// ReAct Agent 项目的统一错误类型
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
    /// IO 错误
    Io(std::io::Error),
    /// 其他错误
    Other(String),
}

/// LLM 相关错误
#[derive(Debug)]
pub enum LlmError {
    /// 网络请求失败
    NetworkError(String),
    /// API 返回错误状态码
    ApiError { status: u16, message: String },
    /// 响应格式无效
    InvalidResponse(String),
    /// 没有返回内容
    EmptyResponse,
    /// 序列化/反序列化错误
    SerializationError(String),
}

/// 工具执行错误
#[derive(Debug)]
pub enum ToolError {
    /// 工具未找到
    NotFound(String),
    /// 参数缺失
    MissingParameter(String),
    /// 参数类型错误
    InvalidParameter { name: String, message: String },
    /// 工具执行失败
    ExecutionFailed { tool: String, message: String },
    /// 工具执行超时
    Timeout(String),
}

/// 解析错误
#[derive(Debug)]
pub enum ParseError {
    /// 无法解析 Thought
    InvalidThought(String),
    /// 无法解析 Action
    InvalidAction(String),
    /// 无法解析 Action Input
    InvalidActionInput(String),
    /// JSON 解析错误
    JsonError(String),
    /// 输出格式不符合预期
    UnexpectedFormat(String),
}

/// Agent 执行错误
#[derive(Debug)]
pub enum AgentError {
    /// 超过最大迭代次数
    MaxIterationsExceeded(usize),
    /// 没有可用的工具
    NoToolsAvailable,
    /// Agent 初始化失败
    InitializationFailed(String),
    /// 执行被中断
    Interrupted,
    /// 没有响应
    NoResponse,
    /// Token 数量超出限制
    TokenLimitExceeded,
}

/// MCP 相关错误
#[derive(Debug)]
pub enum McpError {
    /// 连接服务端失败
    ConnectionFailed(String),
    /// 初始化握手失败
    InitializationFailed(String),
    /// 协议层错误（JSON-RPC 序列化/反序列化等）
    ProtocolError(String),
    /// 工具调用失败
    ToolCallFailed(String),
    /// 传输层已关闭
    TransportClosed,
}

/// 配置错误
#[derive(Debug)]
pub enum ConfigError {
    /// 环境变量解析失败
    EnvParseError(String),
    /// 缺少必要配置项
    MissingConfig(String, String),
    /// 环境变量格式错误
    EnvFormatError(String),
    /// 无效的配置项
    UnMatchConfigError(String, String),
    /// 不存在该 Model Config
    NotFindModelError(String),
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
            ReactError::Io(e) => write!(f, "IO Error: {}", e),
            ReactError::Other(msg) => write!(f, "Error: {}", msg),
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
        }
    }
}

// ── std::error::Error impls with source() chain ──────────────────────────────

impl std::error::Error for ReactError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ReactError::Llm(e) => Some(e),
            ReactError::Tool(e) => Some(e),
            ReactError::Parse(e) => Some(e),
            ReactError::Agent(e) => Some(e),
            ReactError::Config(e) => Some(e),
            ReactError::Mcp(e) => Some(e),
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

// ── From 转换实现 ─────────────────────────────────────────────────────────────

impl From<std::io::Error> for ReactError {
    fn from(err: std::io::Error) -> Self {
        ReactError::Io(err)
    }
}

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

/// 便捷的 Result 类型别名
pub type Result<T> = std::result::Result<T, ReactError>;
