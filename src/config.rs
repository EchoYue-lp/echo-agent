//! 统一配置管理
//!
//! 从 `echo-agent.yaml` 加载全局配置，支持环境变量覆盖。
//!
//! # 配置文件搜索顺序
//!
//! 1. `--config <PATH>` 指定的路径
//! 2. `./echo-agent.yaml`
//! 3. `~/.echo-agent/config.yaml`
//! 4. 无配置文件时使用默认值
//!
//! # 配置优先级
//!
//! CLI 参数 > 环境变量 > 配置文件 > 默认值
//!
//! # 示例
//!
//! ```no_run
//! use echo_agent::config::{load_config, apply_env_overrides};
//! use echo_agent::prelude::*;
//!
//! let mut cfg = load_config(None);
//! apply_env_overrides(&mut cfg);
//! let agent = ReactAgent::new(cfg.to_agent_config());
//! ```

use crate::agent::AgentConfig;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ── 配置结构体 ─────────────────────────────────────────────────────

/// 顶层应用配置
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
#[derive(Default)]
pub struct AppConfig {
    /// 模型配置（名称、温度、最大 token 数）
    pub model: ModelConfig,
    /// Agent 行为配置（系统提示、迭代次数、启用功能）
    pub agent: AgentYamlConfig,
    /// MCP（模型上下文协议）配置
    pub mcp: McpYamlConfig,
    /// 即时通讯通道配置（QQ、飞书）
    pub channels: ChannelsConfig,
    /// 服务端配置（主机、端口）
    pub server: ServerConfig,
    /// 日志级别配置
    pub logging: LoggingConfig,
}

impl AppConfig {
    /// 转换为库的 AgentConfig（builder 风格）
    pub fn to_agent_config(&self) -> AgentConfig {
        AgentConfig::standard(
            &self.model.name,
            &self.agent.name,
            &self.agent.system_prompt,
        )
        .enable_tool(self.agent.enable_tools)
        .enable_memory(self.agent.enable_memory)
        .enable_human_in_loop(self.agent.enable_human_in_loop)
        .max_iterations(self.agent.max_iterations)
        .memory_path(&self.agent.memory_path)
    }
}

/// 模型配置
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct ModelConfig {
    /// 模型名称（如 "qwen-plus", "gpt-4o", "claude-3.5-sonnet"）
    pub name: String,
    /// 最大生成 token 数（None 表示使用模型默认值）
    pub max_tokens: Option<u32>,
    /// 温度参数（0.0 ～ 2.0，None 表示使用模型默认值）
    pub temperature: Option<f32>,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            name: "qwen-plus".to_string(),
            max_tokens: None,
            temperature: None,
        }
    }
}

/// Agent 配置（YAML 映射）
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct AgentYamlConfig {
    /// Agent 名称（用于日志和工具描述）
    pub name: String,
    /// 系统提示词（影响 Agent 行为风格）
    pub system_prompt: String,
    /// 最大迭代次数（单轮对话内最多执行多少步思考）
    pub max_iterations: usize,
    /// 是否启用工具（False 时 Agent 只能进行纯文本对话）
    pub enable_tools: bool,
    /// 是否启用记忆存储（跨会话记忆）
    pub enable_memory: bool,
    /// 是否启用人工审批（敏感操作需要用户确认）
    pub enable_human_in_loop: bool,
    /// 记忆存储路径（SQLite 文件位置）
    pub memory_path: String,
}

impl Default for AgentYamlConfig {
    fn default() -> Self {
        Self {
            name: "echo-assistant".to_string(),
            system_prompt: "你是一个智能助手，可以帮助用户回答问题、执行任务。".to_string(),
            max_iterations: 10,
            enable_tools: true,
            enable_memory: true,
            enable_human_in_loop: true,
            memory_path: "~/.echo-agent/memory".to_string(),
        }
    }
}

/// MCP 配置
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(default)]
pub struct McpYamlConfig {
    /// MCP 配置文件路径（mcp.json）
    pub config_path: Option<String>,
}

/// IM 通道配置
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
#[derive(Default)]
pub struct ChannelsConfig {
    /// QQ 机器人配置
    pub qq: QqChannelConfig,
    /// 飞书机器人配置
    pub feishu: FeishuChannelConfig,
    /// 会话管理配置
    pub session: SessionYamlConfig,
}

/// QQ Bot 通道配置
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
#[derive(Default)]
pub struct QqChannelConfig {
    /// 是否启用 QQ 通道
    pub enabled: bool,
    /// QQ 开发者平台 App ID
    pub app_id: String,
    /// QQ 开发者平台 Client Secret
    pub client_secret: String,
}

/// 飞书通道配置
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct FeishuChannelConfig {
    /// 是否启用飞书通道
    pub enabled: bool,
    /// 飞书开发者平台 App ID
    pub app_id: String,
    /// 飞书开发者平台 App Secret
    pub app_secret: String,
    /// 连接模式: "long_poll"（长轮询）或 "webhook"（Webhook 回调）
    pub mode: String,
}

impl Default for FeishuChannelConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            app_id: String::new(),
            app_secret: String::new(),
            mode: "long_poll".to_string(),
        }
    }
}

