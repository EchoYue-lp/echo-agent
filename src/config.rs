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
use crate::skills::hooks::HooksDefinition;
use echo_core::budget::TokenBudgetConfig;
use echo_core::llm::capabilities::infer_context_window;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub const DEFAULT_AGENT_SYSTEM_PROMPT: &str = r#"You are Echo Agent, a local AI workbench agent for real coding, research, data analysis, and long-running task execution on the user's machine.

Core operating model:
- Establish facts from the available context before making claims. Read relevant files, configs, logs, tests, data, papers, or prior instructions when they can change the answer.
- Use tools when they can verify, inspect, execute, or make progress. Do not pretend tool output exists.
- Preserve user work. Do not overwrite unrelated changes or clean up unrelated files.
- Prefer root-cause fixes over cosmetic workarounds. Keep changes focused and consistent with the existing system.
- Validate changes with the most relevant checks. If validation cannot be run, state the reason and remaining risk.
- For broad read-only analysis, architecture review, codebase review, literature exploration, evidence review, or data profiling, decompose the work and use the runtime/subagent capability when available instead of doing everything serially.
- Keep read-only workers read-only. Mutating edits, shell commands with side effects, installs, network access, and external operations must follow the active approval and execution mode.
- Treat dynamic memories, hook context, task state, and tool results as per-turn context, not stable policy. Stable policy belongs in the system prompt; volatile context should not rewrite the agent identity.
- For research, data, medical, financial, legal, software-version, or other time-sensitive/high-stakes topics, verify with available primary or current sources before presenting precise claims.
- For medical content, distinguish evidence quality, applicability, uncertainty, contraindications, and safety boundaries; do not provide personal diagnosis or treatment decisions.

Response style:
- Use the user's language.
- For reviews, lead with findings and concrete file/line evidence.
- For implementation, summarize what changed, what was verified, and what risk remains.
- Be concise, but do not hide important uncertainty."#;

// ── Config structs ────────────────────────────────────────────────────

/// Top-level application configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
#[derive(Default)]
pub struct AppConfig {
    /// Model configuration (name, temperature, max_tokens).
    pub model: ModelConfig,
    /// Provider-level credentials and endpoint overrides configured from GUI.
    pub model_providers: BTreeMap<String, ModelProviderConfig>,
    /// User-configured models shown in GUI model switchers.
    pub configured_models: Vec<ConfiguredModel>,
    /// Agent behaviour configuration (system prompt, iterations, feature toggles).
    pub agent: AgentYamlConfig,
    /// MCP (Model Context Protocol) configuration.
    pub mcp: McpYamlConfig,
    /// IM channel configuration (QQ, Feishu).
    pub channels: ChannelsConfig,
    /// Webhook event callback configuration.
    pub webhooks: WebhooksConfig,
    /// User-defined lifecycle hooks configuration.
    pub hooks: HooksDefinition,
    /// Server configuration (host, port).
    pub server: ServerConfig,
    /// Logging level configuration.
    pub logging: LoggingConfig,
    /// TUI (Terminal User Interface) configuration.
    pub tui: TuiConfig,
}

/// 解析最终的上下文窗口值。
/// 优先级：用户显式设置 > 名称模式推断 > 默认 128K
fn resolve_context_window(explicit: Option<u32>, provider: &str, model_name: &str) -> usize {
    explicit
        .or_else(|| infer_context_window(provider, model_name))
        .unwrap_or(128_000)
        .clamp(1, 10_000_000) as usize
}

