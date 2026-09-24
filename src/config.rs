//! Provider-neutral framework configuration.
//!
//! File discovery, product prompts, model catalogs, channels, UI settings, and
//! server settings belong to embedding applications. The framework accepts a
//! typed configuration value and explicit paths.

use crate::agent::AgentConfig;
use echo_core::budget::TokenBudgetConfig;
use echo_core::llm::LlmApiProtocol;
use echo_core::llm::capabilities::{
    ModelFactConfidence, ModelFactInputs, ModelFactMetadata, ModelFactSet, ModelFactSource,
    ModelProfileOverride, ModelProfileResolution, ModelProfileResolver, ProviderCapabilityOverride,
};
use serde::{Deserialize, Serialize};
use std::time::SystemTime;

pub const DEFAULT_AGENT_SYSTEM_PROMPT: &str = "You are a helpful assistant.";

/// Reusable configuration for one Agent runtime.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct FrameworkConfig {
    pub model: ModelConfig,
    pub agent: AgentSettings,
}

fn resolve_context_window(resolution: &ModelProfileResolution) -> usize {
    resolution
        .profile
        .context_window
        .unwrap_or(u32::try_from(crate::agent::config::DEFAULT_TOKEN_LIMIT).unwrap_or(128_000))
        .clamp(1, 10_000_000) as usize
}

impl From<FrameworkConfig> for AgentConfig {
    fn from(value: FrameworkConfig) -> Self {
        let FrameworkConfig { model, agent } = value;
        let resolution = model.resolve_model_profile_at(SystemTime::now());
        let resolved_context_window = resolution.profile.context_window;
        let context_window = resolve_context_window(&resolution);
        let token_budget_config = if resolved_context_window.is_some() || agent.token_limit > 0 {
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
            .enable_subagent(agent.enable_subagent)
            .register_agent_dispatch_tool(agent.register_agent_dispatch_tool)
            .max_iterations(agent.max_iterations)
            .subagent_timeout_secs(agent.subagent_timeout_secs)
            .memory_path(&agent.memory_path)
            .temperature(model.temperature)
            .max_tokens(model.max_tokens)
            .model_profile_resolution(resolution)
            .token_budget(token_budget_config)
            .tool_execution(crate::tools::ToolExecutionConfig {
                timeout_ms: agent.tool_timeout_ms,
                ..Default::default()
            });
        if agent.token_limit > 0 {
            config = config.token_limit(agent.token_limit);
        }
        if agent.max_tool_output_tokens > 0 {
            config = config.max_tool_output_tokens(agent.max_tool_output_tokens);
        }
        config
    }
}

impl FrameworkConfig {
    pub fn has_compressor(&self) -> bool {
        self.agent.token_limit > 0
            || self
                .model
                .resolve_model_profile_at(SystemTime::now())
                .profile
                .context_window
                .is_some()
            || !self.agent.compress_strategy.is_empty()
    }

    pub async fn apply_compressor(&self, agent: &crate::agent::ReactAgent) {
        use crate::compression::compressor::SlidingWindowCompressor;

        if !self.has_compressor() {
            return;
        }
        let resolution = self.model.resolve_model_profile_at(SystemTime::now());
        let context_window = resolve_context_window(&resolution);
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

    /// Resolve serialized model facts and legacy explicit fields at `now`.
    pub fn resolve_model_profile_at(&self, now: SystemTime) -> ModelProfileResolution {
        self.resolve_model_profile_with_facts_at(&ModelFactInputs::default(), now)
    }

    /// Attach serialized model facts without changing the existing config shape.
    pub fn with_model_facts(self, facts: ModelFactInputs) -> SourcedModelConfig {
        SourcedModelConfig {
            config: self,
            facts,
        }
    }

    fn resolve_model_profile_with_facts_at(
        &self,
        facts: &ModelFactInputs,
        now: SystemTime,
    ) -> ModelProfileResolution {
        let protocol = self.api_protocol.unwrap_or(LlmApiProtocol::ChatCompletions);
        let mut resolver =
            facts.register_with(ModelProfileResolver::new(), &self.provider, &self.name);
        if self.api_protocol.is_some() {
            resolver = resolver.register_protocol_facts(
                &self.provider,
                ModelFactSet::new_partial(
                    ModelFactMetadata::new(
                        ModelFactSource::ProviderAdapter,
                        "FrameworkConfig::ModelConfig.api_protocol",
                        "framework-model-config-v1",
                        now,
                        None,
                        ModelFactConfidence::VERIFIED,
                    ),
                    ProviderCapabilityOverride::for_protocol(protocol),
                    ModelProfileOverride {
                        supports_streaming: Some(true),
                        ..Default::default()
                    },
                ),
            );
        }
        for facts in self.explicit_model_facts_at(now) {
            resolver = resolver.register_explicit_override(&self.provider, &self.name, facts);
        }
        resolver.resolve_for_protocol_at(
            &self.provider,
            &self.name,
            protocol,
            self.base_url.as_deref(),
            now,
        )
    }

    /// Transfer serialized facts into the provider client configuration so
    /// dynamic capability reads and run snapshots preserve application policy.
    pub fn apply_to_llm_config(
        &self,
        config: crate::llm::LlmConfig,
    ) -> crate::llm::SourcedLlmConfig {
        let mut config = crate::llm::SourcedLlmConfig::new(config);
        for facts in self.explicit_model_facts_at(SystemTime::now()) {
            config = config.with_model_override(facts);
        }
        config
    }

    fn explicit_model_facts_at(&self, now: SystemTime) -> Vec<ModelFactSet> {
        let mut explicit = Vec::new();
        if let Some(context_window) = self.context_window {
            explicit.push(ModelFactSet::new(
                ModelFactMetadata::new(
                    ModelFactSource::CallerOverride,
                    "FrameworkConfig::ModelConfig.context_window",
                    "legacy-model-config-v1",
                    now,
                    None,
                    ModelFactConfidence::VERIFIED,
                ),
                None,
                ModelProfileOverride {
                    context_window: Some(context_window),
                    ..Default::default()
                },
            ));
        }
        explicit
    }
}

/// Serializable model-fact sidecar for an existing [`ModelConfig`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourcedModelConfig {
    pub config: ModelConfig,
    #[serde(default)]
    pub facts: ModelFactInputs,
}

impl SourcedModelConfig {
    pub fn resolve_model_profile_at(&self, now: SystemTime) -> ModelProfileResolution {
        self.config
            .resolve_model_profile_with_facts_at(&self.facts, now)
    }

