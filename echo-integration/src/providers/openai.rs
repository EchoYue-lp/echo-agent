use echo_core::error::{LlmError, ReactError, Result};
use echo_core::llm::capabilities::ProviderCapabilities;
use echo_core::llm::types::{
    ChatCompletionChunk, ChatCompletionRequest, ChatCompletionResponse, ContentPart, Message,
    MessageContent, ResponseFormat, ToolDefinition,
};
use echo_core::llm::{ChatChunk, ChatRequest, ChatResponse, LlmClient};
use futures::Stream;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use reqwest::Client;
use reqwest::header::HeaderMap;
use std::sync::Arc;
use tracing::{Instrument, info_span};

use super::client::{post, stream_post};
use super::config::{Config, LlmConfig, ModelConfig};
use super::thinking_translate::translate_thinking_openai_compat;

// ── Convenience Functions ─────────────────────────────────────────────────────

/// Normalize message content for the OpenAI Chat Completions API.
///
/// OpenAI's content-part schema only recognizes `text` and `image_url`. Our
/// `ContentPart::File` (used for PDF/document attachments) serializes to
/// `{"type":"file",...}`, which OpenAI-compatible gateways (DeepSeek, etc.)
/// reject or silently drop — losing the attachment entirely.
///
/// This replaces any `ContentPart::File` with a text fallback (decoded inline
/// for text-class files, a name-only placeholder for binary), mirroring the
/// Anthropic provider's `file_to_content_block` behaviour. `Text` and
/// `ImageUrl` parts already match the OpenAI spec and pass through untouched.
fn normalize_messages(messages: Vec<Message>) -> Vec<Message> {
    messages
        .into_iter()
        .map(|mut msg| {
            if let MessageContent::Parts(parts) = &mut msg.content {
                let mut rewritten: Vec<ContentPart> = Vec::with_capacity(parts.len());
                for part in parts.drain(..) {
                    rewritten.push(normalize_content_part(part));
                }
                msg.content = MessageContent::Parts(rewritten);
            }
            msg
        })
        .collect()
}

/// Convert a single content part to an OpenAI-compatible form.
fn normalize_content_part(part: ContentPart) -> ContentPart {
    match part {
        ContentPart::Text { .. } | ContentPart::ImageUrl { .. } => part,
        ContentPart::File { name, content } => {
            // Same dispatch as the Anthropic provider: text-class files are
            // decoded and inlined so the model can read them; everything else
            // (including PDFs, since OpenAI has no document block here) becomes
            // a name-only placeholder.
            if is_text_class_filename(&name) {
                let decoded =
                    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &content)
                        .ok();
                let text = decoded
                    .and_then(|bytes| String::from_utf8(bytes).ok())
                    .unwrap_or_else(|| format!("[Attachment: {name}] (undecodable)"));
                ContentPart::Text {
                    text: format!("\n[Attachment: {name}]\n```\n{text}\n```"),
                }
            } else {
                ContentPart::Text {
                    text: format!("\n[Attachment: {name}]"),
                }
            }
        }
    }
}

/// Whether a filename looks like a text-class file (mirrors the Anthropic
/// provider's allowlist so both gateways behave identically).
fn is_text_class_filename(name: &str) -> bool {
    matches!(
        name.rsplit('.')
            .next()
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some(
            "txt"
                | "md"
                | "markdown"
                | "json"
                | "xml"
                | "yaml"
                | "yml"
                | "csv"
                | "tsv"
                | "rs"
                | "py"
                | "js"
                | "ts"
                | "tsx"
                | "jsx"
                | "go"
                | "java"
                | "c"
                | "cpp"
                | "h"
                | "sh"
                | "toml"
                | "ini"
                | "log"
                | "sql"
        )
    )
}

/// Assemble request headers
pub fn assemble_req_header(model: &ModelConfig) -> Result<HeaderMap> {
    let mut header_map = HeaderMap::new();
    header_map.insert(
        "Authorization",
        format!("Bearer {}", model.apikey)
            .parse()
            .map_err(|e| ReactError::Other(format!("Invalid Authorization header: {}", e)))?,
    );
    header_map.insert(
        "Content-Type",
        "application/json"
            .parse()
            .map_err(|e| ReactError::Other(format!("Invalid Content-Type header: {}", e)))?,
    );
    Ok(header_map)
}