impl AppConfig {
    /// Convert to the library's `AgentConfig` (builder style).
    ///
    /// Note: compressor is NOT set here because `set_compressor` is async.
    /// Callers should use `apply_compressor` after constructing the agent.
    pub fn to_agent_config(&self) -> AgentConfig {
        let context_window = resolve_context_window(
            self.model.context_window,
            &self.model.provider,
            &self.model.name,
        );
        // 三态语义：显式 token_limit > 0 时用 token_limit；
        // 否则若用户显式设了 context_window，用它做 token_limit；
        // 两者都没配时保持 usize::MAX（旧行为：不启用压缩/budget）。
        let token_limit = if self.agent.token_limit > 0 {
            self.agent.token_limit
        } else if self.model.context_window.is_some() {
            context_window
        } else {
            usize::MAX
        };
        // TokenBudget 只在显式配置了 context_window 时设置 total_window，
        // 否则保持 Default（None → 走自动检测），避免强制启用 budget。
        let token_budget_config =
            if self.model.context_window.is_some() || self.agent.token_limit > 0 {
                TokenBudgetConfig {
                    total_window: Some(context_window),
                    ..Default::default()
                }
            } else {
                TokenBudgetConfig::default()
            };

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
        .token_limit(token_limit)
        .token_budget(token_budget_config)
        .tool_execution(crate::tools::ToolExecutionConfig {
            timeout_ms: self.agent.tool_timeout_ms,
            ..Default::default()
        })
    }

    /// Whether auto-compression is configured.
    ///
    /// Matches `apply_compressor` predicate: a compressor is active when any of
    /// `token_limit > 0` (explicit), `context_window` is set (inferred limit), or
    /// `compress_strategy` is non-empty (stage4 P4.3: default "summary" turns
    /// compression on out-of-the-box without requiring an explicit token_limit).
    pub fn has_compressor(&self) -> bool {
        self.agent.token_limit > 0
            || self.model.context_window.is_some()
            || !self.agent.compress_strategy.is_empty()
    }

    /// Apply the configured compressor to the agent (stage4 P4.3).
    ///
    /// Strategies: "summary" (SummaryCompressor — LLM summary, default; falls
    /// back to SlidingWindow on LLM failure or when no LLM client is configured),
    /// "sliding" (SlidingWindowCompressor), "adaptive" (AdaptiveCompressor).
    pub async fn apply_compressor(&self, agent: &crate::agent::ReactAgent) {
        // Compression is on when token_limit/context_window is set OR a strategy
        // is explicitly chosen (default "summary"). The agent's resolved
        // token_limit (from create_agent) drives the actual trigger threshold.
        let context_window = resolve_context_window(
            self.model.context_window,
            &self.model.provider,
            &self.model.name,
        );
        let should_compress = self.agent.token_limit > 0
            || self.model.context_window.is_some()
            || !self.agent.compress_strategy.is_empty();
        if !should_compress {
            return;
        }
        use crate::compression::compressor::SlidingWindowCompressor;
        let window = self.agent.compress_window.max(2);
        match self.agent.compress_strategy.as_str() {
            "summary" => {
                use crate::compression::compressor::SummaryCompressor;
                match agent.llm_client().cloned() {
                    Some(llm) => {
                        agent
                            .set_compressor(SummaryCompressor::new(llm, window))
                            .await;
                        tracing::info!(
                            "Compressor: SummaryCompressor (stage4 P4.3 default; falls back to SlidingWindow on LLM failure)"
                        );
                    }
                    None => {
                        tracing::warn!(
                            "compress_strategy=summary but agent has no LLM client — \
                             falling back to SlidingWindow"
                        );
                        agent
                            .set_compressor(SlidingWindowCompressor::new(window))
                            .await;
                    }
                }
            }
            "sliding" | "" => {
                agent
                    .set_compressor(SlidingWindowCompressor::new(window))
                    .await;
            }
            "adaptive" => {
                use crate::compression::levels::{
                    AdaptiveCompressionConfig, AdaptiveCompressor, tune_for_model,
                };
                let mut config = AdaptiveCompressionConfig::default();
                // Auto-tune thresholds from the resolved context_window (not
                // the raw YAML token_limit which may be 0/unset).
                if context_window < usize::MAX {
                    tune_for_model(&mut config, context_window);
                    tracing::info!(
                        context_window = context_window,
                        "Tuned adaptive compression from context window"
                    );
                }
                agent.set_compressor(AdaptiveCompressor::new(config)).await;
            }
            other => {
                tracing::warn!(
                    strategy = other,
                    "Unknown strategy; falling back to sliding. \
                     Supported: summary, sliding, adaptive"
                );
                agent
                    .set_compressor(SlidingWindowCompressor::new(window))
                    .await;
            }
        }
    }
}

