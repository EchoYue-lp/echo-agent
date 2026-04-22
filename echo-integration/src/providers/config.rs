//! LLM 配置加载
//!
//! 通过 YAML 配置文件加载模型与 embedding 配置。
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
use echo_core::error::{ConfigError, Result};
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
/// ```rust,no_run
/// use echo_integration::providers::LlmConfig;
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

    /// 从配置文件加载指定模型的配置
    ///
    /// 查找顺序：YAML 配置文件
    ///
    /// 自动识别 provider 类型：如果配置中指定了 `provider` 字段则直接使用，
    /// 否则根据 `base_url` 自动推断（Anthropic / Ollama / OpenAI 兼容）。
    pub fn from_model(model_name: &str) -> Result<Self> {
        let config = Config::get_model(model_name)?;
        Ok(Self {
            provider: config.provider,
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

    /// 根据 provider 字段构建对应的 [`LlmClient`](echo_core::llm::LlmClient) 实例
    pub fn build_client(&self) -> Result<Box<dyn echo_core::llm::LlmClient>> {
        match self.provider {
            LlmProvider::OpenAi => {
                let client = super::openai::OpenAiClient::new(self.clone())?;
                Ok(Box::new(client))
            }
            LlmProvider::Anthropic => {
                let client = super::anthropic::AnthropicClient::with_base_url(
                    &self.base_url,
                    &self.api_key,
                    &self.model,
                );
                Ok(Box::new(client))
            }
            LlmProvider::Ollama => {
                let client =
                    super::ollama::OllamaClient::with_base_url(&self.base_url, &self.model);
                Ok(Box::new(client))
            }
        }
    }

    /// 转换为内部 ModelConfig 格式
    pub fn to_model_config(&self) -> ModelConfig {
        ModelConfig {
            model: self.model.clone(),
            baseurl: self.base_url.clone(),
            apikey: self.api_key.clone(),
            provider: self.provider.clone(),
        }
    }
}

// ── ProviderFactory ─────────────────────────────────────────────────────────

/// Provider 工厂：通过配置字符串自动实例化 LLM 客户端
///
/// 支持三种配置方式：
///
/// 1. **模型名称**：从配置文件/环境变量加载（如 `"qwen3-max"`）
/// 2. **Provider:Model 格式**：自动匹配内置 Provider（如 `"anthropic:claude-sonnet-4-6"`、`"ollama:llama3"`）
/// 3. **完整 LlmConfig**：手动构造配置后调用 `from_config()`
///
/// # 示例
///
/// ```rust,no_run
/// use echo_integration::providers::ProviderFactory;
/// use echo_core::error::Result;
///
/// # fn example() -> Result<()> {
/// // 方式一：从配置文件加载
/// let client = ProviderFactory::create("qwen3-max")?;
///
/// // 方式二：Provider:Model 简写
/// let client = ProviderFactory::create("anthropic:claude-sonnet-4-6")?;
/// let client = ProviderFactory::create("ollama:llama3")?;
/// let client = ProviderFactory::create("deepseek:deepseek-chat")?;
///
/// // 方式三：从已有配置构建
/// use echo_integration::providers::LlmConfig;
/// let config = LlmConfig::anthropic("sk-ant-...", "claude-sonnet-4-6");
/// let client = ProviderFactory::from_config(&config)?;
/// # Ok(())
/// # }
/// ```
pub struct ProviderFactory;

impl ProviderFactory {
    /// 通过配置字符串自动创建 LLM 客户端
    ///
    /// 解析规则：
    /// - 含 `:` 分隔符 → 解析为 `provider:model`，自动填充 base_url，API key 从环境变量获取
    /// - 不含 `:` → 视为模型名称，从配置文件/环境变量加载完整配置
    pub fn create(config_str: &str) -> Result<Box<dyn echo_core::llm::LlmClient>> {
        if let Some((provider_name, model_name)) = config_str.split_once(':') {
            Self::from_provider_model(provider_name.trim(), model_name.trim())
        } else {
            let config = LlmConfig::from_model(config_str)?;
            config.build_client()
        }
    }

    /// 从 `LlmConfig` 构建客户端
    pub fn from_config(config: &LlmConfig) -> Result<Box<dyn echo_core::llm::LlmClient>> {
        config.build_client()
    }

    /// 从 provider 名称 + 模型名称自动构建
    ///
    /// API key 从对应的环境变量读取：
    /// - `anthropic` → `ANTHROPIC_API_KEY`
    /// - `openai` → `OPENAI_API_KEY`
    /// - `deepseek` → `DEEPSEEK_API_KEY`
    /// - `dashscope`/`qwen` → `DASHSCOPE_API_KEY` / `QWEN_API_KEY`
    /// - `moonshot` → `MOONSHOT_API_KEY`
    /// - `zhipu` → `ZHIPU_API_KEY`
    /// - `ollama` → 无需 API key
    fn from_provider_model(
        provider: &str,
        model: &str,
    ) -> Result<Box<dyn echo_core::llm::LlmClient>> {
        let base_url = provider_base_url(provider).ok_or_else(|| {
            ConfigError::ConfigFileError(format!(
                "未知的 provider: '{provider}'，\
                 支持: openai, anthropic, deepseek, dashscope, moonshot, zhipu, ollama"
            ))
        })?;

        let api_key = Self::env_api_key(provider);
        if api_key.trim().is_empty() && !matches!(provider.to_lowercase().as_str(), "ollama") {
            return Err(ConfigError::MissingConfig(
                format!("{provider}:{model}"),
                format!(
                    "缺少 API key，请设置以下任一环境变量: {}",
                    provider_env_var_names(provider).join(", ")
                ),
            )
            .into());
        }
        let llm_provider = parse_provider(provider);

        let config = LlmConfig {
            provider: llm_provider,
            base_url: base_url.to_string(),
            api_key,
            model: model.to_string(),
        };
        config.build_client()
    }

    /// 根据 provider 名称获取对应的环境变量 API key
    fn env_api_key(provider: &str) -> String {
        let env_vars: &[&str] = match provider.to_lowercase().as_str() {
            "anthropic" => &["ANTHROPIC_API_KEY"],
            "openai" => &["OPENAI_API_KEY"],
            "deepseek" => &["DEEPSEEK_API_KEY"],
            "dashscope" | "qwen" | "aliyun" => &["DASHSCOPE_API_KEY", "QWEN_API_KEY"],
            "moonshot" | "kimi" => &["MOONSHOT_API_KEY", "KIMI_API_KEY"],
            "zhipu" | "glm" => &["ZHIPU_API_KEY", "GLM_API_KEY"],
            "ollama" => return String::new(),
            _ => return String::new(),
        };
        first_present_env(env_vars).unwrap_or_default()
    }

    /// 列出所有支持的 provider 名称
    pub fn supported_providers() -> &'static [&'static str] {
        &[
            "openai",
            "anthropic",
            "deepseek",
            "dashscope",
            "qwen",
            "moonshot",
            "kimi",
            "zhipu",
            "glm",
            "ollama",
        ]
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

/// 从 provider 字符串解析 [`LlmProvider`] 枚举
fn parse_provider(provider: &str) -> LlmProvider {
    match provider.to_lowercase().as_str() {
        "anthropic" => LlmProvider::Anthropic,
        "ollama" => LlmProvider::Ollama,
        // OpenAI 兼容类（openai、deepseek、dashscope 等）统一走 OpenAI 实现
        _ => LlmProvider::OpenAi,
    }
}

/// 根据 base_url 自动推断 [`LlmProvider`]
fn detect_provider_from_url(url: &str) -> LlmProvider {
    let lower = url.to_lowercase();
    if lower.contains("anthropic.com") {
        LlmProvider::Anthropic
    } else if lower.contains("localhost:11434") || lower.contains("ollama") {
        LlmProvider::Ollama
    } else {
        LlmProvider::OpenAi
    }
}

// ── YAML 配置文件类型 ────────────────────────────────────────────────────────

/// 配置文件根结构
#[derive(Debug, Deserialize)]
struct ConfigFile {
    models: HashMap<String, ModelEntry>,
    #[serde(default)]
    embedding: Option<EmbeddingEntry>,
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

/// Embedding 配置条目
#[derive(Debug, Deserialize)]
struct EmbeddingEntry {
    /// 完整 embeddings 端点 URL（与 `base_url` 二选一）
    #[serde(default)]
    endpoint_url: Option<String>,
    /// 基础 URL（自动追加 `/v1/embeddings`）
    #[serde(default)]
    base_url: Option<String>,
    /// API 密钥
    api_key: String,
    /// 实际发送给 API 的模型名称
    #[serde(default)]
    model: Option<String>,
    /// 超时时间（秒）
    #[serde(default)]
    timeout_secs: Option<u64>,
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
    /// LLM 供应商类型（用于自动选择客户端实现）
    #[serde(default)]
    pub provider: LlmProvider,
}

/// 全局配置，持有所有已加载的模型配置表
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Config {
    pub models: HashMap<String, ModelConfig>,
    #[serde(default)]
    invalid_models: HashMap<String, String>,
    #[serde(default)]
    pub embedding: Option<EmbeddingConfig>,
    #[serde(default)]
    invalid_embedding: Option<String>,
}

/// Embedding 运行时配置
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct EmbeddingConfig {
    /// Embeddings 接口完整 URL
    pub url: String,
    /// API 密钥
    pub api_key: String,
    /// 模型名称
    pub model: String,
    /// 超时时间（秒）
    pub timeout_secs: u64,
}

/// 缓存的加载结果：`Ok(Config)` 或 `Err(描述)`。
/// 使用 `Result<Config, String>` 而非裸 `Config`，加载失败时不 panic，
/// 只缓存错误信息，由调用方决定错误处理策略。
static MODEL_CONFIG: OnceLock<std::result::Result<Config, String>> = OnceLock::new();

impl Config {
    /// 加载配置（只读取 YAML 配置文件）
    ///
    /// 每次调用都重新读取文件，不使用缓存。
    /// 如需缓存请使用 [`load_cached`](Self::load_cached)。
    pub fn load() -> Result<Self> {
        dotenv::dotenv().ok();

        if let Some(config) = Self::from_config_file()? {
            tracing::info!("已从配置文件加载模型配置");
            return Ok(config);
        }

        Err(ConfigError::ConfigFileError(
            "未找到模型配置文件，请提供 echo-agent.yaml；环境变量仅支持通过 `${VAR}` 在 YAML 中注入值".to_string(),
        )
        .into())
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
        let local = PathBuf::from("./echo-agent.yaml");
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
        let mut invalid_models = HashMap::new();

        for (key, entry) in file.models {
            let parsed: Result<(String, String, String, LlmProvider)> = (|| {
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

                let api_key = ensure_resolved_api_key(
                    &key,
                    "api_key",
                    &entry.api_key,
                    &resolve_env_ref(&entry.api_key),
                )?;
                let model_name = entry
                    .model
                    .as_deref()
                    .map(resolve_env_ref)
                    .unwrap_or_else(|| key.clone());

                // 确定 provider：显式指定 > 从 base_url 推断
                let provider = match entry.provider.as_deref() {
                    Some(p) => parse_provider(&resolve_env_ref(p)),
                    None => detect_provider_from_url(&base_url),
                };
                Ok((base_url, api_key, model_name, provider))
            })();

            match parsed {
                Ok((base_url, api_key, model_name, provider)) => {
                    let mc = ModelConfig {
                        model: model_name.clone(),
                        baseurl: base_url,
                        apikey: api_key,
                        provider,
                    };

                    models.insert(key.clone(), mc.clone());
                    if key != model_name {
                        models.insert(model_name, mc);
                    }
                }
                Err(err) => {
                    tracing::warn!("跳过无效模型配置 {}: {}", key, err);
                    invalid_models.insert(key.clone(), err.to_string());
                }
            }
        }

        let (embedding, invalid_embedding) = match file.embedding {
            Some(entry) => {
                let parsed: Result<EmbeddingConfig> = (|| {
                    let url = match (entry.endpoint_url.as_deref(), entry.base_url.as_deref()) {
                        (Some(url), _) => resolve_env_ref(url),
                        (None, Some(base)) => {
                            let resolved = resolve_env_ref(base);
                            format!("{}/v1/embeddings", resolved.trim_end_matches('/'))
                        }
                        (None, None) => {
                            return Err(ConfigError::MissingConfig(
                                "embedding".to_string(),
                                "endpoint_url 或 base_url".to_string(),
                            )
                            .into());
                        }
                    };

                    Ok(EmbeddingConfig {
                        url,
                        api_key: ensure_resolved_api_key(
                            "embedding",
                            "api_key",
                            &entry.api_key,
                            &resolve_env_ref(&entry.api_key),
                        )?,
                        model: entry
                            .model
                            .as_deref()
                            .map(resolve_env_ref)
                            .unwrap_or_else(|| "text-embedding-3-small".to_string()),
                        timeout_secs: entry.timeout_secs.unwrap_or(30),
                    })
                })();

                match parsed {
                    Ok(cfg) => (Some(cfg), None),
                    Err(err) => {
                        tracing::warn!("跳过无效 embedding 配置: {}", err);
                        (None, Some(err.to_string()))
                    }
                }
            }
            None => (None, None),
        };

        Ok(Some(Config {
            models,
            invalid_models,
            embedding,
            invalid_embedding,
        }))
    }

    // ── 公共查询 API ─────────────────────────────────────────────────────────

    /// 惰性加载并缓存配置（进程级单例）
    ///
    /// 首次调用时执行 [`load`](Self::load)，后续调用返回缓存结果。
    /// 加载失败时缓存错误信息，不 panic。
    pub fn load_cached() -> Result<&'static Config> {
        let result = MODEL_CONFIG.get_or_init(|| Config::load().map_err(|e| e.to_string()));
        match result {
            Ok(config) => Ok(config),
            Err(msg) => Err(ConfigError::ConfigFileError(msg.clone()).into()),
        }
    }

    /// 获取指定模型的配置
    pub fn get_model(model: &str) -> Result<ModelConfig> {
        let config = Self::load_cached()?;
        if let Some(err) = config.invalid_models.get(model) {
            return Err(ConfigError::ConfigFileError(err.clone()).into());
        }
        Ok(config
            .models
            .get(model)
            .ok_or_else(|| {
                let available: Vec<&str> = config.models.keys().map(|k| k.as_str()).collect();
                ConfigError::NotFindModelError(format!(
                    "{}（可用模型: {}）",
                    model,
                    if available.is_empty() {
                        "无，请创建 echo-agent.yaml 并在其中声明 models.*".to_string()
                    } else {
                        available.join(", ")
                    }
                ))
            })
            .cloned()?)
    }

    /// 检查模型配置是否存在
    ///
    /// 配置加载失败时返回 `false`（不 panic）。
    pub fn has_model(model: &str) -> bool {
        Self::load_cached()
            .map(|config| config.models.contains_key(model))
            .unwrap_or(false)
    }

    /// 列出所有可用的模型名称
    ///
    /// 配置加载失败时返回空列表（不 panic）。
    pub fn list_models() -> Vec<String> {
        Self::load_cached()
            .map(|config| config.models.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// 获取 embedding 配置
    pub fn get_embedding() -> Result<EmbeddingConfig> {
        let config = Self::load_cached()?;
        if let Some(err) = &config.invalid_embedding {
            return Err(ConfigError::ConfigFileError(err.clone()).into());
        }
        config.embedding.clone().ok_or_else(|| {
            ConfigError::MissingConfig(
                "embedding".to_string(),
                "请在 echo-agent.yaml 中配置 embedding 段".to_string(),
            )
            .into()
        })
    }

    /// 检查 embedding 配置是否存在
    pub fn has_embedding() -> bool {
        Self::load_cached()
            .map(|config| config.embedding.is_some())
            .unwrap_or(false)
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
            match std::env::var(var_name).ok().or_else(|| {
                fallback_env_alias(var_name)
                    .and_then(std::env::var_os)
                    .map(|v| v.to_string_lossy().into_owned())
            }) {
                Some(val) => {
                    result = format!("{}{}{}", &result[..start], val, &result[end + 1..]);
                    search_from = start + val.len();
                }
                None => {
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

fn first_present_env(names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        std::env::var(name)
            .ok()
            .filter(|value| !value.trim().is_empty())
    })
}

fn fallback_env_alias(var_name: &str) -> Option<&'static str> {
    match var_name {
        "DASHSCOPE_API_KEY" => Some("QWEN_API_KEY"),
        "QWEN_API_KEY" => Some("DASHSCOPE_API_KEY"),
        "MOONSHOT_API_KEY" => Some("KIMI_API_KEY"),
        "KIMI_API_KEY" => Some("MOONSHOT_API_KEY"),
        "ZHIPU_API_KEY" => Some("GLM_API_KEY"),
        "GLM_API_KEY" => Some("ZHIPU_API_KEY"),
        _ => None,
    }
}

fn provider_env_var_names(provider: &str) -> &'static [&'static str] {
    match provider.to_lowercase().as_str() {
        "anthropic" => &["ANTHROPIC_API_KEY"],
        "openai" => &["OPENAI_API_KEY"],
        "deepseek" => &["DEEPSEEK_API_KEY"],
        "dashscope" | "qwen" | "aliyun" => &["DASHSCOPE_API_KEY", "QWEN_API_KEY"],
        "moonshot" | "kimi" => &["MOONSHOT_API_KEY", "KIMI_API_KEY"],
        "zhipu" | "glm" => &["ZHIPU_API_KEY", "GLM_API_KEY"],
        "ollama" => &[],
        _ => &[],
    }
}

fn ensure_resolved_api_key(
    scope: &str,
    field: &str,
    raw_value: &str,
    resolved_value: &str,
) -> Result<String> {
    if !raw_value.contains("${") {
        return Ok(resolved_value.to_string());
    }

    let unresolved: Vec<String> = extract_env_refs(raw_value)
        .into_iter()
        .filter(|name| {
            std::env::var(name).is_err()
                && fallback_env_alias(name)
                    .and_then(std::env::var_os)
                    .is_none()
        })
        .collect();

    if !unresolved.is_empty() || resolved_value.contains("${") || resolved_value.trim().is_empty() {
        let details = unresolved
            .into_iter()
            .map(|name| {
                if let Some(alias) = fallback_env_alias(&name) {
                    format!("{name}（或别名 {alias}）")
                } else {
                    name
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        return Err(ConfigError::MissingConfig(
            scope.to_string(),
            if details.is_empty() {
                format!("{field} 未解析为有效值")
            } else {
                format!("{field} 依赖的环境变量未设置: {details}")
            },
        )
        .into());
    }

    Ok(resolved_value.to_string())
}

fn extract_env_refs(value: &str) -> Vec<String> {
    let mut refs = Vec::new();
    let mut search_from = 0;
    while let Some(rel_start) = value[search_from..].find("${") {
        let start = search_from + rel_start;
        if let Some(rel_end) = value[start..].find('}') {
            let end = start + rel_end;
            refs.push(value[start + 2..end].to_string());
            search_from = end + 1;
        } else {
            break;
        }
    }
    refs
}

// ── 单元测试 ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn env_test_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

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
        let _guard = env_test_lock();
        unsafe { std::env::set_var("TEST_ECHO_KEY", "resolved-value") };
        assert_eq!(resolve_env_ref("${TEST_ECHO_KEY}"), "resolved-value");
        unsafe { std::env::remove_var("TEST_ECHO_KEY") };
    }

    #[test]
    fn test_resolve_env_ref_missing_var() {
        let _guard = env_test_lock();
        let result = resolve_env_ref("${NONEXISTENT_VAR_12345}");
        assert_eq!(result, "${NONEXISTENT_VAR_12345}");
    }

    #[test]
    fn test_resolve_env_ref_supports_dashscope_qwen_alias() {
        let _guard = env_test_lock();
        unsafe {
            std::env::remove_var("DASHSCOPE_API_KEY");
            std::env::set_var("QWEN_API_KEY", "qwen-alias-value");
        }
        assert_eq!(resolve_env_ref("${DASHSCOPE_API_KEY}"), "qwen-alias-value");
        unsafe {
            std::env::remove_var("QWEN_API_KEY");
        }
    }

    #[test]
    fn test_env_api_key_supports_qwen_alias() {
        let _guard = env_test_lock();
        unsafe {
            std::env::remove_var("DASHSCOPE_API_KEY");
            std::env::set_var("QWEN_API_KEY", "qwen-provider-key");
        }
        assert_eq!(
            ProviderFactory::env_api_key("dashscope"),
            "qwen-provider-key"
        );
        assert_eq!(ProviderFactory::env_api_key("qwen"), "qwen-provider-key");
        unsafe {
            std::env::remove_var("QWEN_API_KEY");
        }
    }

    #[test]
    fn test_ensure_resolved_api_key_reports_missing_alias_group() {
        let _guard = env_test_lock();
        unsafe {
            std::env::remove_var("DASHSCOPE_API_KEY");
            std::env::remove_var("QWEN_API_KEY");
        }
        let err = ensure_resolved_api_key(
            "qwen3.6-plus",
            "api_key",
            "${DASHSCOPE_API_KEY}",
            "${DASHSCOPE_API_KEY}",
        )
        .unwrap_err();
        assert!(format!("{err}").contains("DASHSCOPE_API_KEY"));
        assert!(format!("{err}").contains("QWEN_API_KEY"));
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
    fn test_config_from_yaml_with_embedding() {
        let yaml = r#"
models:
  test-model:
    provider: openai
    api_key: sk-test
embedding:
  base_url: https://api.openai.com
  api_key: ${TEST_EMBED_KEY}
  model: text-embedding-3-small
  timeout_secs: 45
"#;
        unsafe { std::env::set_var("TEST_EMBED_KEY", "embed-key") };
        let file: ConfigFile = serde_yaml::from_str(yaml).unwrap();
        let entry = file.embedding.expect("embedding should exist");
        assert_eq!(entry.base_url.as_deref(), Some("https://api.openai.com"));
        assert_eq!(resolve_env_ref(&entry.api_key), "embed-key");
        unsafe { std::env::remove_var("TEST_EMBED_KEY") };
    }

    #[test]
    fn test_to_model_config() {
        let config = LlmConfig::new("https://example.com", "sk-test", "model-1");
        let mc = config.to_model_config();
        assert_eq!(mc.model, "model-1");
        assert_eq!(mc.baseurl, "https://example.com");
        assert_eq!(mc.apikey, "sk-test");
        assert_eq!(mc.provider, LlmProvider::OpenAi);
    }

    #[test]
    fn test_to_model_config_anthropic() {
        let config = LlmConfig::anthropic("sk-ant-test", "claude-sonnet-4-6");
        let mc = config.to_model_config();
        assert_eq!(mc.provider, LlmProvider::Anthropic);
    }

    #[test]
    fn test_parse_provider() {
        assert_eq!(parse_provider("anthropic"), LlmProvider::Anthropic);
        assert_eq!(parse_provider("Anthropic"), LlmProvider::Anthropic);
        assert_eq!(parse_provider("ollama"), LlmProvider::Ollama);
        assert_eq!(parse_provider("openai"), LlmProvider::OpenAi);
        assert_eq!(parse_provider("deepseek"), LlmProvider::OpenAi);
        assert_eq!(parse_provider("unknown"), LlmProvider::OpenAi);
    }

    #[test]
    fn test_detect_provider_from_url() {
        assert_eq!(
            detect_provider_from_url("https://api.anthropic.com/v1/messages"),
            LlmProvider::Anthropic,
        );
        assert_eq!(
            detect_provider_from_url("http://localhost:11434/api/chat"),
            LlmProvider::Ollama,
        );
        assert_eq!(
            detect_provider_from_url("https://api.openai.com/v1/chat/completions"),
            LlmProvider::OpenAi,
        );
        assert_eq!(
            detect_provider_from_url("https://api.deepseek.com/chat/completions"),
            LlmProvider::OpenAi,
        );
    }

    #[test]
    fn test_provider_factory_supported_providers() {
        let providers = ProviderFactory::supported_providers();
        assert!(providers.contains(&"openai"));
        assert!(providers.contains(&"anthropic"));
        assert!(providers.contains(&"ollama"));
        assert!(providers.contains(&"deepseek"));
    }

    #[test]
    fn test_provider_factory_parse_config_str() {
        assert_eq!(ProviderFactory::env_api_key("ollama"), "");
        assert_eq!(ProviderFactory::env_api_key("unknown"), "");
    }

    #[test]
    fn test_config_from_yaml_with_provider_detection() {
        let yaml = r#"
models:
  claude-test:
    base_url: https://api.anthropic.com/v1/messages
    api_key: sk-test
  ollama-test:
    base_url: http://localhost:11434/api/chat
    api_key: ""
  openai-test:
    provider: openai
    api_key: sk-test
"#;
        let file: ConfigFile = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(file.models.len(), 3);
    }
}