    pub fn apply_to_llm_config(
        &self,
        config: crate::llm::LlmConfig,
    ) -> crate::llm::SourcedLlmConfig {
        let mut sourced = crate::llm::SourcedLlmConfig {
            config,
            facts: self.facts.clone(),
        };
        for facts in self.config.explicit_model_facts_at(SystemTime::now()) {
            sourced = sourced.with_model_override(facts);
        }
        sourced
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
    /// Enable framework-level Subagent registration and dispatch support.
    pub enable_subagent: bool,
    /// Expose the model-callable `agent_tool` dispatcher.
    pub register_agent_dispatch_tool: bool,
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
            enable_subagent: false,
            register_agent_dispatch_tool: false,
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

    #[test]
    fn legacy_context_window_becomes_a_retained_explicit_fact() -> std::result::Result<(), String> {
        let model = ModelConfig {
            provider: "custom".to_string(),
            name: "future-model".to_string(),
            api_protocol: Some(LlmApiProtocol::ChatCompletions),
            context_window: Some(8_192),
            ..ModelConfig::default()
        };
        let stale = ModelFactSet::new(
            ModelFactMetadata::new(
                ModelFactSource::CallerOverride,
                "stale-config",
                "stale-v1",
                SystemTime::UNIX_EPOCH,
                Some(SystemTime::UNIX_EPOCH),
                ModelFactConfidence::VERIFIED,
            ),
            None,
            ModelProfileOverride {
                context_window: Some(1_000_000),
                ..Default::default()
            },
        );
        let now = SystemTime::now();
        let resolution = model
            .clone()
            .with_model_facts(ModelFactInputs {
                model_overrides: vec![stale],
                ..Default::default()
            })
            .resolve_model_profile_at(now);
        assert_eq!(resolution.profile.context_window, Some(8_192));
        assert!(resolution.applied_facts.iter().any(|metadata| {
            metadata.source == ModelFactSource::CallerOverride
                && metadata.provenance == "FrameworkConfig::ModelConfig.context_window"
        }));
        assert!(
            resolution
                .ignored_stale_facts
                .iter()
                .any(|metadata| metadata.version == "stale-v1")
        );

        let config = FrameworkConfig {
            model,
            agent: AgentSettings::default(),
        };
        let agent_config = AgentConfig::from(config);
        assert_eq!(
            agent_config.token_limit,
            crate::agent::config::DEFAULT_TOKEN_LIMIT
        );
        assert!(!agent_config.token_limit_explicit);
        assert!(agent_config.token_budget_config.enabled);
        assert!(agent_config.model_profile.as_ref().is_some_and(|receipt| {
            receipt
                .applied_facts
                .iter()
                .any(|metadata| metadata.source == ModelFactSource::CallerOverride)
        }));
        let agent = crate::agent::ReactAgent::new(agent_config);
        let snapshot = crate::agent::snapshot::AgentRunSnapshot::from_agent(&agent);
        assert_eq!(snapshot.config.token_limit, 8_192);
        Ok(())
    }

    #[test]
    fn model_config_fact_inputs_round_trip_without_legacy_fields() -> std::result::Result<(), String>
    {
        let now = SystemTime::now();
        let config = ModelConfig {
            provider: "custom".to_string(),
            name: "future-model".to_string(),
            api_protocol: Some(LlmApiProtocol::Responses),
            ..ModelConfig::default()
        }
        .with_model_facts(ModelFactInputs {
            exact_model_facts: Some(ModelFactSet::new(
                ModelFactMetadata::new(
                    ModelFactSource::ExactModel,
                    "provider:/models/future-model",
                    "model-v4",
                    now,
                    None,
                    ModelFactConfidence::from_percent_saturating(85),
                ),
                None,
                ModelProfileOverride {
                    context_window: Some(32_000),
                    ..Default::default()
                },
            )),
            ..Default::default()
        });
        let encoded = serde_json::to_string(&config).map_err(|error| error.to_string())?;
        let decoded = serde_json::from_str::<SourcedModelConfig>(&encoded)
            .map_err(|error| error.to_string())?;
        let resolution = decoded.resolve_model_profile_at(now);
        assert_eq!(resolution.profile.context_window, Some(32_000));
        assert!(
            resolution
                .applied_facts
                .iter()
                .any(|metadata| metadata.version == "model-v4")
        );
        Ok(())
    }
}