/// Model configuration.
///
/// The default provider is `deepseek` with model `deepseek-v4-flash`.
/// Users can specify other providers (openai, anthropic, qwen, etc.) and their models.
///
/// Authentication and base URL can be set via:
/// - Config file: auth_token and base_url fields (highest priority)
/// - Struct fields (the application layer is responsible for filling these
///   from its own config file / env vars before constructing a `ModelConfig`).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct ModelConfig {
    /// Default configured model id used by GUI/runtime switchers.
    pub default_model_id: Option<String>,
    /// Model provider (e.g. "deepseek", "openai", "anthropic", "qwen").
    pub provider: String,
    /// Model name (e.g. "deepseek-v4-flash", "gpt-5.5", "claude-3.5-sonnet").
    pub name: String,
    /// API authentication token (optional; the application layer is responsible
    /// for populating this from its own config/env before constructing the struct).
    pub auth_token: Option<String>,
    /// API base URL (optional; populated by the application layer).
    pub base_url: Option<String>,
    /// Maximum tokens to generate (None means use model default).
    pub max_tokens: Option<u32>,
    /// Temperature parameter (0.0–2.0, None means use model default).
    pub temperature: Option<f32>,
    /// Optional model context window size in tokens.
    /// When None, falls back to name-based inference.
    pub context_window: Option<u32>,
    /// 思考深度 / reasoning-depth 控制(可选)。
    ///
    /// 用户可设置:`"auto"`/`""`(默认)、`"disabled"`、`"minimal"`/`"low"`/
    /// `"medium"`/`"high"`、或裸数字(精确 token 预算,主要给 Claude)。
    /// 运行时翻译成 `ThinkingConfig` 注入到 agent 的每个 ChatRequest。
    /// 不支持的模型静默忽略(见 `ModelProfile.thinking_protocol`)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            default_model_id: None,
            provider: "deepseek".to_string(),
            name: "deepseek-v4-flash".to_string(),
            auth_token: None,
            base_url: None,
            max_tokens: None,
            temperature: None,
            context_window: None,
            thinking: None,
        }
    }
}

impl ModelConfig {
    /// Get the effective authentication token.
    ///
    /// Reads only the struct field. Environment-variable fallback is the
    /// application layer's responsibility (it constructs `ModelConfig` from
    /// its own config + env before passing it to the framework). The framework
    /// stays neutral and does not hardcode any env-var name.
    pub fn get_auth_token(&self) -> Option<String> {
        self.auth_token.clone().filter(|s| !s.is_empty())
    }

    /// Get the effective base URL (struct field only; env fallback is app-layer).
    pub fn get_base_url(&self) -> Option<String> {
        self.base_url.clone().filter(|s| !s.is_empty())
    }

    /// Get the effective model name (struct field only; env fallback is app-layer).
    pub fn get_model_name(&self) -> String {
        self.name.clone()
    }
}

/// Provider-level credentials configured through GUI.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct ModelProviderConfig {
    /// API authentication token for this provider.
    pub auth_token: Option<String>,
    /// Base URL override for this provider.
    pub base_url: Option<String>,
}

/// A user-configured model entry available in GUI/TUI switchers.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct ConfiguredModel {
    /// Stable model id, unique within the config.
    pub id: String,
    /// Human-friendly display name.
    pub display_name: String,
    /// Provider id.
    pub provider: String,
    /// Provider model name.
    pub model: String,
    /// Whether this model should be shown in quick switchers.
    pub enabled: bool,
    /// Optional model-specific maximum output tokens.
    pub max_tokens: Option<u32>,
    /// Optional model-specific temperature.
    pub temperature: Option<f32>,
    /// Optional model context window size in tokens.
    /// When set, overrides the auto-detected value for compression threshold,
    /// TokenBudget allocation, and adaptive compression tuning.
    /// When None, falls back to name-based inference.
    pub context_window: Option<u32>,
    /// Optional thinking-depth / reasoning control for this model.
    /// See [`ModelConfig::thinking`] for accepted values. Forwarded to the
    /// agent at runtime via `agent.set_thinking()`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
}

