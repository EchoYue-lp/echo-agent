//! Provider-neutral runtime configuration for LLM clients.
//!
//! Provider catalogs, credentials, and model selection belong to the consuming
//! application. This module only turns an explicit provider/model contract into
//! a concrete wire client and validates that requests respect model capabilities.

use echo_core::error::{ConfigError, LlmError, Result};
use echo_core::llm::capabilities::{
    ModelFactConfidence, ModelFactInputs, ModelFactMetadata, ModelFactSet, ModelFactSource,
    ModelProfileOverride, ModelProfileResolution, ModelProfileResolver, ProviderCapabilityOverride,
};
use echo_core::llm::types::{ContentPart, Message, MessageContent};
use echo_core::llm::{LlmApiProtocol, LlmTimeouts, ModelInputModality, ThinkingProtocol};
use serde::{Deserialize, Serialize};
use std::time::SystemTime;

/// Resolve a provider API root or complete endpoint for one explicit protocol.
///
/// A recognized complete endpoint is preserved when it already matches, or
/// replaced when a different protocol is selected under the same provider.
pub fn resolve_protocol_endpoint(base_url: &str, protocol: LlmApiProtocol) -> Result<String> {
    let trimmed = base_url.trim();
    if trimmed.is_empty() {
        return Err(ConfigError::ConfigFileError(
            "provider base_url must not be empty".to_string(),
        )
        .into());
    }

    let mut url = url::Url::parse(trimmed).map_err(|error| {
        ConfigError::ConfigFileError(format!("invalid provider base_url '{trimmed}': {error}"))
    })?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(ConfigError::ConfigFileError(format!(
            "provider base_url must use http or https: '{trimmed}'"
        ))
        .into());
    }

    let existing_protocol = LlmApiProtocol::try_from_endpoint(url.as_str());
    if existing_protocol == Some(protocol) {
        return Ok(url.to_string());
    }

    let segments_to_remove = match existing_protocol {
        Some(LlmApiProtocol::ChatCompletions) => 2,
        Some(LlmApiProtocol::Responses | LlmApiProtocol::Anthropic) => 1,
        None => 0,
    };
    let mut segments = url.path_segments_mut().map_err(|()| {
        ConfigError::ConfigFileError(format!(
            "provider base_url cannot be used as an API root: '{trimmed}'"
        ))
    })?;
    segments.pop_if_empty();
    for _ in 0..segments_to_remove {
        segments.pop();
    }
    for segment in protocol.endpoint_path().split('/') {
        segments.push(segment);
    }
    drop(segments);
    Ok(url.to_string())
}

/// Fully resolved model configuration injected by an application.
#[derive(Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    /// Stable application-defined provider id used for model capability policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_name: Option<String>,
    /// Wire protocol selected for this model.
    pub api_protocol: LlmApiProtocol,
    /// Complete endpoint URL for [`Self::api_protocol`].
    pub base_url: String,
    /// Resolved API credential. May be empty for local providers.
    pub api_key: String,
    /// Provider-facing model name.
    pub model: String,
    /// Input types accepted by this model. Pure text is the default.
    #[serde(default = "ModelInputModality::text_only")]
    pub input_modalities: Vec<ModelInputModality>,
    /// Thinking wire dialect resolved centrally from the runtime contract.
    /// This is a compatibility projection; request paths resolve the retained
    /// sourced facts again at their invocation boundary.
    #[serde(default)]
    pub thinking_protocol: ThinkingProtocol,
    /// Request and streaming timeout policy shared by every provider transport.
    #[serde(default)]
    pub timeouts: LlmTimeouts,
}

impl std::fmt::Debug for LlmConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LlmConfig")
            .field("provider_name", &self.provider_name)
            .field("api_protocol", &self.api_protocol)
            .field("base_url", &self.base_url)
            .field("api_key", &"[REDACTED]")
            .field("model", &self.model)
            .field("input_modalities", &self.input_modalities)
            .field("thinking_protocol", &self.thinking_protocol)
            .field("timeouts", &self.timeouts)
            .finish()
    }
}

