//! Anthropic Messages API provider
//!
//! Implements [`LlmClient`](echo_core::llm::LlmClient) for Anthropic's `/v1/messages` endpoint.
//! System messages are sent as a top-level `system` field (not in the messages array).

use echo_core::error::{LlmError, Result};
use echo_core::llm::types::{
    ContentPart, DeltaFunctionCall, DeltaMessage, DeltaToolCall, FunctionCall, Message,
    MessageContent, ToolCall,
};
use echo_core::llm::{ChatChunk, ChatRequest, ChatResponse, LlmClient};
use echo_core::retry::{RetryPolicy, with_retry_if};
use futures::StreamExt;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{Instrument, info_span};

fn is_retryable(err: &LlmError) -> bool {
    match err {
        LlmError::NetworkError(_) => true,
        LlmError::ApiError { status, .. } => *status == 429 || *status >= 500,
        _ => false,
    }
}

pub struct AnthropicClient {
    client: Arc<Client>,
    api_key: String,
    model: String,
    base_url: String,
}

impl AnthropicClient {
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            client: Arc::new(Self::build_http_client()),
            api_key: api_key.into(),
            model: model.into(),
            base_url: "https://api.anthropic.com/v1/messages".to_string(),
        }
    }

    pub fn with_base_url(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            client: Arc::new(Self::build_http_client()),
            api_key: api_key.into(),
            model: model.into(),
            base_url: base_url.into(),
        }
    }

    fn build_http_client() -> Client {
        Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .unwrap_or_default()
    }

    fn convert_request(&self, request: &ChatRequest) -> AnthropicRequest {
        let mut system: Option<String> = None;
        let mut messages = Vec::new();

        for msg in &request.messages {
            if msg.role == "system" {
                system = msg.content.as_text();
                continue;
            }

            if msg.role == "tool" {
                messages.push(AnthropicMessage {
                    role: "user".to_string(),
                    content: AnthropicContent::Blocks(vec![ContentBlock::ToolResult {
                        tool_use_id: msg.tool_call_id.clone().unwrap_or_default(),
                        content: msg.content.as_text().unwrap_or_default(),
                    }]),
                });
                continue;
            }

            if msg.role == "assistant"
                && let Some(ref tool_calls) = msg.tool_calls
            {
                let mut blocks: Vec<ContentBlock> = Vec::new();
                if let Some(ref text) = msg.content.as_text()
                    && !text.is_empty()
                {
                    blocks.push(ContentBlock::Text { text: text.clone() });
                }
                for tc in tool_calls {
                    let input: serde_json::Value =
                        serde_json::from_str(&tc.function.arguments).unwrap_or_default();
                    blocks.push(ContentBlock::ToolUse {
                        id: tc.id.clone(),
                        name: tc.function.name.clone(),
                        input,
                    });
                }
                messages.push(AnthropicMessage {
                    role: "assistant".to_string(),
                    content: AnthropicContent::Blocks(blocks),
                });
                continue;
            }

            let content = match &msg.content {
                MessageContent::Parts(parts) => {
                    let blocks: Vec<ContentBlock> = parts
                        .iter()
                        .filter_map(|part| match part {
                            ContentPart::Text { text } => {
                                Some(ContentBlock::Text { text: text.clone() })
                            }
                            ContentPart::ImageUrl { image_url } => Some(ContentBlock::Image {
                                source: data_url_to_image_source(&image_url.url),
                            }),
                            ContentPart::File { name, .. } => Some(ContentBlock::Text {
                                text: format!("\n[Attachment: {name}]"),
                            }),
                        })
                        .collect();
                    if blocks.is_empty() {
                        AnthropicContent::Text(String::new())
                    } else {
                        AnthropicContent::Blocks(blocks)
                    }
                }
                _ => AnthropicContent::Text(msg.content.as_text().unwrap_or_default()),
            };

            messages.push(AnthropicMessage {
                role: msg.role.clone(),
                content,
            });
        }

        let tools: Option<Vec<AnthropicToolDef>> = request.tools.as_ref().map(|tools| {
            tools
                .iter()
                .map(|t| AnthropicToolDef {
                    name: t.function.name.clone(),
                    description: Some(t.function.description.clone()),
                    input_schema: t.function.parameters.clone(),
                })
                .collect()
        });

        AnthropicRequest {
            model: self.model.clone(),
            max_tokens: request.max_tokens.unwrap_or(4096),
            system,
            messages,
            temperature: request.temperature,
            tools,
            stream: None,
        }
    }

    fn convert_response(&self, resp: AnthropicResponse) -> ChatResponse {
        let mut content_parts: Vec<String> = Vec::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();

        for block in &resp.content {
            match block {
                ContentBlock::Text { text } => content_parts.push(text.clone()),
                ContentBlock::ToolUse { id, name, input } => {
                    tool_calls.push(ToolCall {
                        id: id.clone(),
                        call_type: "function".to_string(),
                        function: FunctionCall {
                            name: name.clone(),
                            arguments: serde_json::to_string(input).unwrap_or_default(),
                        },
                    });
                }
                _ => {}
            }
        }

        let finish_reason = match resp.stop_reason.as_deref() {
            Some("end_turn") => Some("stop".to_string()),
            Some("tool_use") => Some("tool_calls".to_string()),
            Some("max_tokens") => Some("length".to_string()),
            other => other.map(String::from),
        };

        let message = Message {
            role: "assistant".to_string(),
            content: if content_parts.is_empty() {
                echo_core::llm::types::MessageContent::Empty
            } else {
                echo_core::llm::types::MessageContent::Text(content_parts.join(""))
            },
            tool_calls: if tool_calls.is_empty() {
                None
            } else {
                Some(tool_calls)
            },
            tool_call_id: None,
            name: None,
            reasoning_content: None,
        };

        ChatResponse {
            message,
            finish_reason,
            raw: Default::default(),
        }
    }
}