/// Synchronous chat request (standalone function, uses environment variable config).
///
/// `messages` accepts a slice reference so callers don't need to repeatedly clone
/// the entire message list in a retry loop. Internally converts to an owned Vec
/// as needed, with a fixed cost of a single clone.
#[allow(clippy::too_many_arguments)]
pub async fn chat(
    client: Arc<Client>,
    model_name: &str,
    messages: &[Message],
    temperature: Option<f32>,
    max_tokens: Option<u32>,
    stream: Option<bool>,
    tools: Option<Vec<ToolDefinition>>,
    tool_choice: Option<String>,
    response_format: Option<ResponseFormat>,
    user_id: Option<String>,
) -> Result<ChatCompletionResponse> {
    let model = Config::get_model(model_name)?;
    let request_body = ChatCompletionRequest {
        model: model.model.clone(),
        messages: normalize_messages(messages.to_vec()),
        temperature,
        max_tokens,
        stream,
        tools,
        tool_choice,
        response_format,
        stream_options: None,
        reasoning_effort: None,
        enable_thinking: None,
        thinking_budget: None,
        glm_thinking: None,
        user_id,
    };

    let header_map = assemble_req_header(&model)?;
    post(client, &request_body, header_map, model.baseurl.as_str()).await
}

/// Streaming chat request (standalone function, uses environment variable config)
#[allow(clippy::too_many_arguments)]
pub async fn stream_chat(
    client: Arc<Client>,
    model_name: &str,
    messages: Vec<Message>,
    temperature: Option<f32>,
    max_tokens: Option<u32>,
    tools: Option<Vec<ToolDefinition>>,
    tool_choice: Option<String>,
    response_format: Option<ResponseFormat>,
    cancel_token: Option<tokio_util::sync::CancellationToken>,
    user_id: Option<String>,
) -> Result<impl Stream<Item = Result<ChatCompletionChunk>> + use<>> {
    let model = Config::get_model(model_name)?;
    let request_body = ChatCompletionRequest {
        model: model.model.clone(),
        messages: normalize_messages(messages),
        temperature,
        max_tokens,
        stream: Some(true),
        stream_options: Some(serde_json::json!({"include_usage": true})),
        tools,
        tool_choice,
        response_format,
        reasoning_effort: None,
        enable_thinking: None,
        thinking_budget: None,
        glm_thinking: None,
        user_id,
    };

    let header_map = assemble_req_header(&model)?;
    let url = model.baseurl.clone();
    stream_post(client, request_body, header_map, url, cancel_token).await
}

// ── OpenAI Client Implementation ───────────────────────────────────────────────

/// OpenAI-compatible client.
///
/// Supports any service compatible with the OpenAI Chat Completions API.
pub struct OpenAiClient {
    client: Arc<Client>,
    config: ModelConfig,
    header_map: HeaderMap,
}

impl OpenAiClient {
    /// Create a client from environment variables
    pub fn from_env(model_name: &str) -> Result<Self> {
        let config = Config::get_model(model_name)?;
        let header_map = assemble_req_header(&config)?;
        Ok(Self {
            client: Arc::new(Self::build_http_client()),
            config,
            header_map,
        })
    }

    /// Create a client with a custom configuration
    pub fn new(config: LlmConfig) -> Result<Self> {
        let model_config = config.to_model_config();
        let header_map = assemble_req_header(&model_config)?;
        Ok(Self {
            client: Arc::new(Self::build_http_client()),
            config: model_config,
            header_map,
        })
    }

    /// Create a client with a shared HTTP client
    pub fn with_client(client: Arc<Client>, config: LlmConfig) -> Result<Self> {
        let model_config = config.to_model_config();
        let header_map = assemble_req_header(&model_config)?;
        Ok(Self {
            client,
            config: model_config,
            header_map,
        })
    }

    fn build_http_client() -> Client {
        Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .unwrap_or_default()
    }
}

