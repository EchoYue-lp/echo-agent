//! Unified configuration management.
//!
//! Loads global configuration from `echo-agent.yaml`.
//!
//! # Config File Search Order
//!
//! 1. `--config <PATH>` (explicit path)
//! 2. `./echo-agent.yaml` (current directory)
//! 3. `~/.echo-agent/config.yaml` (user home)
//! 4. Built-in defaults (no file required)
//!
//! # Quick Start
//!
//! ```rust,no_run
//! use echo_agent::config::{load_config, apply_env_overrides};
//! use echo_agent::prelude::*;
//!
//! # fn main() -> echo_agent::error::Result<()> {
//! let mut cfg = load_config(None);
//! apply_env_overrides(&mut cfg);
//! let config = cfg.to_agent_config();
//! let agent = ReactAgentBuilder::new()
//!     .model(&cfg.model.name)
//!     .system_prompt(&cfg.agent.system_prompt)
//!     .build()?;
//! # Ok(())
//! # }
//! ```
//!
//! # Key Types
//!
//! | Type | Description |
//! |------|-------------|
//! | [`AppConfig`] | Top-level config struct (model, agent, mcp, channels, server, logging) |
//! | [`ModelConfig`] | Model name, temperature, max_tokens |
//! | [`AgentYamlConfig`] | System prompt, iterations, feature toggles |

use crate::agent::AgentConfig;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ── Config structs ────────────────────────────────────────────────────

/// Top-level application configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
#[derive(Default)]
pub struct AppConfig {
    /// Model configuration (name, temperature, max_tokens).
    pub model: ModelConfig,
    /// Agent behaviour configuration (system prompt, iterations, feature toggles).
    pub agent: AgentYamlConfig,
    /// MCP (Model Context Protocol) configuration.
    pub mcp: McpYamlConfig,
    /// IM channel configuration (QQ, Feishu).
    pub channels: ChannelsConfig,
    /// Server configuration (host, port).
    pub server: ServerConfig,
    /// Logging level configuration.
    pub logging: LoggingConfig,
}

impl AppConfig {
    /// Convert to the library's `AgentConfig` (builder style).
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
        .temperature(self.model.temperature)
        .max_tokens(self.model.max_tokens)
        .tool_execution(crate::tools::ToolExecutionConfig {
            timeout_ms: self.agent.tool_timeout_ms,
            ..Default::default()
        })
    }
}

/// Model configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct ModelConfig {
    /// Model name (e.g. "qwen-plus", "gpt-4o", "claude-3.5-sonnet").
    pub name: String,
    /// Maximum tokens to generate (None means use model default).
    pub max_tokens: Option<u32>,
    /// Temperature parameter (0.0–2.0, None means use model default).
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

/// Agent configuration (YAML mapping).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct AgentYamlConfig {
    /// Agent name (used in logs and tool descriptions).
    pub name: String,
    /// System prompt (shapes agent behaviour and tone).
    pub system_prompt: String,
    /// Maximum iterations (max reasoning steps per conversation turn).
    pub max_iterations: usize,
    /// Enable tools (when false, agent does text-only conversation).
    pub enable_tools: bool,
    /// Enable memory storage (cross-session memory).
    pub enable_memory: bool,
    /// Enable human-in-the-loop approval (user confirmation for sensitive actions).
    pub enable_human_in_loop: bool,
    /// Memory storage path.
    pub memory_path: String,
    /// Tool execution timeout in milliseconds (default 120_000 = 2 min), used for MCP tools and other long-running calls.
    pub tool_timeout_ms: u64,
}

impl Default for AgentYamlConfig {
    fn default() -> Self {
        Self {
            name: "echo-assistant".to_string(),
            system_prompt: "You are an intelligent assistant that helps users answer questions and complete tasks.".to_string(),
            max_iterations: 10,
            enable_tools: true,
            enable_memory: true,
            enable_human_in_loop: true,
            memory_path: "~/.echo-agent/memory".to_string(),
            tool_timeout_ms: 120_000,
        }
    }
}

/// MCP configuration.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(default)]
pub struct McpYamlConfig {
    /// Path to the MCP configuration file (mcp.json).
    pub config_path: Option<String>,
}

/// IM channel configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
#[derive(Default)]
pub struct ChannelsConfig {
    /// QQ Bot configuration.
    pub qq: QqChannelConfig,
    /// Feishu Bot configuration.
    pub feishu: FeishuChannelConfig,
    /// Session management configuration.
    pub session: SessionYamlConfig,
}

/// QQ Bot channel configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
#[derive(Default)]
pub struct QqChannelConfig {
    /// Whether the QQ channel is enabled.
    pub enabled: bool,
    /// QQ Developer Platform App ID.
    pub app_id: String,
    /// QQ Developer Platform Client Secret.
    pub client_secret: String,
}