impl LlmClient for AnthropicClient {
    fn chat(&self, request: ChatRequest) -> BoxFuture<'_, Result<ChatResponse>> {
        let model = self.model.clone();
        Box::pin(
            async move {
                let body = self.convert_request(&request);

                let policy = RetryPolicy::default();
                let resp = with_retry_if(
                    &policy,
                    || {
                        let client = self.client.clone();
                        let base_url = self.base_url.clone();
                        let api_key = self.api_key.clone();
                        let body = &body;
                        async move {
                            let resp = client
                                .post(&base_url)
                                .header("x-api-key", &api_key)
                                .header("anthropic-version", "2023-06-01")
                                .header("content-type", "application/json")
                                .json(body)
                                .send()
                                .await
                                .map_err(|e| LlmError::NetworkError(e.to_string()))?;

                            let status = resp.status();
                            if !status.is_success() {
                                let text = resp.text().await.unwrap_or_default();
                                return Err(LlmError::ApiError {
                                    status: status.as_u16(),
                                    message: text,
                                });
                            }

                            Ok(resp)
                        }
                    },
                    is_retryable,
                )
                .await?;

                let anthropic_resp: AnthropicResponse = resp
                    .json()
                    .await
                    .map_err(|e| LlmError::NetworkError(format!("Response parse error: {e}")))?;

                Ok(self.convert_response(anthropic_resp))
            }
            .instrument(info_span!("anthropic_chat", model = %model)),
        )
    }

    fn chat_stream(
        &self,
        request: ChatRequest,
    ) -> BoxFuture<'_, Result<BoxStream<'_, Result<ChatChunk>>>> {
        let model = self.model.clone();
        Box::pin(
            async move {
            let mut body = self.convert_request(&request);
            body.stream = Some(true);

            let policy = RetryPolicy::default();
            let resp = with_retry_if(
                &policy,
                || {
                    let client = self.client.clone();
                    let base_url = self.base_url.clone();
                    let api_key = self.api_key.clone();
                    let body = &body;
                    async move {
                        let resp = client
                            .post(&base_url)
                            .header("x-api-key", &api_key)
                            .header("anthropic-version", "2023-06-01")
                            .header("content-type", "application/json")
                            .json(body)
                            .send()
                            .await
                            .map_err(|e| LlmError::NetworkError(e.to_string()))?;

                        let status = resp.status();
                        if !status.is_success() {
                            let text = resp.text().await.unwrap_or_default();
                            return Err(LlmError::ApiError {
                                status: status.as_u16(),
                                message: text,
                            });
                        }

                        Ok(resp)
                    }
                },
                is_retryable,
            )
            .await?;

            let byte_stream = resp.bytes_stream();
            let mut buffer = String::new();
            // Track in-progress tool calls during streaming (index → accumulated args)
            let mut tool_call_args: std::collections::HashMap<usize, (String, String, String)> =
                std::collections::HashMap::new();

            let stream = async_stream::stream! {
                let mut byte_stream = std::pin::pin!(byte_stream);
                while let Some(chunk_result) = byte_stream.next().await {
                    // Check for cancellation
                    if let Some(ref ct) = request.cancel_token
                        && ct.is_cancelled() {
                            tracing::info!("Anthropic stream cancelled by caller");
                            return;
                        }

                    let chunk = match chunk_result {
                        Ok(c) => c,
                        Err(e) => {
                            yield Err(LlmError::NetworkError(e.to_string()).into());
                            return;
                        }
                    };

                    buffer.push_str(&String::from_utf8_lossy(&chunk));

                    while let Some(line_end) = buffer.find('\n') {
                        let line = buffer[..line_end].trim().to_string();
                        buffer = buffer[line_end + 1..].to_string();

                        if line.is_empty() || line.starts_with("event:") {
                            continue;
                        }

                        if let Some(data) = line.strip_prefix("data: ") {
                            if data == "[DONE]" {
                                return;
                            }
                            if let Ok(event) = serde_json::from_str::<AnthropicStreamEvent>(data) {
                                match event {
                                    AnthropicStreamEvent::ContentBlockStart {
                                        content_block:
                                            ContentBlockStartBody::ToolUse { id, name },
                                        ..
                                    } => {
                                        // Start tracking a new tool_use block
                                        let idx = tool_call_args.len();
                                        tool_call_args.insert(idx, (id, name, String::new()));
                                    }
                                    AnthropicStreamEvent::ContentBlockStart { .. } => {
                                        // text block start — no action needed
                                    }
                                    AnthropicStreamEvent::ContentBlockDelta {
                                        index,
                                        delta,
                                    } => {
                                        if let Some(text) = delta.text {
                                            yield Ok(ChatChunk {
                                                delta: DeltaMessage {
                                                    role: Some("assistant".to_string()),
                                                    content: Some(text),
                                                    reasoning_content: None,
                                                    tool_calls: None,
                                                },
                                                finish_reason: None,
                                                usage: None,
                                            });
                                        } else if let Some(partial) = delta.partial_json {
                                            // Accumulate tool_use arguments
                                            if let Some(entry) = tool_call_args.get_mut(&index) {
                                                entry.2.push_str(&partial);
                                            }
                                        }
                                    }
                                    AnthropicStreamEvent::ContentBlockStop { index } => {
                                        // Finalize tool call and emit
                                        if let Some((id, name, args)) =
                                            tool_call_args.remove(&index)
                                        {
                                            let parsed_args: serde_json::Value =
                                                serde_json::from_str(&args)
                                                    .unwrap_or(serde_json::Value::Null);
                                            yield Ok(ChatChunk {
                                                delta: DeltaMessage {
                                                    role: None,
                                                    content: None,
                                                    reasoning_content: None,
                                                    tool_calls: Some(vec![DeltaToolCall {
                                                        index: index as u32,
                                                        id: Some(id),
                                                        call_type: Some("function".to_string()),
                                                        function: Some(DeltaFunctionCall {
                                                            name: Some(name),
                                                            arguments: Some(
                                                                parsed_args.to_string(),
                                                            ),
                                                        }),
                                                    }]),
                                                },
                                                finish_reason: None,
                                                usage: None,
                                            });
                                        }
                                    }
                                    AnthropicStreamEvent::MessageDelta { delta, .. } => {
                                        let finish = match delta.stop_reason.as_deref() {
                                            Some("end_turn") => Some("stop".to_string()),
                                            Some("tool_use") => Some("tool_calls".to_string()),
                                            other => other.map(String::from),
                                        };
                                        yield Ok(ChatChunk {
                                            delta: DeltaMessage {
                                                role: None,
                                                content: None,
                                                reasoning_content: None,
                                                tool_calls: None,
                                            },
                                            finish_reason: finish,
                                            usage: None,
                                        });
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                }
            };

            Ok(Box::pin(stream) as BoxStream<'_, Result<ChatChunk>>)
        }
        .instrument(info_span!("anthropic_chat_stream", model = %model)),
        )
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

// ── Anthropic API types ──────────────────────────────────────────────────────

#[derive(Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<AnthropicToolDef>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

#[derive(Serialize)]
struct AnthropicMessage {
    role: String,
    content: AnthropicContent,
}

#[derive(Serialize)]
#[serde(untagged)]
enum AnthropicContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type")]
enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image { source: ImageSource },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ImageSource {
    Base64 {
        media_type: String,
        data: String,
    },
    #[serde(rename = "url")]
    Url_ {
        url: String,
    },
}

/// Parse a data URL into an Anthropic ImageSource
fn data_url_to_image_source(url: &str) -> ImageSource {
    if let Some(rest) = url.strip_prefix("data:")
        && let Some((media_type, b64_data)) = rest.split_once(';')
        && let Some(data) = b64_data.strip_prefix("base64,")
    {
        return ImageSource::Base64 {
            media_type: media_type.to_string(),
            data: data.to_string(),
        };
    }
    ImageSource::Url_ {
        url: url.to_string(),
    }
}

#[derive(Serialize)]
struct AnthropicToolDef {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    input_schema: serde_json::Value,
}

#[derive(Deserialize)]
struct AnthropicResponse {
    content: Vec<ContentBlock>,
    stop_reason: Option<String>,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum AnthropicStreamEvent {
    #[serde(rename = "content_block_start")]
    ContentBlockStart {
        #[serde(rename = "index")]
        _index: usize,
        content_block: ContentBlockStartBody,
    },
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta { index: usize, delta: ContentDelta },
    #[serde(rename = "content_block_stop")]
    ContentBlockStop { index: usize },
    #[serde(rename = "message_delta")]
    MessageDelta { delta: MessageDeltaBody },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum ContentBlockStartBody {
    #[serde(rename = "tool_use")]
    ToolUse { id: String, name: String },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct ContentDelta {
    text: Option<String>,
    #[serde(rename = "partial_json")]
    partial_json: Option<String>,
}

#[derive(Deserialize)]
struct MessageDeltaBody {
    stop_reason: Option<String>,
}