impl LlmConfig {
    /// Build a runtime model config from an explicit provider contract.
    pub fn for_provider(
        provider_name: impl Into<String>,
        base_url: impl AsRef<str>,
        api_key: impl Into<String>,
        model: impl Into<String>,
        api_protocol: LlmApiProtocol,
    ) -> Result<Self> {
        let provider_name = provider_name.into();
        let model = model.into();
        if model.trim().is_empty() {
            return Err(ConfigError::MissingConfig(
                "model".to_string(),
                "model name must not be empty".to_string(),
            )
            .into());
        }
        let base_url = resolve_protocol_endpoint(base_url.as_ref(), api_protocol)?;
        let observed_at = SystemTime::now();
        let mut config = Self {
            provider_name: (!provider_name.trim().is_empty()).then_some(provider_name),
            api_protocol,
            base_url,
            api_key: api_key.into(),
            model,
            input_modalities: ModelInputModality::text_only(),
            thinking_protocol: ThinkingProtocol::None,
            timeouts: LlmTimeouts::default(),
        };
        config.refresh_thinking_protocol(observed_at);
        Ok(config)
    }

    /// Set the concrete model's accepted input modalities.
    pub fn with_input_modalities(mut self, input_modalities: Vec<ModelInputModality>) -> Self {
        self.input_modalities = normalize_input_modalities(input_modalities);
        self
    }

    /// Set the timeout policy used by the concrete provider client.
    pub fn with_timeouts(mut self, timeouts: LlmTimeouts) -> Self {
        self.timeouts = timeouts;
        self
    }

    /// Install fresh provider-wide facts.
    pub fn with_provider_facts(self, facts: ModelFactSet) -> SourcedLlmConfig {
        SourcedLlmConfig::new(self).with_provider_facts(facts)
    }

    /// Install fresh facts for this exact provider/model identity.
    pub fn with_exact_model_facts(self, facts: ModelFactSet) -> SourcedLlmConfig {
        SourcedLlmConfig::new(self).with_exact_model_facts(facts)
    }

    /// Install an exact caller override.
    pub fn with_model_override(self, facts: ModelFactSet) -> SourcedLlmConfig {
        SourcedLlmConfig::new(self).with_model_override(facts)
    }

    /// Resolve all configured facts at an explicit time.
    pub fn resolve_model_profile_at(&self, now: SystemTime) -> ModelProfileResolution {
        let provider = self.provider_name.as_deref().unwrap_or("");
        self.model_profile_resolver_at(now, &ModelFactInputs::default())
            .resolve_for_protocol_at(
                provider,
                &self.model,
                self.api_protocol,
                Some(&self.base_url),
                now,
            )
    }

    pub(crate) fn model_profile_resolver_at(
        &self,
        now: SystemTime,
        facts: &ModelFactInputs,
    ) -> ModelProfileResolver {
        let provider = self.provider_name.as_deref().unwrap_or("");
        facts.register_with(
            ModelProfileResolver::new()
                .register_protocol_facts(provider, protocol_fact_set(self.api_protocol, now)),
            provider,
            &self.model,
        )
    }

    fn refresh_thinking_protocol(&mut self, now: SystemTime) {
        self.thinking_protocol = self.resolve_model_profile_at(now).profile.thinking_protocol;
    }

    /// Build the wire client selected by [`Self::api_protocol`].
    pub fn build_client(&self) -> Result<Box<dyn echo_core::llm::LlmClient>> {
        SourcedLlmConfig::new(self.clone()).build_client()
    }

    pub(crate) fn build_client_with_resolver(
        &self,
        resolver: ModelProfileResolver,
    ) -> Result<Box<dyn echo_core::llm::LlmClient>> {
        match self.api_protocol {
            LlmApiProtocol::Responses => Ok(Box::new(
                super::responses::ResponsesClient::new(self.clone())?
                    .with_model_profile_resolver(resolver),
            )),
            LlmApiProtocol::ChatCompletions => Ok(Box::new(
                super::openai::OpenAiClient::new(self.clone())?
                    .with_model_profile_resolver(resolver),
            )),
            LlmApiProtocol::Anthropic => Ok(Box::new(
                super::anthropic::AnthropicClient::with_base_url(
                    &self.base_url,
                    &self.api_key,
                    &self.model,
                )
                .with_model_profile_resolver(
                    self.provider_name.clone().unwrap_or_default(),
                    resolver,
                )
                .with_input_modalities(self.input_modalities.clone())
                .with_timeouts(self.timeouts),
            )),
        }
    }

    pub(crate) fn validate_input_modalities(&self, messages: &[Message]) -> Result<()> {
        validate_model_input_modalities(&self.model, &self.input_modalities, messages)
    }
}