/// IM 会话配置
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct SessionYamlConfig {
    /// 会话超时时间（分钟），超过此时间后会话自动重置
    pub timeout_minutes: u64,
    /// 触发会话重置的关键词列表（用户消息包含这些词时会重置会话）
    pub reset_keywords: Vec<String>,
    /// 触发会话重置的命令列表（用户消息以这些命令开头时会重置会话）
    pub reset_commands: Vec<String>,
}

impl Default for SessionYamlConfig {
    fn default() -> Self {
        Self {
            timeout_minutes: 60,
            reset_keywords: vec![
                "重置对话".to_string(),
                "新对话".to_string(),
                "清除记忆".to_string(),
            ],
            reset_commands: vec![
                "/reset".to_string(),
                "/clear".to_string(),
                "/new".to_string(),
            ],
        }
    }
}

/// 服务配置
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct ServerConfig {
    /// 监听主机（如 "0.0.0.0" 或 "127.0.0.1"）
    pub host: String,
    /// 监听端口
    pub port: u16,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".to_string(),
            port: 3000,
        }
    }
}

/// 日志配置
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct LoggingConfig {
    /// 日志级别（"debug", "info", "warn", "error"）
    pub level: String,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
        }
    }
}

// ── 配置加载 ────────────────────────────────────────────────────────

/// 配置文件搜索路径
fn config_search_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    paths.push(PathBuf::from("echo-agent.yaml"));
    if let Ok(home) = std::env::var("HOME") {
        paths.push(PathBuf::from(home).join(".echo-agent").join("config.yaml"));
    }
    paths
}

fn load_from_file(path: &PathBuf) -> Result<AppConfig, String> {
    let content = std::fs::read_to_string(path).map_err(|e| format!("读取配置文件失败: {}", e))?;
    serde_yaml::from_str(&content).map_err(|e| format!("解析配置文件失败: {}", e))
}

/// 加载配置（搜索默认路径）
pub fn load_config(explicit_path: Option<&str>) -> AppConfig {
    if let Some(path_str) = explicit_path {
        let path = PathBuf::from(path_str);
        match load_from_file(&path) {
            Ok(config) => {
                tracing::info!("已加载配置: {}", path.display());
                return config;
            }
            Err(e) => {
                tracing::error!("加载配置文件 {} 失败: {}", path.display(), e);
                tracing::info!("使用默认配置");
                return AppConfig::default();
            }
        }
    }

    for path in config_search_paths() {
        if path.exists() {
            match load_from_file(&path) {
                Ok(config) => {
                    tracing::info!("已加载配置: {}", path.display());
                    return config;
                }
                Err(e) => {
                    tracing::warn!("加载配置文件 {} 失败: {}", path.display(), e);
                }
            }
        }
    }

    tracing::info!("未找到配置文件，使用默认配置");
    AppConfig::default()
}

/// 合并环境变量覆盖
///
/// 环境变量优先级高于配置文件，但低于 CLI 参数。
pub fn apply_env_overrides(config: &mut AppConfig) {
    if let Ok(v) = std::env::var("MODEL_NAME") {
        config.model.name = v;
    }
    if let Ok(v) = std::env::var("QQ_APP_ID") {
        config.channels.qq.app_id = v;
        if !config.channels.qq.app_id.is_empty() {
            config.channels.qq.enabled = true;
        }
    }
    if let Ok(v) = std::env::var("QQ_CLIENT_SECRET") {
        config.channels.qq.client_secret = v;
    }
    if let Ok(v) = std::env::var("FEISHU_APP_ID") {
        config.channels.feishu.app_id = v;
        if !config.channels.feishu.app_id.is_empty() {
            config.channels.feishu.enabled = true;
        }
    }
    if let Ok(v) = std::env::var("FEISHU_APP_SECRET") {
        config.channels.feishu.app_secret = v;
    }
    if let Ok(v) = std::env::var("MCP_CONFIG_PATH") {
        config.mcp.config_path = Some(v);
    }
}

// ── 单元测试 ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = AppConfig::default();
        assert_eq!(config.model.name, "qwen-plus");
        assert_eq!(config.agent.name, "echo-assistant");
        assert_eq!(config.agent.max_iterations, 10);
        assert!(config.agent.enable_tools);
        assert!(config.agent.enable_memory);
        assert!(config.agent.enable_human_in_loop);
        assert!(!config.channels.qq.enabled);
        assert!(!config.channels.feishu.enabled);
        assert_eq!(config.server.port, 3000);
        assert_eq!(config.logging.level, "info");
    }

    #[test]
    fn test_to_agent_config() {
        let config = AppConfig::default();
        let agent_config = config.to_agent_config();
        assert_eq!(agent_config.get_model_name(), "qwen-plus");
        assert_eq!(agent_config.get_agent_name(), "echo-assistant");
        assert!(agent_config.is_tool_enabled());
        assert!(agent_config.is_memory_enabled());
        assert!(agent_config.is_human_in_loop_enabled());
        assert_eq!(agent_config.get_max_iterations(), 10);
    }

    #[test]
    fn test_load_config_no_file() {
        // Should return defaults when no file exists
        let config = load_config(None);
        assert_eq!(config.model.name, "qwen-plus");
    }

    #[test]
    fn test_yaml_roundtrip() {
        let config = AppConfig::default();
        let yaml = serde_yaml::to_string(&config).unwrap();
        let parsed: AppConfig = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed.model.name, config.model.name);
        assert_eq!(parsed.agent.system_prompt, config.agent.system_prompt);
    }
}