impl Default for ConfiguredModel {
    fn default() -> Self {
        Self {
            id: String::new(),
            display_name: String::new(),
            provider: String::new(),
            model: String::new(),
            enabled: true,
            max_tokens: None,
            temperature: None,
            context_window: None,
            thinking: None,
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
    /// Maximum iterations (max reasoning steps per conversation turn). 0 means unlimited.
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
    /// Token limit for context auto-compression. When the estimated token count
    /// exceeds this limit, the configured compressor is triggered automatically.
    /// Set to 0 to disable auto-compression (default: 0, meaning no limit).
    pub token_limit: usize,
    /// Context compression strategy: "summary" (SummaryCompressor, default —
    /// LLM summary, falls back to SlidingWindow on LLM failure), "sliding"
    /// (SlidingWindowCompressor), "adaptive" (AdaptiveCompressor).
    /// Effective whenever `has_compressor()` is true (token_limit>0,
    /// context_window set, or this field is non-empty).
    pub compress_strategy: String,
    /// Window size / keep-recent count for SlidingWindowCompressor / SummaryCompressor
    /// (number of recent messages to keep uncompressed). Default: 20.
    pub compress_window: usize,
}

impl Default for AgentYamlConfig {
    fn default() -> Self {
        Self {
            name: "echo-assistant".to_string(),
            system_prompt: DEFAULT_AGENT_SYSTEM_PROMPT.to_string(),
            max_iterations: 0,
            enable_tools: true,
            enable_memory: true,
            enable_human_in_loop: true,
            memory_path: "~/.echo-agent/memory".to_string(),
            tool_timeout_ms: 120_000,
            token_limit: 0,
            // (stage4 P4.3) Default to SummaryCompressor — durable facts are
            // flushed by pre_compaction_flush (E1) before summary compression
            // runs, so summarizing old messages is safe.
            compress_strategy: "summary".to_string(),
            compress_window: 20,
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
    /// Webhook bind address (only used when mode = "webhook").
    pub webhook_bind: String,
    /// Webhook path (only used when mode = "webhook").
    pub webhook_path: String,
    /// Webhook verification token (optional, only used when mode = "webhook").
    pub webhook_verification_token: Option<String>,
}

impl Default for FeishuChannelConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            app_id: String::new(),
            app_secret: String::new(),
            mode: "long_poll".to_string(),
            webhook_bind: "0.0.0.0:9000".to_string(),
            webhook_path: "/webhook/feishu".to_string(),
            webhook_verification_token: None,
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

/// TUI (Terminal User Interface) configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct TuiConfig {
    /// Maximum total characters of chat messages to keep in the TUI display.
    /// Oldest messages (excluding the welcome message) are trimmed when exceeded.
    pub max_display_chars: usize,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            max_display_chars: 20_000,
        }
    }
}

/// Webhook configuration.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(default)]
pub struct WebhooksConfig {
    /// List of webhook endpoints.
    pub endpoints: Vec<WebhookEntryConfig>,
}

/// Single webhook endpoint configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WebhookEntryConfig {
    /// Callback URL.
    pub url: String,
    /// Event types to subscribe (empty = all).
    #[serde(default)]
    pub events: Vec<String>,
    /// HMAC-SHA256 signing secret (optional).
    #[serde(default)]
    pub secret: Option<String>,
}

// ── Config loading ───────────────────────────────────────────────────

/// Config file search paths.
pub fn config_search_paths() -> Vec<PathBuf> {
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
    serde_yaml_ng::from_str(&content).map_err(|e| format!("Failed to parse config file: {}", e))
}

