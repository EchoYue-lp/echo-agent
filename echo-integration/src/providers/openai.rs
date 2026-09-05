use echo_core::error::{LlmError, ReactError, Result};
use echo_core::llm::types::{ChatCompletionRequest, ContentPart, Message, MessageContent};
use echo_core::llm::{ChatChunk, ChatRequest, ChatResponse, LlmClient};
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use reqwest::Client;
use reqwest::header::HeaderMap;
use std::sync::Arc;
use tracing::{Instrument, info_span};

use super::client::{post, stream_post};
use super::config::LlmConfig;
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
        ContentPart::ResourceLink { resource } => ContentPart::Text {
            text: resource.model_text(),
        },
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
pub fn assemble_req_header(model: &LlmConfig) -> Result<HeaderMap> {
    let mut header_map = HeaderMap::new();
    header_map.insert(
        "Authorization",
        format!("Bearer {}", model.api_key)
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

// ── OpenAI Client Implementation ───────────────────────────────────────────────

/// OpenAI-compatible client.
///
/// Supports any service compatible with the OpenAI Chat Completions API.
pub struct OpenAiClient {
    client: Arc<Client>,
    config: LlmConfig,
    header_map: HeaderMap,
}

impl OpenAiClient {
    /// Create a client with a custom configuration
    pub fn new(config: LlmConfig) -> Result<Self> {
        let header_map = assemble_req_header(&config)?;
        Ok(Self {
            client: Arc::new(Self::build_http_client()),
            config,
            header_map,
        })
    }

    /// Create a client with a shared HTTP client
    pub fn with_client(client: Arc<Client>, config: LlmConfig) -> Result<Self> {
        let header_map = assemble_req_header(&config)?;
        Ok(Self {
            client,
            config,
            header_map,
        })
    }

    fn build_http_client() -> Client {
        Client::new()
    }
}

impl LlmClient for OpenAiClient {
    fn chat(&self, request: ChatRequest) -> BoxFuture<'_, Result<ChatResponse>> {
        let model = self.config.model.clone();
        Box::pin(
            async move {
                self.config.validate_input_modalities(&request.messages)?;
                let timeouts = request.timeouts.unwrap_or(self.config.timeouts);
                let t = translate_thinking_openai_compat(
                    &self.config.model,
                    self.config.api_protocol,
                    self.config.thinking_protocol,
                    &request.thinking,
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
                    thinking_type: t.thinking_type,
                    think: t.ollama_think,
                    user_id: request.user_id.clone(),
                };

                let raw = post(
                    self.client.clone(),
                    &req,
                    self.header_map.clone(),
                    &self.config.base_url,
                    timeouts,
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
                self.config.validate_input_modalities(&request.messages)?;
                let timeouts = request.timeouts.unwrap_or(self.config.timeouts);
                let t = translate_thinking_openai_compat(
                    &self.config.model,
                    self.config.api_protocol,
                    self.config.thinking_protocol,
                    &request.thinking,
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
                    thinking_type: t.thinking_type,
                    think: t.ollama_think,
                    user_id: request.user_id.clone(),
                };

                let stream = stream_post(
                    self.client.clone(),
                    req,
                    self.header_map.clone(),
                    self.config.base_url.clone(),
                    timeouts,
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
    use echo_core::llm::types::ChatCompletionChunk;

    fn multimodal_msg(parts: Vec<ContentPart>) -> Message {
        let mut msg = Message::user(String::new());
        msg.content = MessageContent::Parts(parts);
        msg
    }

    #[test]
    fn text_and_image_parts_pass_through_unchanged() -> std::result::Result<(), String> {
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
        let parts = out
            .first()
            .and_then(|message| match &message.content {
                MessageContent::Parts(parts) => Some(parts),
                MessageContent::Text(_) | MessageContent::Empty => None,
            })
            .ok_or_else(|| "expected message parts".to_string())?;
        assert_eq!(parts.len(), 2);
        assert!(matches!(parts.first(), Some(ContentPart::Text { .. })));
        assert!(matches!(parts.get(1), Some(ContentPart::ImageUrl { .. })));
        Ok(())
    }

    #[test]
    fn text_class_file_is_inlined_as_text() -> std::result::Result<(), String> {
        let b64 = base64::engine::general_purpose::STANDARD.encode(b"hello notes");
        let msg = multimodal_msg(vec![ContentPart::File {
            name: "notes.txt".to_string(),
            content: b64,
        }]);
        let out = normalize_messages(vec![msg]);
        let part = out
            .first()
            .and_then(|message| match &message.content {
                MessageContent::Parts(parts) => parts.first(),
                MessageContent::Text(_) | MessageContent::Empty => None,
            })
            .ok_or_else(|| "expected one message part".to_string())?;
        let ContentPart::Text { text } = part else {
            return Err("expected text content part".to_string());
        };
        assert!(text.contains("hello notes"));
        assert!(text.contains("notes.txt"));
        Ok(())
    }

    #[test]
    fn binary_file_becomes_placeholder() -> std::result::Result<(), String> {
        let b64 = base64::engine::general_purpose::STANDARD.encode(b"\x00\x01zip");
        let msg = multimodal_msg(vec![ContentPart::File {
            name: "archive.zip".to_string(),
            content: b64,
        }]);
        let out = normalize_messages(vec![msg]);
        let part = out
            .first()
            .and_then(|message| match &message.content {
                MessageContent::Parts(parts) => parts.first(),
                MessageContent::Text(_) | MessageContent::Empty => None,
            })
            .ok_or_else(|| "expected one message part".to_string())?;
        let ContentPart::Text { text } = part else {
            return Err("expected text placeholder".to_string());
        };
        assert!(text.contains("archive.zip"));
        Ok(())
    }

    #[test]
    fn linked_resource_becomes_text_only_at_provider_boundary() -> std::result::Result<(), String> {
        let resource = echo_core::llm::types::LinkedResource {
            annotations: None,
            description: Some("source context".to_string()),
            mime_type: Some("text/rust".to_string()),
            name: "lib.rs".to_string(),
            size: None,
            title: None,
            uri: "file:///workspace/src/lib.rs".to_string(),
            meta: None,
        };
        let msg = multimodal_msg(vec![ContentPart::ResourceLink {
            resource: Box::new(resource),
        }]);
        let out = normalize_messages(vec![msg]);
        let part = out
            .first()
            .and_then(|message| match &message.content {
                MessageContent::Parts(parts) => parts.first(),
                MessageContent::Text(_) | MessageContent::Empty => None,
            })
            .ok_or_else(|| "expected one message part".to_string())?;
        let ContentPart::Text { text } = part else {
            return Err("expected provider text fallback".to_string());
        };
        assert!(text.contains("file:///workspace/src/lib.rs"));
        assert!(text.contains("source context"));
        Ok(())
    }

    #[test]
    fn plain_text_message_is_untouched() -> std::result::Result<(), String> {
        let msg = Message::user("hello".to_string());
        let out = normalize_messages(vec![msg]);
        assert_eq!(
            out.first().and_then(|message| message.content.as_text()),
            Some("hello".to_string())
        );
        Ok(())
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
