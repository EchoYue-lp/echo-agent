use crate::error::{ConfigError, ReactError, Result};
use dotenv::dotenv;
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::OnceLock;
use tracing::{debug, info, warn};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ModelConfig {
    pub model: String,
    pub baseurl: String,
    pub apikey: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Config {
    pub models: HashMap<String, ModelConfig>,
}

static MODEL_CONFIG: OnceLock<Config> = OnceLock::new();

impl Config {
    pub fn from_env() -> Result<Self> {
        dotenv().ok();

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
                    .or_insert_with(HashMap::new)
                    .insert(config_key, value);
            }
        }
        let mut models = HashMap::new();
        for (model_id, config_map) in model_configs {
            // 检查必要配置项是否齐全
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
            MODEL_CONFIG.get_or_init(|| Config::from_env().expect("Failed to load config. "));
        let result = config.models.get(model);
        result
            .ok_or_else(|| ReactError::Config(ConfigError::NotFindModelError(model.to_string())))
            .cloned()
    }
}
