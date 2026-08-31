//! Provider-neutral framework configuration.
//!
//! File discovery, product prompts, model catalogs, channels, UI settings, and
//! server settings belong to embedding applications. The framework accepts a
//! typed configuration value and explicit paths.

use crate::agent::AgentConfig;
use echo_core::budget::TokenBudgetConfig;
use echo_core::llm::LlmApiProtocol;
use echo_core::llm::capabilities::infer_context_window;
use serde::{Deserialize, Serialize};

pub const DEFAULT_AGENT_SYSTEM_PROMPT: &str = "You are a helpful assistant.";

/// Reusable configuration for one Agent runtime.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct FrameworkConfig {
    pub model: ModelConfig,
    pub agent: AgentSettings,
}

fn resolve_context_window(explicit: Option<u32>, provider: &str, model_name: &str) -> usize {
    explicit
        .or_else(|| infer_context_window(provider, model_name))
        .unwrap_or(u32::try_from(crate::agent::config::DEFAULT_TOKEN_LIMIT).unwrap_or(128_000))
        .clamp(1, 10_000_000) as usize
}

impl From<FrameworkConfig> for AgentConfig {
    fn from(value: FrameworkConfig) -> Self {
        let FrameworkConfig { model, agent } = value;
        let context_window =
            resolve_context_window(model.context_window, &model.provider, &model.name);
        let token_limit = if agent.token_limit > 0 {
            agent.token_limit
        } else if model.context_window.is_some() {
            context_window
        } else {
            usize::MAX
        };
        let token_budget_config = if model.context_window.is_some() || agent.token_limit > 0 {
            TokenBudgetConfig {
                total_window: Some(context_window),
                ..Default::default()
            }
        } else {
            TokenBudgetConfig::default()
        };

        let mut config = AgentConfig::standard(&model.name, &agent.name, &agent.system_prompt)
            .enable_tool(agent.enable_tools)
            .enable_memory(agent.enable_memory)
            .enable_human_in_loop(agent.enable_human_in_loop)
            .max_iterations(agent.max_iterations)
            .subagent_timeout_secs(agent.subagent_timeout_secs)
            .memory_path(&agent.memory_path)
            .temperature(model.temperature)
            .max_tokens(model.max_tokens)
            .token_limit(token_limit)
            .token_budget(token_budget_config)
            .tool_execution(crate::tools::ToolExecutionConfig {
                timeout_ms: agent.tool_timeout_ms,
                ..Default::default()
            });
        if agent.max_tool_output_tokens > 0 {
            config = config.max_tool_output_tokens(agent.max_tool_output_tokens);
        }
        config
    }
}

impl FrameworkConfig {
    pub fn has_compressor(&self) -> bool {
        self.agent.token_limit > 0
            || self.model.context_window.is_some()
            || !self.agent.compress_strategy.is_empty()
    }

    pub async fn apply_compressor(&self, agent: &crate::agent::ReactAgent) {
        use crate::compression::compressor::SlidingWindowCompressor;

        if !self.has_compressor() {
            return;
        }
        let context_window = resolve_context_window(
            self.model.context_window,
            &self.model.provider,
            &self.model.name,
        );
        let window = self.agent.compress_window.max(2);
        match self.agent.compress_strategy.as_str() {
            "summary" => {
                use crate::compression::compressor::SummaryCompressor;
                match agent.llm_client().cloned() {
                    Some(llm) => {
                        agent
                            .set_compressor(SummaryCompressor::new(llm, window))
                            .await
                    }
                    None => {
                        tracing::warn!(
                            "summary compression requires an LLM client; using sliding window"
                        );
                        agent
                            .set_compressor(SlidingWindowCompressor::new(window))
                            .await;
                    }
                }
            }
            "hybrid" => {
                use crate::compression::compressor::HybridCompressor;
                match agent.llm_client().cloned() {
                    Some(llm) => {
                        agent
                            .set_compressor(HybridCompressor::summary_buffer(llm, window))
                            .await;
                    }
                    None => {
                        tracing::warn!(
                            "hybrid compression requires an LLM client; using sliding window"
                        );
                        agent
                            .set_compressor(SlidingWindowCompressor::new(window))
                            .await;
                    }
                }
            }
            "adaptive" => {
                use crate::compression::levels::{
                    AdaptiveCompressionConfig, AdaptiveCompressor, tune_for_model,
                };
                let mut config = AdaptiveCompressionConfig::default();
                tune_for_model(&mut config, context_window);
                agent.set_compressor(AdaptiveCompressor::new(config)).await;
            }
            "sliding" | "" => {
                agent
                    .set_compressor(SlidingWindowCompressor::new(window))
                    .await;
            }
            other => {
                tracing::warn!(
                    strategy = other,
                    "unknown compression strategy; using sliding"
                );
                agent
                    .set_compressor(SlidingWindowCompressor::new(window))
                    .await;
            }
        }
    }
}

/// Direct model settings chosen by an embedding application.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct ModelConfig {
    pub provider: String,
    pub name: String,
    pub auth_token: Option<String>,
    pub base_url: Option<String>,
    pub api_protocol: Option<LlmApiProtocol>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub context_window: Option<u32>,
}

impl ModelConfig {
    pub fn get_auth_token(&self) -> Option<String> {
        self.auth_token.clone().filter(|value| !value.is_empty())
    }

    pub fn get_base_url(&self) -> Option<String> {
        self.base_url.clone().filter(|value| !value.is_empty())
    }

    pub fn get_model_name(&self) -> String {
        self.name.clone()
    }
}

/// Serializable provider-neutral Agent settings.
///
/// The value is format-independent; applications may load it from YAML, JSON,
/// environment variables, or another configuration source.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct AgentSettings {
    pub name: String,
    pub system_prompt: String,
    pub max_iterations: usize,
    pub enable_tools: bool,
    pub enable_memory: bool,
    pub enable_human_in_loop: bool,
    pub memory_path: String,
    pub tool_timeout_ms: u64,
    pub max_tool_output_tokens: usize,
    pub token_limit: usize,
    pub compress_strategy: String,
    pub compress_window: usize,
    pub subagent_timeout_secs: u64,
}

impl Default for AgentSettings {
    fn default() -> Self {
        Self {
            name: "assistant".to_string(),
            system_prompt: DEFAULT_AGENT_SYSTEM_PROMPT.to_string(),
            max_iterations: 10,
            enable_tools: false,
            enable_memory: false,
            enable_human_in_loop: false,
            memory_path: String::new(),
            tool_timeout_ms: 120_000,
            max_tool_output_tokens: 0,
            token_limit: 0,
            compress_strategy: String::new(),
            compress_window: 20,
            subagent_timeout_secs: 600,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_do_not_choose_product_or_persistence_policy() {
        let config = FrameworkConfig::default();
        assert_eq!(config.agent.system_prompt, DEFAULT_AGENT_SYSTEM_PROMPT);
        assert!(!config.agent.enable_memory);
        assert!(config.agent.memory_path.is_empty());
        assert!(config.model.name.is_empty());
    }
}