/// Serializable model-fact sidecar for an existing [`LlmConfig`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourcedLlmConfig {
    pub config: LlmConfig,
    #[serde(default)]
    pub facts: ModelFactInputs,
}

impl SourcedLlmConfig {
    pub fn new(config: LlmConfig) -> Self {
        Self {
            config,
            facts: ModelFactInputs::default(),
        }
    }

    pub fn with_provider_facts(mut self, facts: ModelFactSet) -> Self {
        self.facts.provider_facts = Some(facts);
        self
    }

    pub fn with_exact_model_facts(mut self, facts: ModelFactSet) -> Self {
        self.facts.exact_model_facts = Some(facts);
        self
    }

    pub fn with_model_override(mut self, facts: ModelFactSet) -> Self {
        self.facts.model_overrides.push(facts);
        self
    }

    pub fn resolve_model_profile_at(&self, now: SystemTime) -> ModelProfileResolution {
        let provider = self.config.provider_name.as_deref().unwrap_or("");
        self.config
            .model_profile_resolver_at(now, &self.facts)
            .resolve_for_protocol_at(
                provider,
                &self.config.model,
                self.config.api_protocol,
                Some(&self.config.base_url),
                now,
            )
    }

    pub fn build_client(&self) -> Result<Box<dyn echo_core::llm::LlmClient>> {
        self.config.build_client_with_resolver(
            self.config
                .model_profile_resolver_at(SystemTime::now(), &self.facts),
        )
    }
}

pub(crate) fn protocol_fact_set(
    api_protocol: LlmApiProtocol,
    observed_at: SystemTime,
) -> ModelFactSet {
    ModelFactSet::new_partial(
        ModelFactMetadata::new(
            ModelFactSource::ProviderAdapter,
            format!("echo-integration::{api_protocol:?}"),
            env!("CARGO_PKG_VERSION"),
            observed_at,
            None,
            ModelFactConfidence::VERIFIED,
        ),
        ProviderCapabilityOverride::for_protocol(api_protocol),
        ModelProfileOverride {
            supports_streaming: Some(true),
            ..Default::default()
        },
    )
}

fn normalize_input_modalities(
    input_modalities: Vec<ModelInputModality>,
) -> Vec<ModelInputModality> {
    let mut normalized = ModelInputModality::text_only();
    for modality in input_modalities {
        if !normalized.contains(&modality) {
            normalized.push(modality);
        }
    }
    normalized
}

pub(crate) fn validate_model_input_modalities(
    model: &str,
    input_modalities: &[ModelInputModality],
    messages: &[Message],
) -> Result<()> {
    for part in messages
        .iter()
        .filter_map(|message| match &message.content {
            MessageContent::Parts(parts) => Some(parts.as_slice()),
            MessageContent::Text(_) | MessageContent::Empty => None,
        })
        .flatten()
    {
        let modality = match part {
            ContentPart::ImageUrl { .. } => Some(ModelInputModality::Image),
            ContentPart::File { name, .. } => file_input_modality(name),
            ContentPart::ResourceLink { .. } => None,
            ContentPart::Text { .. } => None,
        };
        if let Some(modality) = modality.filter(|value| !input_modalities.contains(value)) {
            let label = match modality {
                ModelInputModality::Text => "text",
                ModelInputModality::Image => "image",
                ModelInputModality::Audio => "audio",
                ModelInputModality::Video => "video",
            };
            return Err(LlmError::InvalidResponse(format!(
                "model '{model}' is not configured for {label} input"
            ))
            .into());
        }
    }
    Ok(())
}

