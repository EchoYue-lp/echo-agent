//! LLM 配置加载
//!
//! 支持两种配置方式（按优先级）：
//!
//! ## 1. YAML 配置文件（推荐）
//!
//! 查找顺序：`$ECHO_AGENT_CONFIG` → `./echo-agent.yaml` → `~/.echo-agent/config.yaml`
//!
//! ```yaml
//! models:
//!   qwen3-max:
//!     base_url: https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions
//!     api_key: sk-xxx
//!
//!   deepseek-chat:
//!     base_url: https://api.deepseek.com/chat/completions
//!     api_key: ${DS_API_KEY}   # 支持引用环境变量
//!
//!   gpt-4o:
//!     provider: openai          # 内置 Provider 快捷方式
//!     api_key: ${OPENAI_API_KEY}
//! ```
//!
//! ## 2. 环境变量（兼容旧格式）
//!
//! ```text
//! AGENT_MODEL_<ID>_MODEL=gpt-4o
//! AGENT_MODEL_<ID>_BASEURL=https://api.openai.com/v1/chat/completions
//! AGENT_MODEL_<ID>_APIKEY=sk-...
//! ```

use crate::error::{ConfigError, ReactError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

// ── 公共类型 ─────────────────────────────────────────────────────────────────

/// LLM 供应商类型
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub enum LlmProvider {
    /// OpenAI 兼容 API（默认，适用于 OpenAI、DashScope、DeepSeek 等）
    #[default]
    OpenAi,
    /// Anthropic Messages API
    Anthropic,
    /// Ollama 本地推理
    Ollama,
}

/// LLM 运行时配置（依赖注入模式）
///
/// 可以直接创建并注入到 Agent，无需配置文件或环境变量。
///
/// # 示例
///
/// ```rust
/// use echo_agent::llm::LlmConfig;
///
/// // 方式一：直接指定
/// let config = LlmConfig::new(
///     "https://api.openai.com/v1/chat/completions",
///     "sk-...",
///     "gpt-4o",
/// );
///
/// // 方式二：使用 Provider 快捷方式
/// let config = LlmConfig::openai("sk-...", "gpt-4o");
///
/// // 方式三：Anthropic
/// let config = LlmConfig::anthropic("sk-ant-...", "claude-sonnet-4-6");
///
/// // 方式四：Ollama 本地
/// let config = LlmConfig::ollama("llama3");
///
/// // 方式五：从配置文件/环境变量加载
/// // let config = LlmConfig::from_model("qwen3-max").unwrap();
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    /// LLM 供应商
    #[serde(default)]
    pub provider: LlmProvider,
    /// Chat Completions 接口完整 URL
    pub base_url: String,
    /// API 密钥
    pub api_key: String,
    /// 模型名称
    pub model: String,
}

