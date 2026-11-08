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
        // A bounded recent tail scales to the live Agent, including a selected
        // runtime model rather than only the bootstrap model config.
        let recent_tokens = if self.agent.compress_window == 0 {
            agent
                .config()
                .get_token_limit()
                .saturating_div(4)
                .clamp(1, 20_000)
        } else {
            0
        }; // Positive windows retain the explicitly selected message count cap.
        match self.agent.compress_strategy.as_str() {
            "summary" => {
                use crate::compression::compressor::SummaryCompressor;
                match agent.llm_client().cloned() {
                    Some(llm) => {
                        agent
                            .set_compressor(
                                SummaryCompressor::new(llm, window)
                                    .with_recent_token_budget(recent_tokens),
                            )
                            .await
                    }
                    None => {
                        tracing::warn!(
                            "summary compression requires an LLM client; using sliding window"
                        );
                        agent
                            .set_compressor(
                                SlidingWindowCompressor::new(window)
                                    .with_recent_token_budget(recent_tokens),
                            )
                            .await;
                    }
                }
            }
            "hybrid" => {
                use crate::compression::compressor::HybridCompressor;
                match agent.llm_client().cloned() {
                    Some(llm) => {
                        agent
                            .set_compressor(
                                HybridCompressor::builder()
                                    .stage(
                                        crate::compression::compressor::SummaryCompressor::new(
                                            llm, window,
                                        )
                                        .with_recent_token_budget(recent_tokens),
                                    )
                                    .stage(
                                        SlidingWindowCompressor::new(window)
                                            .with_recent_token_budget(recent_tokens),
                                    )
                                    .build(),
                            )
                            .await;
                    }
                    None => {
                        tracing::warn!(
                            "hybrid compression requires an LLM client; using sliding window"
                        );
                        agent
                            .set_compressor(
                                SlidingWindowCompressor::new(window)
                                    .with_recent_token_budget(recent_tokens),
                            )
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
                    .set_compressor(
                        SlidingWindowCompressor::new(window)
                            .with_recent_token_budget(recent_tokens),
                    )
                    .await;
            }
            other => {
                tracing::warn!(
                    strategy = other,
                    "unknown compression strategy; using sliding"
                );
                agent
                    .set_compressor(
                        SlidingWindowCompressor::new(window)
                            .with_recent_token_budget(recent_tokens),
                    )
                    .await;
            }
        }
    }
}