/// Feishu channel configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct FeishuChannelConfig {
    /// Whether the Feishu channel is enabled.
    pub enabled: bool,
    /// Feishu Developer Platform App ID.
    pub app_id: String,
    /// Feishu Developer Platform App Secret.
    pub app_secret: String,
    /// Connection mode: "long_poll" or "webhook".
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

/// IM session configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct SessionYamlConfig {
    /// Session timeout in minutes; the session auto-resets after this duration.
    pub timeout_minutes: u64,
    /// Keywords that trigger a session reset when present in the user message.
    pub reset_keywords: Vec<String>,
    /// Commands that trigger a session reset when the user message starts with one.
    pub reset_commands: Vec<String>,
}

impl Default for SessionYamlConfig {
    fn default() -> Self {
        Self {
            timeout_minutes: 60,
            reset_keywords: vec![
                "reset conversation".to_string(),
                "new conversation".to_string(),
                "clear memory".to_string(),
            ],
            reset_commands: vec![
                "/reset".to_string(),
                "/clear".to_string(),
                "/new".to_string(),
            ],
        }
    }
}

/// Server configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct ServerConfig {
    /// Host to listen on (e.g. "0.0.0.0" or "127.0.0.1").
    pub host: String,
    /// Port to listen on.
    pub port: u16,
    /// Maximum request body size in bytes (default 1 MB, for A2A HTTP service).
    pub max_body_bytes: usize,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".to_string(),
            port: 3000,
            max_body_bytes: 1024 * 1024, // 1MB
        }
    }
}

/// Logging configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct LoggingConfig {
    /// Log level ("debug", "info", "warn", "error").
    pub level: String,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
        }
    }
}

// ── Config loading ───────────────────────────────────────────────────

/// Config file search paths.
fn config_search_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(explicit) = std::env::var("ECHO_AGENT_CONFIG")
        && !explicit.trim().is_empty()
    {
        paths.push(PathBuf::from(explicit));
    }
    paths.push(PathBuf::from("echo-agent.yaml"));
    if let Ok(home) = std::env::var("HOME") {
        paths.push(PathBuf::from(home).join(".echo-agent").join("config.yaml"));
    }
    paths
}

fn load_from_file(path: &PathBuf) -> Result<AppConfig, String> {
    let content =
        std::fs::read_to_string(path).map_err(|e| format!("Failed to read config file: {}", e))?;
    serde_yaml::from_str(&content).map_err(|e| format!("Failed to parse config file: {}", e))
}

/// Load configuration (searches default paths).
pub fn load_config(explicit_path: Option<&str>) -> AppConfig {
    if let Some(path_str) = explicit_path {
        let path = PathBuf::from(path_str);
        match load_from_file(&path) {
            Ok(config) => {
                tracing::info!("Config loaded: {}", path.display());
                return config;
            }
            Err(e) => {
                tracing::error!("Failed to load config {}: {}", path.display(), e);
                tracing::info!("Using default config");
                return AppConfig::default();
            }
        }
    }

    for path in config_search_paths() {
        if path.exists() {
            match load_from_file(&path) {
                Ok(config) => {
                    tracing::info!("Config loaded: {}", path.display());
                    return config;
                }
                Err(e) => {
                    tracing::warn!("Failed to load config {}: {}", path.display(), e);
                }
            }
        }
    }

    tracing::info!("No config file found, using defaults");
    AppConfig::default()
}

/// Apply basic environment variable overrides.
///
/// Only covers config file path, channel keys, and other bootstrap items;
/// model selection must come from YAML.
pub fn apply_env_overrides(config: &mut AppConfig) {
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

// ── Unit tests ───────────────────────────────────────────────────────

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
        // An explicit but missing config path should fall back to defaults.
        let missing_path =
            std::env::temp_dir().join(format!("echo-agent-missing-config-{}.yaml", uuid()));
        let config = load_config(missing_path.to_str());
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

    #[test]
    fn test_load_config_honors_echo_agent_config_env() {
        let temp_path =
            std::env::temp_dir().join(format!("echo-agent-config-{}.yaml", std::process::id()));
        std::fs::write(
            &temp_path,
            r#"
model:
  name: qwen-vl-max
"#,
        )
        .unwrap();

        unsafe { std::env::set_var("ECHO_AGENT_CONFIG", &temp_path) };
        let config = load_config(None);
        unsafe { std::env::remove_var("ECHO_AGENT_CONFIG") };
        std::fs::remove_file(&temp_path).unwrap();

        assert_eq!(config.model.name, "qwen-vl-max");
    }

    fn uuid() -> u128 {
        use std::time::{SystemTime, UNIX_EPOCH};

        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    }
}
