use crate::error::{ConfigError, Result};
use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Model {
    pub model: String,
    pub baseurl: String,
    pub apikey: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Models {
    pub high: Model,
    pub middle: Model,
    pub default: Model,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Config {
    pub models: Models,
}

impl Config {
    pub fn load(path: &str) -> Result<Self> {
        let file = std::fs::File::open(path).map_err(|_| {
            ConfigError::FileNotFound(path.to_string())
        })?;
        let config: Config = serde_yaml::from_reader(file)?;
        Ok(config)
    }
}
