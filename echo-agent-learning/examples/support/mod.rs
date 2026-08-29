use echo_agent::error::{ConfigError, Result};
use echo_agent::llm::{LlmApiProtocol, LlmConfig};

pub fn llm_config(selector: Option<&str>) -> Result<LlmConfig> {
    let provider = required_env("ECHO_AGENT_PROVIDER")?;
    let base_url = required_env("ECHO_AGENT_BASE_URL")?;
    let model = selector
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .map_or_else(|| required_env("ECHO_AGENT_MODEL"), Ok)?;
    let api_key = std::env::var("ECHO_AGENT_API_KEY").unwrap_or_default();
    let protocol = match required_env("ECHO_AGENT_API_PROTOCOL")?
        .to_ascii_lowercase()
        .as_str()
    {
        "chat_completions" | "chat-completions" => LlmApiProtocol::ChatCompletions,
        "responses" => LlmApiProtocol::Responses,
        "anthropic" | "messages" => LlmApiProtocol::Anthropic,
        value => {
            return Err(ConfigError::ConfigFileError(format!(
                "unsupported ECHO_AGENT_API_PROTOCOL '{value}'"
            ))
            .into());
        }
    };
    LlmConfig::for_provider(&provider, &base_url, api_key, &model, protocol)
}

fn required_env(name: &str) -> Result<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ConfigError::MissingConfig("example".to_string(), name.to_string()).into())
}