impl LlmConfig {
    /// 创建新的 LLM 配置（默认 OpenAI 兼容 provider）
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            provider: LlmProvider::OpenAi,
            base_url: base_url.into(),
            api_key: api_key.into(),
            model: model.into(),
        }
    }

    /// 从配置文件或环境变量加载指定模型的配置
    ///
    /// 查找顺序：YAML 配置文件 → 环境变量（`AGENT_MODEL_*`）
    pub fn from_model(model_name: &str) -> Result<Self> {
        let config = Config::get_model(model_name)?;
        Ok(Self {
            provider: LlmProvider::OpenAi,
            base_url: config.baseurl,
            api_key: config.apikey,
            model: config.model,
        })
    }

    /// 从环境变量创建配置（`from_model` 的别名，向后兼容）
    pub fn from_env(model_name: &str) -> Result<Self> {
        Self::from_model(model_name)
    }

    /// 创建 OpenAI 配置
    pub fn openai(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: LlmProvider::OpenAi,
            base_url: "https://api.openai.com/v1/chat/completions".to_string(),
            api_key: api_key.into(),
            model: model.into(),
        }
    }

    /// 创建 Anthropic 配置
    pub fn anthropic(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: LlmProvider::Anthropic,
            base_url: "https://api.anthropic.com/v1/messages".to_string(),
            api_key: api_key.into(),
            model: model.into(),
        }
    }

    /// 创建 Ollama 本地推理配置
    pub fn ollama(model: impl Into<String>) -> Self {
        Self {
            provider: LlmProvider::Ollama,
            base_url: "http://localhost:11434/api/chat".to_string(),
            api_key: String::new(),
            model: model.into(),
        }
    }

    /// 创建 DeepSeek 配置
    pub fn deepseek(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: LlmProvider::OpenAi,
            base_url: "https://api.deepseek.com/chat/completions".to_string(),
            api_key: api_key.into(),
            model: model.into(),
        }
    }

    /// 创建阿里云百炼（DashScope）配置
    pub fn dashscope(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: LlmProvider::OpenAi,
            base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions"
                .to_string(),
            api_key: api_key.into(),
            model: model.into(),
        }
    }

    /// 创建自定义端点的配置（`new` 的别名）
    pub fn custom(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self::new(base_url, api_key, model)
    }

    /// 根据 provider 字段构建对应的 [`LlmClient`] 实例
    pub fn build_client(&self) -> Result<Box<dyn crate::llm::LlmClient>> {
        match self.provider {
            LlmProvider::OpenAi => {
                let client = crate::llm::OpenAiClient::new(self.clone())?;
                Ok(Box::new(client))
            }
            LlmProvider::Anthropic => {
                let client = crate::llm::providers::AnthropicClient::with_base_url(
                    &self.base_url,
                    &self.api_key,
                    &self.model,
                );
                Ok(Box::new(client))
            }
            LlmProvider::Ollama => {
                let client =
                    crate::llm::providers::OllamaClient::with_base_url(&self.base_url, &self.model);
                Ok(Box::new(client))
            }
        }
    }

    /// 转换为内部 ModelConfig 格式
    pub(crate) fn to_model_config(&self) -> ModelConfig {
        ModelConfig {
            model: self.model.clone(),
            baseurl: self.base_url.clone(),
            apikey: self.api_key.clone(),
        }
    }
}

// ── 内置 Provider 定义 ───────────────────────────────────────────────────────

/// 已知 Provider 的默认 base_url 映射
fn provider_base_url(provider: &str) -> Option<&'static str> {
    match provider.to_lowercase().as_str() {
        "openai" => Some("https://api.openai.com/v1/chat/completions"),
        "anthropic" => Some("https://api.anthropic.com/v1/messages"),
        "deepseek" => Some("https://api.deepseek.com/chat/completions"),
        "dashscope" | "qwen" | "aliyun" => {
            Some("https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions")
        }
        "moonshot" | "kimi" => Some("https://api.moonshot.cn/v1/chat/completions"),
        "zhipu" | "glm" => Some("https://open.bigmodel.cn/api/paas/v4/chat/completions"),
        "ollama" => Some("http://localhost:11434/v1/chat/completions"),
        _ => None,
    }
}

// ── YAML 配置文件类型 ────────────────────────────────────────────────────────

/// 配置文件根结构
#[derive(Debug, Deserialize)]
struct ConfigFile {
    models: HashMap<String, ModelEntry>,
}

/// 单个模型的配置条目
#[derive(Debug, Deserialize)]
struct ModelEntry {
    /// API 端点 URL（与 `provider` 二选一）
    #[serde(default)]
    base_url: Option<String>,
    /// API 密钥
    api_key: String,
    /// 实际发送给 API 的模型名称（默认使用配置 key）
    #[serde(default)]
    model: Option<String>,
    /// 内置 Provider 名称（如 "openai"、"deepseek"），自动填充 base_url
    #[serde(default)]
    provider: Option<String>,
}

// ── 内部配置类型 ─────────────────────────────────────────────────────────────

/// 单个模型的连接配置（内部使用）
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ModelConfig {
    /// LLM 接口中使用的模型名（如 `qwen3-max`）
    pub model: String,
    /// Chat Completions 接口完整 URL
    pub baseurl: String,
    /// API 密钥
    pub apikey: String,
}