impl LlmClient for OpenAiClient {
    fn chat(&self, request: ChatRequest) -> BoxFuture<'_, Result<ChatResponse>> {
        let model = self.config.model.clone();
        Box::pin(
            async move {
                // Resolve the thinking protocol from the REAL provider (e.g.
                // "dashscope"), not the LlmProvider enum (which collapses all
                // OpenAI-compatible providers into "openai"). The same model
                // (e.g. deepseek-v4-pro) uses reasoning_effort via
                // api.deepseek.com but enable_thinking via Bailian/DashScope.
                let provider_str = self.config.provider_name.as_deref().unwrap_or("openai");
                let t = translate_thinking_openai_compat(
                    &self.config.model,
                    provider_str,
                    &request.thinking,
                    ProviderCapabilities::openai_compatible(),
                );
                let req = ChatCompletionRequest {
                    model: self.config.model.clone(),
                    messages: normalize_messages(request.messages),
                    // o-series / GPT-5 reasoning models reject temperature.
                    temperature: if t.drop_temperature {
                        None
                    } else {
                        request.temperature
                    },
                    max_tokens: request.max_tokens,
                    stream: None,
                    stream_options: None,
                    tools: request.tools,
                    tool_choice: request.tool_choice,
                    response_format: request.response_format,
                    reasoning_effort: t.reasoning_effort,
                    enable_thinking: t.enable_thinking,
                    thinking_budget: t.thinking_budget,
                    glm_thinking: t.glm_thinking,
                    user_id: request.user_id.clone(),
                };

                let raw = post(
                    self.client.clone(),
                    &req,
                    self.header_map.clone(),
                    &self.config.baseurl,
                )
                .await?;

                let choice = raw.choices.first().ok_or(LlmError::EmptyResponse)?;

                Ok(ChatResponse {
                    message: choice.message.clone(),
                    finish_reason: choice.finish_reason.clone(),
                    raw,
                })
            }
            .instrument(info_span!("openai_chat", model = %model)),
        )
    }

    fn chat_stream(
        &self,
        request: ChatRequest,
    ) -> BoxFuture<'_, Result<BoxStream<'static, Result<ChatChunk>>>> {
        let model = self.config.model.clone();
        Box::pin(
            async move {
                let provider_str = self.config.provider_name.as_deref().unwrap_or("openai");
                let t = translate_thinking_openai_compat(
                    &self.config.model,
                    provider_str,
                    &request.thinking,
                    ProviderCapabilities::openai_compatible(),
                );
                let req = ChatCompletionRequest {
                    model: self.config.model.clone(),
                    messages: normalize_messages(request.messages),
                    temperature: if t.drop_temperature {
                        None
                    } else {
                        request.temperature
                    },
                    max_tokens: request.max_tokens,
                    stream: Some(true),
                    stream_options: Some(serde_json::json!({"include_usage": true})),
                    tools: request.tools,
                    tool_choice: request.tool_choice,
                    response_format: request.response_format,
                    reasoning_effort: t.reasoning_effort,
                    enable_thinking: t.enable_thinking,
                    thinking_budget: t.thinking_budget,
                    glm_thinking: t.glm_thinking,
                    user_id: request.user_id.clone(),
                };

                let stream = stream_post(
                    self.client.clone(),
                    req,
                    self.header_map.clone(),
                    self.config.baseurl.clone(),
                    request.cancel_token,
                )
                .await?;

                Ok(Box::pin(futures::StreamExt::map(stream, |result| {
                    result.map(|chunk| {
                        let choice = chunk.choices.first();
                        ChatChunk {
                            delta: choice.map(|c| c.delta.clone()).unwrap_or_default(),
                            finish_reason: choice.and_then(|c| c.finish_reason.clone()),
                            usage: chunk.usage.clone(),
                        }
                    })
                })) as BoxStream<'_, Result<ChatChunk>>)
            }
            .instrument(info_span!("openai_chat_stream", model = %model)),
        )
    }

    fn model_name(&self) -> &str {
        &self.config.model
    }
}

/// Default [`LlmClient`] implementation based on the [`chat`] function
pub struct DefaultLlmClient {
    client: Arc<Client>,
    model_name: String,
}

impl DefaultLlmClient {
    pub fn new(client: Arc<Client>, model_name: impl Into<String>) -> Self {
        Self {
            client,
            model_name: model_name.into(),
        }
    }
}