/// Direct model settings chosen by an embedding application.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ModelConfig {
    pub provider: String,
    pub name: String,
    pub auth_token: Option<String>,
    pub base_url: Option<String>,
    pub api_protocol: Option<LlmApiProtocol>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    /// 当前调用方直接选择的window；仍转为同一个CallerOverride事实，None只清除此直接覆盖。
    pub context_window: Option<u32>,
    /// 当前序列化来源事实，与直接设置共同进入同一个resolver，不使用旧config/facts侧车。
    pub facts: ModelFactInputs,
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

    /// Resolve this one typed configuration and all retained source facts at `now`.
    pub fn resolve_model_profile_at(&self, now: SystemTime) -> ModelProfileResolution {
        let protocol = self.api_protocol.unwrap_or(LlmApiProtocol::ChatCompletions);
        let mut resolver =
            self.facts
                .register_with(ModelProfileResolver::new(), &self.provider, &self.name);
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

    /// Replace the serialized fact inputs of this current configuration.
    /// Clearing them does not erase the independent direct context_window selection.
    pub fn with_model_facts(mut self, facts: ModelFactInputs) -> Self {
        self.facts = facts;
        self
    }

    /// Transfer serialized facts into the provider client configuration so
    /// dynamic capability reads and run snapshots preserve application policy.
    pub fn apply_to_llm_config(
        &self,
        config: crate::llm::LlmConfig,
    ) -> crate::llm::SourcedLlmConfig {
        let mut config = crate::llm::SourcedLlmConfig {
            config,
            facts: self.facts.clone(),
        };
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
    fn current_context_window_remains_a_retained_explicit_fact() -> std::result::Result<(), String>
    {
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
    fn model_config_fact_inputs_round_trip_in_one_current_shape() -> std::result::Result<(), String>
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
        let decoded =
            serde_json::from_str::<ModelConfig>(&encoded).map_err(|error| error.to_string())?;
        let resolution = decoded.resolve_model_profile_at(now);
        assert_eq!(resolution.profile.context_window, Some(32_000));
        assert!(
            resolution
                .applied_facts
                .iter()
                .any(|metadata| metadata.version == "model-v4")
        );
        let agent_config = AgentConfig::from(FrameworkConfig {
            model: decoded,
            agent: AgentSettings::default(),
        });
        assert!(agent_config.model_profile.as_ref().is_some_and(|receipt| {
            receipt.applied_facts.iter().any(|fact| {
                fact.version == "model-v4" && fact.provenance == "provider:/models/future-model"
            })
        }));
        let agent = crate::agent::ReactAgent::new(agent_config);
        let snapshot = crate::agent::snapshot::AgentRunSnapshot::from_agent(&agent);
        assert_eq!(snapshot.config.token_limit, 32_000);
        Ok(())
    }

    #[test]
    fn current_model_facts_and_direct_window_have_independent_clear_semantics()
    -> std::result::Result<(), String> {
        let now = SystemTime::now();
        let facts = ModelFactInputs {
            exact_model_facts: Some(ModelFactSet::new(
                ModelFactMetadata::new(
                    ModelFactSource::ExactModel,
                    "current:model-catalog",
                    "current-facts-v1",
                    now,
                    None,
                    ModelFactConfidence::VERIFIED,
                ),
                None,
                ModelProfileOverride {
                    context_window: Some(32_000),
                    ..Default::default()
                },
            )),
            ..Default::default()
        };
        let mut model = ModelConfig {
            provider: "custom".into(),
            name: "future-model".into(),
            api_protocol: Some(LlmApiProtocol::Responses),
            context_window: Some(8_192),
            ..Default::default()
        }
        .with_model_facts(facts);
        let explicit = model.resolve_model_profile_at(now);
        assert_eq!(explicit.profile.context_window, Some(8_192));
        assert!(
            explicit
                .applied_facts
                .iter()
                .any(|fact| fact.source == ModelFactSource::CallerOverride
                    && fact.provenance == "FrameworkConfig::ModelConfig.context_window")
        );
        model.context_window = None;
        let sourced = model.resolve_model_profile_at(now);
        assert_eq!(sourced.profile.context_window, Some(32_000));
        assert!(
            sourced
                .applied_facts
                .iter()
                .any(|fact| fact.provenance == "current:model-catalog"
                    && fact.version == "current-facts-v1")
        );
        model = model.with_model_facts(ModelFactInputs::default());
        let cleared = model.resolve_model_profile_at(now);
        assert!(
            cleared
                .applied_facts
                .iter()
                .all(|fact| fact.provenance != "current:model-catalog")
        );
        model.context_window = Some(4_096);
        let direct = model
            .with_model_facts(ModelFactInputs::default())
            .resolve_model_profile_at(now);
        assert_eq!(direct.profile.context_window, Some(4_096));
        Ok(())
    }

    #[test]
    fn one_model_config_shape_rejects_the_retired_nested_sidecar() -> std::result::Result<(), String>
    {
        let model = ModelConfig {
            provider: "custom".into(),
            name: "future-model".into(),
            ..Default::default()
        }
        .with_model_facts(ModelFactInputs::default());
        let encoded = serde_json::to_value(&model).map_err(|error| error.to_string())?;
        assert!(encoded.get("facts").is_some());
        assert!(encoded.get("config").is_none());
        let old = serde_json::json!({"config":encoded,"facts":{}});
        assert!(serde_json::from_value::<ModelConfig>(old).is_err());
        Ok(())
    }
    #[test]
    fn current_fact_sources_reach_llm_config_without_a_sidecar() -> std::result::Result<(), String>
    {
        let now = SystemTime::now();
        let facts = ModelFactInputs {
            exact_model_facts: Some(ModelFactSet::new(
                ModelFactMetadata::new(
                    ModelFactSource::ExactModel,
                    "current:provider-model",
                    "source-v2",
                    now,
                    None,
                    ModelFactConfidence::VERIFIED,
                ),
                None,
                ModelProfileOverride {
                    context_window: Some(32_000),
                    ..Default::default()
                },
            )),
            ..Default::default()
        };
        let model = ModelConfig {
            provider: "custom".into(),
            name: "future-model".into(),
            api_protocol: Some(LlmApiProtocol::ChatCompletions),
            ..Default::default()
        }
        .with_model_facts(facts.clone());
        let llm = crate::llm::LlmConfig::for_provider(
            "custom",
            "https://example.invalid/v1",
            "",
            "future-model",
            LlmApiProtocol::ChatCompletions,
        )
        .map_err(|error| error.to_string())?;
        let sourced = model.apply_to_llm_config(llm.clone());
        assert_eq!(
            serde_json::to_value(&sourced.facts).map_err(|error| error.to_string())?,
            serde_json::to_value(&facts).map_err(|error| error.to_string())?
        );
        let mut direct = model;
        direct.context_window = Some(8_192);
        let sourced = direct.apply_to_llm_config(llm);
        assert!(
            sourced
                .facts
                .exact_model_facts
                .as_ref()
                .is_some_and(|fact| fact.metadata.provenance == "current:provider-model"
                    && fact.metadata.version == "source-v2")
        );
        assert!(
            sourced
                .facts
                .model_overrides
                .iter()
                .any(
                    |fact| fact.metadata.source == ModelFactSource::CallerOverride
                        && fact.metadata.provenance
                            == "FrameworkConfig::ModelConfig.context_window"
                )
        );
        Ok(())
    }
}
