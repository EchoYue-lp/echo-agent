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
    /// Token数量超出限制
    TokenLimitExceeded,
}

/// 配置错误
#[derive(Debug)]
pub enum ConfigError {
    /// 配置文件未找到
    FileNotFound(String),
    /// 配置解析失败
    ParseFailed(String),
    /// 缺少必需的配置项
    MissingField(String),
    /// 配置值无效
    InvalidValue { field: String, message: String },
}

// 实现 Display trait
impl fmt::Display for ReactError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReactError::Llm(e) => write!(f, "LLM Error: {}", e),
            ReactError::Tool(e) => write!(f, "Tool Error: {}", e),
            ReactError::Parse(e) => write!(f, "Parse Error: {}", e),
            ReactError::Agent(e) => write!(f, "Agent Error: {}", e),
            ReactError::Config(e) => write!(f, "Config Error: {}", e),
            ReactError::Io(e) => write!(f, "IO Error: {}", e),
            ReactError::Other(msg) => write!(f, "Error: {}", msg),
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
            AgentError::TokenLimitExceeded => write!(f, "Token limit from LLM"),
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::FileNotFound(path) => write!(f, "Config file not found: {}", path),
            ConfigError::ParseFailed(msg) => write!(f, "Failed to parse config: {}", msg),
            ConfigError::MissingField(field) => write!(f, "Missing config field: {}", field),
            ConfigError::InvalidValue { field, message } => {
                write!(f, "Invalid config value for '{}': {}", field, message)
            }
        }
    }
}

// 实现 std::error::Error trait
impl std::error::Error for ReactError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ReactError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl std::error::Error for LlmError {}
impl std::error::Error for ToolError {}
impl std::error::Error for ParseError {}
impl std::error::Error for AgentError {}
impl std::error::Error for ConfigError {}

// From 转换实现
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

impl From<serde_yaml::Error> for ReactError {
    fn from(err: serde_yaml::Error) -> Self {
        ReactError::Config(ConfigError::ParseFailed(err.to_string()))
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

impl From<ConfigError> for ReactError {
    fn from(err: ConfigError) -> Self {
        ReactError::Config(err)
    }
}

// 便捷的 Result 类型别名
pub type Result<T> = std::result::Result<T, ReactError>;