fn file_input_modality(name: &str) -> Option<ModelInputModality> {
    let extension = std::path::Path::new(name)
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .map(str::to_ascii_lowercase)?;
    match extension.as_str() {
        "apng" | "avif" | "bmp" | "gif" | "heic" | "heif" | "jpeg" | "jpg" | "png" | "tif"
        | "tiff" | "webp" => Some(ModelInputModality::Image),
        "aac" | "aiff" | "alac" | "flac" | "m4a" | "mp3" | "ogg" | "opus" | "wav" => {
            Some(ModelInputModality::Audio)
        }
        "avi" | "m4v" | "mkv" | "mov" | "mp4" | "mpeg" | "mpg" | "webm" => {
            Some(ModelInputModality::Video)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_endpoint_resolution_supports_one_root_with_multiple_protocols()
    -> std::result::Result<(), String> {
        let root = "https://gateway.example/api/v1?tenant=eko";
        assert_eq!(
            resolve_protocol_endpoint(root, LlmApiProtocol::Responses)
                .map_err(|error| error.to_string())?,
            "https://gateway.example/api/v1/responses?tenant=eko"
        );
        assert_eq!(
            resolve_protocol_endpoint(root, LlmApiProtocol::ChatCompletions)
                .map_err(|error| error.to_string())?,
            "https://gateway.example/api/v1/chat/completions?tenant=eko"
        );
        assert_eq!(
            resolve_protocol_endpoint(root, LlmApiProtocol::Anthropic)
                .map_err(|error| error.to_string())?,
            "https://gateway.example/api/v1/messages?tenant=eko"
        );
        Ok(())
    }

    #[test]
    fn provider_config_defaults_to_text_and_preserves_explicit_capabilities()
    -> std::result::Result<(), String> {
        let text = LlmConfig::for_provider(
            "custom",
            "https://gateway.example/v1",
            "",
            "text-model",
            LlmApiProtocol::ChatCompletions,
        )
        .map_err(|error| error.to_string())?;
        assert_eq!(text.input_modalities, ModelInputModality::text_only());
        assert_eq!(text.timeouts, LlmTimeouts::default());

        let timeouts =
            LlmTimeouts::default().with_first_chunk_timeout(std::time::Duration::from_secs(15));
        let multimodal = text
            .with_input_modalities(vec![
                ModelInputModality::Image,
                ModelInputModality::Audio,
                ModelInputModality::Video,
            ])
            .with_timeouts(timeouts);
        assert_eq!(
            multimodal.input_modalities,
            ModelInputModality::all_supported()
        );
        assert_eq!(multimodal.timeouts, timeouts);
        Ok(())
    }

    #[test]
    fn provider_config_resolves_thinking_without_user_fields() -> std::result::Result<(), String> {
        let config = LlmConfig::for_provider(
            "openai",
            "https://api.openai.com/v1",
            "test-key",
            "gpt-5.6-sol",
            LlmApiProtocol::Responses,
        )
        .map_err(|error| error.to_string())?;
        assert_eq!(
            config.thinking_protocol,
            ThinkingProtocol::OpenaiReasoningEffort
        );
        Ok(())
    }

    #[test]
    fn serialized_model_facts_drive_thinking_and_reject_stale_budget_facts()
    -> std::result::Result<(), String> {
        let now = SystemTime::now();
        let fresh = ModelFactSet::new(
            ModelFactMetadata::new(
                ModelFactSource::ExactModel,
                "provider:/models/future-model",
                "etag-v2",
                now,
                now.checked_add(std::time::Duration::from_secs(60)),
                ModelFactConfidence::from_percent_saturating(90),
            ),
            None,
            ModelProfileOverride {
                context_window: Some(64_000),
                thinking_protocol: Some(ThinkingProtocol::ModelManaged),
                ..Default::default()
            },
        );
        let config = LlmConfig::for_provider(
            "custom",
            "https://gateway.example/v1",
            "test-key",
            "future-model",
            LlmApiProtocol::ChatCompletions,
        )
        .map_err(|error| error.to_string())?
        .with_exact_model_facts(fresh);
        let encoded = serde_json::to_string(&config).map_err(|error| error.to_string())?;
        let decoded = serde_json::from_str::<SourcedLlmConfig>(&encoded)
            .map_err(|error| error.to_string())?;
        let resolved = decoded.resolve_model_profile_at(now);
        assert_eq!(resolved.profile.context_window, Some(64_000));
        assert_eq!(
            resolved.profile.thinking_protocol,
            ThinkingProtocol::ModelManaged
        );
        assert!(resolved.applied_facts.iter().any(|metadata| {
            metadata.source == ModelFactSource::ExactModel && metadata.version == "etag-v2"
        }));

        let stale = ModelFactSet::new(
            ModelFactMetadata::new(
                ModelFactSource::ExactModel,
                "provider:/models/future-model",
                "etag-v1",
                SystemTime::UNIX_EPOCH,
                Some(SystemTime::UNIX_EPOCH),
                ModelFactConfidence::VERIFIED,
            ),
            None,
            ModelProfileOverride {
                context_window: Some(1_000_000),
                thinking_protocol: Some(ThinkingProtocol::ModelManaged),
                ..Default::default()
            },
        );
        let stale_resolution = decoded
            .with_exact_model_facts(stale)
            .resolve_model_profile_at(now);
        assert_eq!(stale_resolution.profile.context_window, None);
        assert_eq!(
            stale_resolution.profile.thinking_protocol,
            ThinkingProtocol::None
        );
        assert!(
            stale_resolution
                .ignored_stale_facts
                .iter()
                .any(|metadata| metadata.version == "etag-v1")
        );
        Ok(())
    }

    #[test]
    fn legacy_serialized_config_defaults_missing_fact_inputs() -> std::result::Result<(), String> {
        let decoded = serde_json::from_value::<LlmConfig>(serde_json::json!({
            "provider_name": "openai",
            "api_protocol": "responses",
            "base_url": "https://api.openai.com/v1/responses",
            "api_key": "test-key",
            "model": "gpt-5",
            "input_modalities": ["text"],
            "thinking_protocol": "openai_reasoning_effort",
            "timeouts": {}
        }))
        .map_err(|error| error.to_string())?;
        let resolution = decoded.resolve_model_profile_at(SystemTime::now());
        assert!(resolution.profile.capabilities.structured_output);
        assert!(resolution.applied_facts.iter().any(|metadata| {
            metadata.source == ModelFactSource::ProviderAdapter
                && metadata.provenance.contains("Responses")
        }));
        Ok(())
    }

    #[test]
    fn partial_provider_facts_preserve_current_adapter_protocol() -> std::result::Result<(), String>
    {
        let now = SystemTime::now();
        let sourced = LlmConfig::for_provider(
            "custom",
            "https://gateway.example/v1",
            "test-key",
            "future-model",
            LlmApiProtocol::ChatCompletions,
        )
        .map_err(|error| error.to_string())?
        .with_provider_facts(ModelFactSet::new(
            ModelFactMetadata::new(
                ModelFactSource::ProviderAdapter,
                "provider:/models",
                "provider-v2",
                now,
                None,
                ModelFactConfidence::VERIFIED,
            ),
            None,
            ModelProfileOverride {
                max_output_tokens: Some(4_096),
                ..Default::default()
            },
        ));
        let resolution = sourced.resolve_model_profile_at(now);
        assert_eq!(resolution.profile.max_output_tokens, Some(4_096));
        assert!(resolution.profile.capabilities.streaming_tool_calls);
        assert!(!resolution.profile.capabilities.named_sse_events);

        let client = sourced.build_client().map_err(|error| error.to_string())?;
        assert_eq!(
            client.protocol_capabilities(),
            ProviderCapabilityOverride::openai_chat_protocol()
        );
        Ok(())
    }

    #[test]
    fn anthropic_adapter_capabilities_survive_custom_provider_label()
    -> std::result::Result<(), String> {
        let config = LlmConfig::for_provider(
            "private-anthropic-gateway",
            "https://gateway.example/v1/messages",
            "test-key",
            "claude-test",
            LlmApiProtocol::Anthropic,
        )
        .map_err(|error| error.to_string())?;
        let client = config.build_client().map_err(|error| error.to_string())?;
        let capabilities = client.capabilities();

        assert!(capabilities.image_input);
        assert!(capabilities.tool_support);
        assert!(capabilities.supports_parallel_tool_calls);
        assert!(!capabilities.structured_output);
        assert_eq!(capabilities.tokenizer_name, Some("claude"));
        Ok(())
    }

    #[test]
    fn text_only_model_rejects_image_audio_and_video() {
        let mut image = Message::user(String::new());
        image.content = MessageContent::Parts(vec![ContentPart::ImageUrl {
            image_url: echo_core::llm::types::ImageUrl {
                url: "data:image/png;base64,AA==".to_string(),
                detail: None,
            },
        }]);
        assert!(
            validate_model_input_modalities(
                "text-model",
                &ModelInputModality::text_only(),
                &[image],
            )
            .is_err()
        );

        for (name, modality) in [("meeting.mp3", "audio"), ("demo.mp4", "video")] {
            let mut message = Message::user(String::new());
            message.content = MessageContent::Parts(vec![ContentPart::File {
                name: name.to_string(),
                content: String::new(),
            }]);
            let error = validate_model_input_modalities(
                "text-model",
                &ModelInputModality::text_only(),
                &[message],
            )
            .err()
            .map(|value| value.to_string())
            .unwrap_or_default();
            assert!(error.contains(modality));
        }
    }
}