/// Persist an [`AppConfig`] back to the first writable config file.
///
/// Search order: `$ECHO_AGENT_CONFIG` → `./echo-agent.yaml` → `~/.echo-agent/config.yaml`.
/// The first existing file (or first path if none exist) is overwritten.
pub fn save_config(config: &AppConfig) -> std::result::Result<(), String> {
    let search = config_search_paths();
    // Prefer an already-existing file; otherwise use the first path (./echo-agent.yaml)
    let target = search.iter().find(|p| p.exists()).unwrap_or(&search[1]);
    let yaml =
        serde_yaml_ng::to_string(config).map_err(|e| format!("serialization failed: {e}"))?;
    let header = "# Echo Agent Configuration\n# Auto-saved via Web API or CLI\n\n";
    let content = format!("{header}{yaml}");
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create directory failed: {e}"))?;
    }
    std::fs::write(target, content).map_err(|e| format!("write failed: {e}"))?;
    // P1-4: the config file holds plaintext secrets (channel app_secret /
    // client_secret, and is the on-disk source for MCP env/headers tokens).
    // Restrict it to owner-only (0600) so other users on the host can't read
    // credentials. (Full at-rest encryption would need an OS keychain to store
    // the key; file permissions are the standard local-app mitigation and the
    // scope of this fix.)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = std::fs::set_permissions(target, std::fs::Permissions::from_mode(0o600)) {
            tracing::warn!("Failed to set config file permissions to 0600: {e}");
        }
    }
    Ok(())
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

    /// RAII guard that sets an env var and restores (or removes) it on drop.
    struct EnvGuard {
        key: &'static str,
        old: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &'static str, val: &str) -> Self {
            let old = std::env::var(key).ok();
            // SAFETY: test code, single-threaded via cargo test harness
            unsafe { std::env::set_var(key, val) };
            Self { key, old }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.old {
                Some(v) => unsafe { std::env::set_var(self.key, v) },
                None => unsafe { std::env::remove_var(self.key) },
            }
        }
    }

    #[test]
    fn test_default_config() {
        let config = AppConfig::default();
        assert_eq!(config.model.provider, "deepseek");
        assert_eq!(config.model.name, "deepseek-v4-flash");
        assert_eq!(config.agent.name, "echo-assistant");
        assert_eq!(config.agent.max_iterations, 0);
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
        assert_eq!(agent_config.get_model_name(), "deepseek-v4-flash");
        assert_eq!(agent_config.get_agent_name(), "echo-assistant");
        assert!(agent_config.is_tool_enabled());
        assert!(agent_config.is_memory_enabled());
        assert!(agent_config.is_human_in_loop_enabled());
        assert_eq!(agent_config.get_max_iterations(), 0);
    }

    #[test]
    fn test_model_config_getters_return_fields_only() {
        // The framework does NOT read env vars for LLM credentials — that is
        // the application layer's job. Getters return only struct fields.
        // (Set an unrelated env var to prove the getter ignores env entirely.)
        let _guard = EnvGuard::set("SOME_UNRELATED_AUTH_TOKEN", "env-token");

        let config = ModelConfig {
            auth_token: Some("config-token".to_string()),
            base_url: Some("https://config.example/v1".to_string()),
            name: "deepseek-v4-flash".to_string(),
            ..Default::default()
        };

        assert_eq!(config.get_auth_token().as_deref(), Some("config-token"));
        assert_eq!(
            config.get_base_url().as_deref(),
            Some("https://config.example/v1")
        );
        assert_eq!(config.get_model_name(), "deepseek-v4-flash");

        // Empty fields return None / default — no env fallback.
        let empty = ModelConfig::default();
        assert_eq!(empty.get_auth_token(), None);
        assert_eq!(empty.get_base_url(), None);
    }

    #[test]
    fn test_load_config_no_file() {
        // An explicit but missing config path should fall back to defaults.
        let missing_path =
            std::env::temp_dir().join(format!("echo-agent-missing-config-{}.yaml", uuid()));
        let config = load_config(missing_path.to_str());
        assert_eq!(config.model.provider, "deepseek");
        assert_eq!(config.model.name, "deepseek-v4-flash");
    }

    #[test]
    fn test_yaml_roundtrip() {
        let config = AppConfig::default();
        let yaml = serde_yaml_ng::to_string(&config).unwrap();
        let parsed: AppConfig = serde_yaml_ng::from_str(&yaml).unwrap();
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
  name: qwen3.7-plus
"#,
        )
        .unwrap();

        let _guard = EnvGuard::set("ECHO_AGENT_CONFIG", temp_path.to_str().unwrap());
        let config = load_config(None);
        drop(_guard);
        std::fs::remove_file(&temp_path).unwrap();

        assert_eq!(config.model.name, "qwen3.7-plus");
    }

    fn uuid() -> u128 {
        use std::time::{SystemTime, UNIX_EPOCH};

        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    }
}