/// 全局配置，持有所有已加载的模型配置表
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Config {
    pub models: HashMap<String, ModelConfig>,
}

static MODEL_CONFIG: OnceLock<Config> = OnceLock::new();

impl Config {
    /// 加载配置（YAML 配置文件优先，回退到环境变量）
    pub fn load() -> Result<Self> {
        dotenv::dotenv().ok();

        // 优先尝试 YAML 配置文件
        if let Some(config) = Self::from_config_file()? {
            tracing::info!("📄 已从配置文件加载模型配置");
            return Ok(config);
        }

        // 回退到环境变量
        tracing::debug!("未找到配置文件，尝试从环境变量加载");
        Self::from_legacy_env()
    }

    /// 查找配置文件路径
    ///
    /// 查找顺序：
    /// 1. `$ECHO_AGENT_CONFIG` 环境变量指定的路径
    /// 2. `./echo-agent.yaml`（当前目录）
    /// 3. `~/.echo-agent/config.yaml`（用户目录）
    fn config_file_path() -> Option<PathBuf> {
        // 1. 环境变量指定
        if let Ok(path) = std::env::var("ECHO_AGENT_CONFIG") {
            let p = PathBuf::from(&path);
            if p.exists() {
                return Some(p);
            }
        }

        // 2. 当前目录
        let local = PathBuf::from("echo-agent.yaml");
        if local.exists() {
            return Some(local);
        }

        // 3. 用户主目录
        if let Ok(home) = std::env::var("HOME") {
            let global = PathBuf::from(home).join(".echo-agent").join("config.yaml");
            if global.exists() {
                return Some(global);
            }
        }

        None
    }

    /// 从 YAML 配置文件加载
    fn from_config_file() -> Result<Option<Self>> {
        let path = match Self::config_file_path() {
            Some(p) => p,
            None => return Ok(None),
        };

        tracing::debug!("正在加载配置文件: {}", path.display());

        let content = std::fs::read_to_string(&path).map_err(|e| {
            ConfigError::ConfigFileError(format!("无法读取配置文件 {}: {}", path.display(), e))
        })?;

        let file: ConfigFile = serde_yaml::from_str(&content).map_err(|e| {
            ConfigError::ConfigFileError(format!("配置文件解析失败 {}: {}", path.display(), e))
        })?;

        let mut models = HashMap::new();

        for (key, entry) in file.models {
            // 解析 base_url：显式指定 > provider 快捷方式
            let base_url = match (entry.base_url.as_deref(), entry.provider.as_deref()) {
                (Some(url), _) => resolve_env_ref(url),
                (None, Some(provider)) => {
                    let resolved_provider = resolve_env_ref(provider);
                    provider_base_url(&resolved_provider)
                        .ok_or_else(|| {
                            ConfigError::ConfigFileError(format!(
                                "模型 '{}' 指定了未知的 provider: '{}'，\
                                 支持的 provider: openai, anthropic, deepseek, dashscope, moonshot, zhipu, ollama",
                                key, resolved_provider
                            ))
                        })?
                        .to_string()
                }
                (None, None) => {
                    return Err(ConfigError::MissingConfig(
                        key.clone(),
                        "base_url 或 provider".to_string(),
                    )
                    .into());
                }
            };

            let api_key = resolve_env_ref(&entry.api_key);
            let model_name = entry
                .model
                .as_deref()
                .map(resolve_env_ref)
                .unwrap_or_else(|| key.clone());

            let mc = ModelConfig {
                model: model_name.clone(),
                baseurl: base_url,
                apikey: api_key,
            };

            // 同时注册 key 和 model_name，方便按任意名称查找
            models.insert(key.clone(), mc.clone());
            if key != model_name {
                models.insert(model_name, mc);
            }
        }

        Ok(Some(Config { models }))
    }