impl LlmClient for DefaultLlmClient {
    fn chat(&self, request: ChatRequest) -> BoxFuture<'_, Result<ChatResponse>> {
        Box::pin(async move {
            if request.thinking.is_some() {
                tracing::warn!(
                    model = %self.model_name,
                    "DefaultLlmClient does not translate thinking config; use a configured OpenAiClient/AnthropicClient to apply it"
                );
            }
            let raw = chat(
                self.client.clone(),
                &self.model_name,
                &request.messages,
                request.temperature,
                request.max_tokens,
                None,
                request.tools,
                request.tool_choice,
                request.response_format,
                request.user_id,
            )
            .await?;

            let choice = raw.choices.first().ok_or(LlmError::EmptyResponse)?;

            Ok(ChatResponse {
                message: choice.message.clone(),
                finish_reason: choice.finish_reason.clone(),
                raw,
            })
        })
    }

    fn chat_stream(
        &self,
        request: ChatRequest,
    ) -> BoxFuture<'_, Result<BoxStream<'static, Result<ChatChunk>>>> {
        Box::pin(async move {
            if request.thinking.is_some() {
                tracing::warn!(
                    model = %self.model_name,
                    "DefaultLlmClient does not translate thinking config; use a configured OpenAiClient/AnthropicClient to apply it"
                );
            }
            let stream = stream_chat(
                self.client.clone(),
                &self.model_name,
                request.messages,
                request.temperature,
                request.max_tokens,
                request.tools,
                request.tool_choice,
                request.response_format,
                request.cancel_token,
                request.user_id,
            )
            .await?;

            Ok(Box::pin(futures::StreamExt::map(stream, |result| {
                result.map(|chunk| {
                    let choice = chunk.choices.first();
                    ChatChunk {
                        delta: choice.map(|c| c.delta.clone()).unwrap_or_default(),
                        finish_reason: choice.and_then(|c| c.finish_reason.clone()),
                        usage: chunk.usage.clone(),
                    }
                })
            })) as BoxStream<'_, Result<ChatChunk>>)
        })
    }

    fn chat_simple(&self, messages: Vec<Message>) -> BoxFuture<'_, Result<String>> {
        self.chat_simple_with_options(
            messages,
            echo_core::llm::SimpleChatOptions {
                temperature: Some(0.3),
                max_tokens: Some(2048),
            },
        )
    }

    fn chat_simple_with_options(
        &self,
        messages: Vec<Message>,
        options: echo_core::llm::SimpleChatOptions,
    ) -> BoxFuture<'_, Result<String>> {
        Box::pin(async move {
            let response = chat(
                self.client.clone(),
                &self.model_name,
                &messages,
                options.temperature,
                options.max_tokens,
                Some(false),
                None,
                None,
                None,
                None,
            )
            .await?;

            response
                .choices
                .into_iter()
                .next()
                .and_then(|c| c.message.content.as_text())
                .ok_or_else(|| ReactError::Other("LLM returned empty content".to_string()))
        })
    }

    fn model_name(&self) -> &str {
        &self.model_name
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;

    fn multimodal_msg(parts: Vec<ContentPart>) -> Message {
        let mut msg = Message::user(String::new());
        msg.content = MessageContent::Parts(parts);
        msg
    }

    #[test]
    fn text_and_image_parts_pass_through_unchanged() {
        let msg = multimodal_msg(vec![
            ContentPart::Text {
                text: "hi".to_string(),
            },
            ContentPart::ImageUrl {
                image_url: echo_core::llm::types::ImageUrl {
                    url: "data:image/png;base64,AAA".to_string(),
                    detail: None,
                },
            },
        ]);
        let out = normalize_messages(vec![msg]);
        match &out[0].content {
            MessageContent::Parts(parts) => {
                assert_eq!(parts.len(), 2);
                assert!(matches!(parts[0], ContentPart::Text { .. }));
                assert!(matches!(parts[1], ContentPart::ImageUrl { .. }));
            }
            other => panic!("expected Parts, got {other:?}"),
        }
    }

    #[test]
    fn text_class_file_is_inlined_as_text() {
        let b64 = base64::engine::general_purpose::STANDARD.encode(b"hello notes");
        let msg = multimodal_msg(vec![ContentPart::File {
            name: "notes.txt".to_string(),
            content: b64,
        }]);
        let out = normalize_messages(vec![msg]);
        match &out[0].content {
            MessageContent::Parts(parts) => {
                assert_eq!(parts.len(), 1);
                match &parts[0] {
                    ContentPart::Text { text } => {
                        assert!(text.contains("hello notes"));
                        assert!(text.contains("notes.txt"));
                    }
                    other => panic!("expected Text, got {other:?}"),
                }
            }
            other => panic!("expected Parts, got {other:?}"),
        }
    }

    #[test]
    fn binary_file_becomes_placeholder() {
        let b64 = base64::engine::general_purpose::STANDARD.encode(b"\x00\x01zip");
        let msg = multimodal_msg(vec![ContentPart::File {
            name: "archive.zip".to_string(),
            content: b64,
        }]);
        let out = normalize_messages(vec![msg]);
        match &out[0].content {
            MessageContent::Parts(parts) => {
                assert_eq!(parts.len(), 1);
                match &parts[0] {
                    ContentPart::Text { text } => {
                        assert!(text.contains("archive.zip"));
                    }
                    other => panic!("expected Text placeholder, got {other:?}"),
                }
            }
            other => panic!("expected Parts, got {other:?}"),
        }
    }

    #[test]
    fn plain_text_message_is_untouched() {
        let msg = Message::user("hello".to_string());
        let out = normalize_messages(vec![msg]);
        assert_eq!(out[0].content.as_text(), Some("hello".to_string()));
    }
}
