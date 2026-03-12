//! LLM 配置加载
//!
//! 从环境变量读取模型配置，格式：
//! ```text
//! AGENT_MODEL_<ID>_MODEL=gpt-4o
//! AGENT_MODEL_<ID>_BASEURL=https://api.openai.com/v1/chat/completions
//! AGENT_MODEL_<ID>_APIKEY=sk-...
//! ```
//! `<ID>` 为自定义标识（如 `GPT4O`、`QWEN`），不区分大小写。

use crate::error::{ConfigError, ReactError, Result};
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::OnceLock;

/// LLM 运行时配置（依赖注入模式）
///
/// 可以直接创建并注入到 Agent，无需环境变量。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    /// Chat Completions 接口完整 URL
    pub base_url: String,
    /// API 密钥
    pub api_key: String,
    /// 模型名称
    pub model: String,
}

impl LlmConfig {
    /// 创建新的 LLM 配置
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            base_url: base_url.into(),
            api_key: api_key.into(),
            model: model.into(),
        }
    }

    /// 从环境变量创建配置
    ///
    /// 格式：`AGENT_MODEL_<ID>_MODEL`, `AGENT_MODEL_<ID>_BASEURL`, `AGENT_MODEL_<ID>_APIKEY`
    pub fn from_env(model_name: &str) -> Result<Self> {
        let config = Config::get_model(model_name)?;
        Ok(Self {
            base_url: config.baseurl,
            api_key: config.apikey,
            model: config.model,
        })
    }

    /// 创建 OpenAI 兼容的配置
    pub fn openai(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            base_url: "https://api.openai.com/v1/chat/completions".to_string(),
            api_key: api_key.into(),
            model: model.into(),
        }
    }

    /// 创建自定义端点的配置
    pub fn custom(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self::new(base_url, api_key, model)
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

// ── 环境变量配置（向后兼容）───────────────────────────────────────────────────────

/// 单个模型的连接配置（内部使用）
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ModelConfig {
    /// LLM 接口中使用的模型名（如 `qwen3-max`）
    pub model: String,
    /// Chat Completions 接口完整 URL
    pub baseurl: String,
    pub apikey: String,
}

/// 全局配置，持有所有已加载的模型配置表（key = model 字段值）
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Config {
    pub models: HashMap<String, ModelConfig>,
}

static MODEL_CONFIG: OnceLock<Config> = OnceLock::new();

impl Config {
    pub fn from_env() -> Result<Self> {
        dotenv::dotenv().ok();

        const PREFIX: &str = "AGENT_MODEL_";
        let mut model_configs: HashMap<String, HashMap<String, String>> = HashMap::new();
        for (key, value) in std::env::vars() {
            if let Some(suffix) = key.strip_prefix(PREFIX) {
                let parts: Vec<&str> = suffix.split('_').collect();
                if parts.len() != 2 {
                    return Err(ReactError::Config(ConfigError::EnvFormatError(key)));
                }
                let model_id = parts[0].to_lowercase();
                let config_key = parts[1].to_lowercase();

                match config_key.as_str() {
                    "model" | "baseurl" | "apikey" => {}
                    _ => {
                        return Err(ReactError::Config(ConfigError::UnMatchConfigError(
                            config_key, key,
                        )));
                    }
                }
                model_configs
                    .entry(model_id.clone())
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

    pub fn get_model(model: &str) -> Result<ModelConfig> {
        let config =
            MODEL_CONFIG.get_or_init(|| Config::from_env().expect("Failed to load config"));
        config
            .models
            .get(model)
            .ok_or_else(|| ReactError::Config(ConfigError::NotFindModelError(model.to_string())))
            .cloned()
    }

    /// 检查模型配置是否存在
    pub fn has_model(model: &str) -> bool {
        let config =
            MODEL_CONFIG.get_or_init(|| Config::from_env().expect("Failed to load config"));
        config.models.contains_key(model)
    }

    /// 列出所有可用的模型名称
    pub fn list_models() -> Vec<String> {
        let config =
            MODEL_CONFIG.get_or_init(|| Config::from_env().expect("Failed to load config"));
        config.models.keys().cloned().collect()
    }
}