    /// 从环境变量加载（兼容旧格式 `AGENT_MODEL_<ID>_*`）
    fn from_legacy_env() -> Result<Self> {
        const PREFIX: &str = "AGENT_MODEL_";
        let mut model_configs: HashMap<String, HashMap<String, String>> = HashMap::new();

        for (key, value) in std::env::vars() {
            if let Some(suffix) = key.strip_prefix(PREFIX) {
                // 找到最后一个 _ 作为分隔符，前面是 model_id，后面是 config_key
                // 这样支持 model_id 中包含下划线，如 AGENT_MODEL_QWEN3_MAX_APIKEY
                let (model_id, config_key) = match suffix.rfind('_') {
                    Some(pos) => {
                        let id = suffix[..pos].to_lowercase();
                        let cfg = suffix[pos + 1..].to_lowercase();
                        (id, cfg)
                    }
                    None => {
                        return Err(ReactError::Config(ConfigError::EnvFormatError(key)));
                    }
                };

                match config_key.as_str() {
                    "model" | "baseurl" | "apikey" => {}
                    _ => {
                        return Err(ReactError::Config(ConfigError::UnMatchConfigError(
                            config_key, key,
                        )));
                    }
                }

                model_configs
                    .entry(model_id)
                    .or_default()
                    .insert(config_key, value);
            }
        }

        let mut models = HashMap::new();
        for (model_id, config_map) in model_configs {
            let model = config_map
                .get("model")
                .ok_or_else(|| ConfigError::MissingConfig(model_id.clone(), "model".to_string()))?
                .clone();
            let baseurl = config_map
                .get("baseurl")
                .ok_or_else(|| ConfigError::MissingConfig(model_id.clone(), "baseurl".to_string()))?
                .clone();
            let apikey = config_map
                .get("apikey")
                .ok_or_else(|| ConfigError::MissingConfig(model_id.clone(), "apikey".to_string()))?
                .clone();

            models.insert(
                model.to_string(),
                ModelConfig {
                    model,
                    baseurl,
                    apikey,
                },
            );
        }

        Ok(Self { models })
    }

    // ── 公共查询 API ─────────────────────────────────────────────────────────

    /// 获取指定模型的配置
    pub fn get_model(model: &str) -> Result<ModelConfig> {
        let config = MODEL_CONFIG.get_or_init(|| Config::load().expect("Failed to load config"));
        config
            .models
            .get(model)
            .ok_or_else(|| {
                let available: Vec<&str> = config.models.keys().map(|k| k.as_str()).collect();
                ReactError::Config(ConfigError::NotFindModelError(format!(
                    "{}（可用模型: {}）",
                    model,
                    if available.is_empty() {
                        "无，请创建 echo-agent.yaml 或设置 AGENT_MODEL_* 环境变量".to_string()
                    } else {
                        available.join(", ")
                    }
                )))
            })
            .cloned()
    }

    /// 检查模型配置是否存在
    pub fn has_model(model: &str) -> bool {
        let config = MODEL_CONFIG.get_or_init(|| Config::load().expect("Failed to load config"));
        config.models.contains_key(model)
    }

    /// 列出所有可用的模型名称
    pub fn list_models() -> Vec<String> {
        let config = MODEL_CONFIG.get_or_init(|| Config::load().expect("Failed to load config"));
        config.models.keys().cloned().collect()
    }

    /// 向后兼容：`from_env` 是 `load` 的别名
    pub fn from_env() -> Result<Self> {
        Self::load()
    }
}

// ── 工具函数 ─────────────────────────────────────────────────────────────────

/// 解析字符串中的 `${VAR_NAME}` 环境变量引用
///
/// - `${VAR_NAME}` → 展开为环境变量值
/// - 普通字符串 → 原样返回
/// - 环境变量不存在时原样保留 `${VAR_NAME}`
fn resolve_env_ref(value: &str) -> String {
    if !value.contains("${") {
        return value.to_string();
    }

    let mut result = value.to_string();
    let mut search_from = 0;
    while let Some(rel_start) = result[search_from..].find("${") {
        let start = search_from + rel_start;
        if let Some(rel_end) = result[start..].find('}') {
            let end = start + rel_end;
            let var_name = &result[start + 2..end];
            match std::env::var(var_name) {
                Ok(val) => {
                    result = format!("{}{}{}", &result[..start], val, &result[end + 1..]);
                    search_from = start + val.len();
                }
                Err(_) => {
                    tracing::warn!("环境变量 {} 未设置", var_name);
                    search_from = end + 1;
                }
            }
        } else {
            break;
        }
    }

    result
}

