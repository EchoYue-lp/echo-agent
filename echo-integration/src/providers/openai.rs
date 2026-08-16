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

fn token_limits(model: &str, limit: Option<u32>) -> (Option<u32>, Option<u32>) {
    let lower = model.to_ascii_lowercase();
    if lower.starts_with("gpt-5")
        || lower.starts_with("o1")
        || lower.starts_with("o3")
        || lower.starts_with("o4")
    {
        (None, limit)
    } else {
        (limit, None)
    }
}

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
            // Signed/redacted reasoning blocks are Anthropic protocol state.
            // Preserve them in provider-neutral history, but do not leak these
            // extension fields to OpenAI-compatible endpoints.
            msg.reasoning_blocks = None;
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
    let (max_tokens, max_completion_tokens) = token_limits(&model.model, max_tokens);
    let request_body = ChatCompletionRequest {
        model: model.model.clone(),
        messages: normalize_messages(messages.to_vec()),
        temperature,
        max_tokens,
        max_completion_tokens,
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
    let (max_tokens, max_completion_tokens) = token_limits(&model.model, max_tokens);
    let request_body = ChatCompletionRequest {
        model: model.model.clone(),
        messages: normalize_messages(messages),
        temperature,
        max_tokens,
        max_completion_tokens,
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
                let (max_tokens, max_completion_tokens) =
                    token_limits(&self.config.model, request.max_tokens);
                let req = ChatCompletionRequest {
                    model: self.config.model.clone(),
                    messages: normalize_messages(request.messages),
                    // o-series / GPT-5 reasoning models reject temperature.
                    temperature: if t.drop_temperature {
                        None
                    } else {
                        request.temperature
                    },
                    max_tokens,
                    max_completion_tokens,
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
                    usage: raw.usage.clone(),
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
                let (max_tokens, max_completion_tokens) =
                    token_limits(&self.config.model, request.max_tokens);
                let req = ChatCompletionRequest {
                    model: self.config.model.clone(),
                    messages: normalize_messages(request.messages),
                    temperature: if t.drop_temperature {
                        None
                    } else {
                        request.temperature
                    },
                    max_tokens,
                    max_completion_tokens,
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

    #[test]
    fn anthropic_reasoning_blocks_are_not_forwarded() {
        use echo_core::llm::types::ReasoningBlock;

        let mut msg = Message::assistant("answer".to_string());
        msg.reasoning_blocks = Some(vec![ReasoningBlock::Signed {
            thinking: "private reasoning".to_string(),
            signature: "signature".to_string(),
        }]);

        let out = normalize_messages(vec![msg]);
        assert!(
            out.first()
                .is_some_and(|message| message.reasoning_blocks.is_none())
        );
    }

    #[test]
    fn literal_stream_chunks_preserve_tool_identity_and_terminal_usage()
    -> std::result::Result<(), serde_json::Error> {
        let tool: ChatCompletionChunk = serde_json::from_str(
            r#"{"id":"chatcmpl-1","choices":[{"index":0,"delta":{"role":"assistant","tool_calls":[{"index":0,"id":"call-7","type":"function","function":{"name":"read_file","arguments":"{\"path\":"}}]},"finish_reason":null}]}"#,
        )?;
        let tool_call = tool
            .choices
            .first()
            .and_then(|choice| choice.delta.tool_calls.as_ref())
            .and_then(|calls| calls.first());
        assert!(matches!(
            tool_call,
            Some(call)
                if call.id.as_deref() == Some("call-7")
                    && call.function.as_ref().and_then(|value| value.name.as_deref())
                        == Some("read_file")
                    && call.function.as_ref().and_then(|value| value.arguments.as_deref())
                        == Some("{\"path\":")
        ));

        let terminal: ChatCompletionChunk = serde_json::from_str(
            r#"{"id":"chatcmpl-1","choices":[{"index":0,"delta":null,"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":11,"completion_tokens":3,"total_tokens":14,"prompt_tokens_details":{"cached_tokens":5}}}"#,
        )?;
        assert!(terminal.choices.first().is_some_and(|choice| {
            choice.delta.content.is_none() && choice.finish_reason.as_deref() == Some("tool_calls")
        }));
        assert!(terminal.usage.as_ref().is_some_and(|usage| {
            usage.total_tokens == Some(14)
                && usage
                    .prompt_tokens_details
                    .as_ref()
                    .and_then(|details| details.cached_tokens)
                    == Some(5)
        }));
        Ok(())
    }
}
