//! Provider-neutral runtime configuration for LLM clients.
//!
//! Provider catalogs, credentials, and model selection belong to the consuming
//! application. This module only turns an explicit provider/model contract into
//! a concrete wire client and validates that requests respect model capabilities.

use echo_core::error::{ConfigError, LlmError, Result};
use echo_core::llm::capabilities::resolve_thinking_profile;
use echo_core::llm::types::{ContentPart, Message, MessageContent};
use echo_core::llm::{LlmApiProtocol, ModelInputModality, ThinkingProtocol};
use serde::{Deserialize, Serialize};

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
    #[serde(default)]
    pub thinking_protocol: ThinkingProtocol,
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
        let thinking_protocol =
            resolve_thinking_profile(&provider_name, &model, api_protocol, Some(&base_url))
                .protocol;
        Ok(Self {
            provider_name: (!provider_name.trim().is_empty()).then_some(provider_name),
            api_protocol,
            base_url,
            api_key: api_key.into(),
            model,
            input_modalities: ModelInputModality::text_only(),
            thinking_protocol,
        })
    }

    /// Set the concrete model's accepted input modalities.
    pub fn with_input_modalities(mut self, input_modalities: Vec<ModelInputModality>) -> Self {
        self.input_modalities = normalize_input_modalities(input_modalities);
        self
    }

    /// Build the wire client selected by [`Self::api_protocol`].
    pub fn build_client(&self) -> Result<Box<dyn echo_core::llm::LlmClient>> {
        match self.api_protocol {
            LlmApiProtocol::Responses => Ok(Box::new(super::responses::ResponsesClient::new(
                self.clone(),
            )?)),
            LlmApiProtocol::ChatCompletions => {
                Ok(Box::new(super::openai::OpenAiClient::new(self.clone())?))
            }
            LlmApiProtocol::Anthropic => Ok(Box::new(
                super::anthropic::AnthropicClient::with_base_url(
                    &self.base_url,
                    &self.api_key,
                    &self.model,
                )
                .with_input_modalities(self.input_modalities.clone()),
            )),
        }
    }

    pub(crate) fn validate_input_modalities(&self, messages: &[Message]) -> Result<()> {
        validate_model_input_modalities(&self.model, &self.input_modalities, messages)
    }
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

        let multimodal = text.with_input_modalities(vec![
            ModelInputModality::Image,
            ModelInputModality::Audio,
            ModelInputModality::Video,
        ]);
        assert_eq!(
            multimodal.input_modalities,
            ModelInputModality::all_supported()
        );
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