// ── 单元测试 ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_llm_config_new() {
        let config = LlmConfig::new("https://example.com", "sk-test", "gpt-4o");
        assert_eq!(config.base_url, "https://example.com");
        assert_eq!(config.api_key, "sk-test");
        assert_eq!(config.model, "gpt-4o");
    }

    #[test]
    fn test_llm_config_openai() {
        let config = LlmConfig::openai("sk-test", "gpt-4o");
        assert!(config.base_url.contains("openai.com"));
        assert_eq!(config.model, "gpt-4o");
    }

    #[test]
    fn test_llm_config_deepseek() {
        let config = LlmConfig::deepseek("sk-test", "deepseek-chat");
        assert!(config.base_url.contains("deepseek.com"));
        assert_eq!(config.model, "deepseek-chat");
    }

    #[test]
    fn test_llm_config_dashscope() {
        let config = LlmConfig::dashscope("sk-test", "qwen3-max");
        assert!(config.base_url.contains("dashscope.aliyuncs.com"));
        assert_eq!(config.model, "qwen3-max");
    }

    #[test]
    fn test_llm_config_anthropic() {
        let config = LlmConfig::anthropic("sk-test", "claude-sonnet-4-6");
        assert!(config.base_url.contains("anthropic.com"));
        assert_eq!(config.model, "claude-sonnet-4-6");
    }

    #[test]
    fn test_provider_base_url() {
        assert!(provider_base_url("openai").is_some());
        assert!(provider_base_url("anthropic").is_some());
        assert!(provider_base_url("deepseek").is_some());
        assert!(provider_base_url("dashscope").is_some());
        assert!(provider_base_url("qwen").is_some());
        assert!(provider_base_url("ollama").is_some());
        assert!(provider_base_url("unknown_provider").is_none());
    }

    #[test]
    fn test_resolve_env_ref_plain() {
        assert_eq!(resolve_env_ref("sk-plain-key"), "sk-plain-key");
    }

    #[test]
    fn test_resolve_env_ref_with_var() {
        unsafe { std::env::set_var("TEST_ECHO_KEY", "resolved-value") };
        assert_eq!(resolve_env_ref("${TEST_ECHO_KEY}"), "resolved-value");
        unsafe { std::env::remove_var("TEST_ECHO_KEY") };
    }

    #[test]
    fn test_resolve_env_ref_missing_var() {
        let result = resolve_env_ref("${NONEXISTENT_VAR_12345}");
        assert_eq!(result, "${NONEXISTENT_VAR_12345}");
    }

    #[test]
    fn test_config_from_yaml_string() {
        let yaml = r#"
models:
  test-model:
    base_url: https://api.example.com/v1/chat
    api_key: sk-test-key
  alias-model:
    provider: openai
    api_key: sk-alias
    model: gpt-4o-mini
"#;
        let file: ConfigFile = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(file.models.len(), 2);
        assert!(file.models.contains_key("test-model"));
        assert!(file.models.contains_key("alias-model"));

        let entry = &file.models["alias-model"];
        assert_eq!(entry.provider.as_deref(), Some("openai"));
        assert_eq!(entry.model.as_deref(), Some("gpt-4o-mini"));
    }

    #[test]
    fn test_to_model_config() {
        let config = LlmConfig::new("https://example.com", "sk-test", "model-1");
        let mc = config.to_model_config();
        assert_eq!(mc.model, "model-1");
        assert_eq!(mc.baseurl, "https://example.com");
        assert_eq!(mc.apikey, "sk-test");
    }
}
