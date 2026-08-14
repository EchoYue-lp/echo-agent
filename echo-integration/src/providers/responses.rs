//! OpenAI Responses API adapter.
//!
//! The adapter keeps the framework's provider-neutral `LlmClient` contract and
//! translates full local conversation history into Responses input items. EKO
//! therefore remains file/local-history authoritative and does not depend on
//! `previous_response_id`, hosted conversations, or server-side storage.

use echo_core::error::{LlmError, Result};
use echo_core::llm::capabilities::ProviderCapabilities;
use echo_core::llm::types::{
    ChatCompletionResponse, Choice, ContentPart, DeltaFunctionCall, DeltaMessage, DeltaToolCall,
    FunctionCall, Message, MessageContent, ReasoningBlock, ResponseFormat, Role, TokenUsageDetails,
    ToolCall, ToolDefinition, Usage,
};
use echo_core::llm::{ChatChunk, ChatRequest, ChatResponse, LlmClient};
use futures::StreamExt;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use reqwest::Client;
use reqwest::header::HeaderMap;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, info_span};

use super::client::{JsonSseEvent, post_json, stream_json_sse};
use super::config::{LlmConfig, ModelConfig};
use super::openai::assemble_req_header;
use super::thinking_translate::translate_thinking_openai_compat;

/// Client for the OpenAI Responses API.
pub struct ResponsesClient {
    client: Arc<Client>,
    config: ModelConfig,
    header_map: HeaderMap,
}

impl ResponsesClient {
    /// Create a Responses client from an injected provider configuration.
    pub fn new(config: LlmConfig) -> Result<Self> {
        let model_config = config.to_model_config();
        let header_map = assemble_req_header(&model_config)?;
        Ok(Self {
            client: Arc::new(Self::build_http_client()),
            config: model_config,
            header_map,
        })
    }

    /// Create a Responses client with a shared HTTP client.
    pub fn with_client(client: Arc<Client>, config: LlmConfig) -> Result<Self> {
        let model_config = config.to_model_config();
        let header_map = assemble_req_header(&model_config)?;
        Ok(Self {
            client,
            config: model_config,
            header_map,
        })
    }

    /// Send a complete Responses request body without narrowing its schema.
    ///
    /// This is the low-level path for hosted tools, background responses,
    /// conversations, metadata, service tiers, and future Responses fields
    /// that are intentionally outside the provider-neutral [`ChatRequest`].
    pub async fn create_raw(&self, request: Value) -> Result<Value> {
        post_json(
            self.client.clone(),
            request,
            self.header_map.clone(),
            &self.config.baseurl,
        )
        .await
    }

    /// Stream complete semantic Responses events without narrowing the event schema.
    pub async fn create_raw_stream(
        &self,
        request: Value,
        cancel_token: Option<CancellationToken>,
    ) -> Result<BoxStream<'static, Result<Value>>> {
        let raw_stream = stream_json_sse(
            self.client.clone(),
            request,
            self.header_map.clone(),
            self.config.baseurl.clone(),
            self.config.model.clone(),
            cancel_token,
        )
        .await?;
        let stream = async_stream::try_stream! {
            futures::pin_mut!(raw_stream);
            while let Some(event) = raw_stream.next().await {
                match event? {
                    JsonSseEvent::Done => return,
                    JsonSseEvent::Data(value) => yield value,
                }
            }
        };
        Ok(Box::pin(stream))
    }

    fn build_http_client() -> Client {
        Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .unwrap_or_default()
    }

    fn request_body(&self, request: &ChatRequest, stream: bool) -> Value {
        let provider = self.config.provider_name.as_deref().unwrap_or("openai");
        let thinking = translate_thinking_openai_compat(
            &self.config.model,
            provider,
            &request.thinking,
            ProviderCapabilities::openai_compatible(),
        );
        let mut body = json!({
            "model": self.config.model,
            "input": messages_to_input(&request.messages),
            "store": false,
            "stream": stream,
            "include": ["reasoning.encrypted_content"],
        });
        let Some(object) = body.as_object_mut() else {
            return body;
        };

        insert_option(object, "max_output_tokens", request.max_tokens);
        if !thinking.drop_temperature {
            insert_option(object, "temperature", request.temperature);
        }
        if let Some(tools) = request.tools.as_ref() {
            object.insert("tools".to_string(), tools_to_responses(tools));
        }
        if let Some(choice) = request.tool_choice.as_deref() {
            object.insert("tool_choice".to_string(), tool_choice_to_responses(choice));
        }
        if let Some(format) = request.response_format.as_ref() {
            object.insert("text".to_string(), response_format_to_responses(format));
        }
        if let Some(effort) = thinking.reasoning_effort {
            object.insert(
                "reasoning".to_string(),
                json!({"effort": effort, "summary": "auto"}),
            );
        }
        if let Some(cache_key) = request.user_id.as_ref() {
            object.insert("prompt_cache_key".to_string(), json!(cache_key));
        }
        body
    }
}

impl LlmClient for ResponsesClient {
    fn chat(&self, request: ChatRequest) -> BoxFuture<'_, Result<ChatResponse>> {
        let model = self.config.model.clone();
        Box::pin(
            async move {
                let body = self.request_body(&request, false);
                let raw = post_json(
                    self.client.clone(),
                    body,
                    self.header_map.clone(),
                    &self.config.baseurl,
                )
                .await?;
                response_to_chat(raw)
            }
            .instrument(info_span!("openai_responses", model = %model)),
        )
    }

    fn chat_stream(
        &self,
        request: ChatRequest,
    ) -> BoxFuture<'_, Result<BoxStream<'static, Result<ChatChunk>>>> {
        let model = self.config.model.clone();
        Box::pin(
            async move {
                let cancel_token = request.cancel_token.clone();
                let body = self.request_body(&request, true);
                let raw_stream = stream_json_sse(
                    self.client.clone(),
                    body,
                    self.header_map.clone(),
                    self.config.baseurl.clone(),
                    self.config.model.clone(),
                    cancel_token,
                )
                .await?;
                let stream = async_stream::try_stream! {
                    futures::pin_mut!(raw_stream);
                    while let Some(event) = raw_stream.next().await {
                        match event? {
                            JsonSseEvent::Done => return,
                            JsonSseEvent::Data(value) => {
                                if let Some(chunk) = stream_event_to_chunk(value)? {
                                    yield chunk;
                                }
                            }
                        }
                    }
                };
                Ok(Box::pin(stream) as BoxStream<'static, Result<ChatChunk>>)
            }
            .instrument(info_span!("openai_responses_stream", model = %model)),
        )
    }

    fn model_name(&self) -> &str {
        &self.config.model
    }
}

fn insert_option<T: serde::Serialize>(
    object: &mut serde_json::Map<String, Value>,
    key: &str,
    value: Option<T>,
) {
    let Some(value) = value else {
        return;
    };
    if let Ok(value) = serde_json::to_value(value) {
        object.insert(key.to_string(), value);
    }
}

fn messages_to_input(messages: &[Message]) -> Value {
    let mut input = Vec::new();
    for message in messages {
        if matches!(message.role, Role::Assistant) {
            append_opaque_reasoning(&mut input, message.reasoning_blocks.as_deref());
        }

        if matches!(message.role, Role::Tool) {
            if let Some(call_id) = message.tool_call_id.as_ref() {
                input.push(json!({
                    "type": "function_call_output",
                    "call_id": call_id,
                    "output": message.content.as_text().unwrap_or_default(),
                }));
            }
            continue;
        }

        if !matches!(message.content, MessageContent::Empty) {
            input.push(json!({
                "type": "message",
                "role": responses_role(&message.role),
                "content": content_to_responses(&message.content),
            }));
        }

        for tool_call in message.tool_calls.as_deref().unwrap_or_default() {
            input.push(json!({
                "type": "function_call",
                "call_id": tool_call.id,
                "name": tool_call.function.name,
                "arguments": tool_call.function.arguments,
            }));
        }
    }
    Value::Array(input)
}

fn append_opaque_reasoning(target: &mut Vec<Value>, blocks: Option<&[ReasoningBlock]>) {
    for block in blocks.unwrap_or_default() {
        let ReasoningBlock::Opaque {
            provider,
            id,
            data,
            summary,
        } = block
        else {
            continue;
        };
        if provider != "openai_responses" {
            continue;
        }
        target.push(json!({
            "type": "reasoning",
            "id": id,
            "encrypted_content": data,
            "summary": summary.iter().map(|text| json!({
                "type": "summary_text",
                "text": text,
            })).collect::<Vec<_>>(),
        }));
    }
}

fn responses_role(role: &Role) -> &str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "user",
        Role::Custom(role) if role == "developer" => "developer",
        Role::Custom(_) => "user",
    }
}

fn content_to_responses(content: &MessageContent) -> Value {
    match content {
        MessageContent::Text(text) => Value::String(text.clone()),
        MessageContent::Empty => Value::String(String::new()),
        MessageContent::Parts(parts) => Value::Array(
            parts
                .iter()
                .map(|part| match part {
                    ContentPart::Text { text } => json!({
                        "type": "input_text",
                        "text": text,
                    }),
                    ContentPart::ImageUrl { image_url } => json!({
                        "type": "input_image",
                        "detail": image_url.detail.as_deref().unwrap_or("auto"),
                        "image_url": image_url.url,
                    }),
                    ContentPart::File { name, content } => json!({
                        "type": "input_file",
                        "filename": name,
                        "file_data": format!("data:application/octet-stream;base64,{content}"),
                    }),
                })
                .collect(),
        ),
    }
}

fn tools_to_responses(tools: &[ToolDefinition]) -> Value {
    Value::Array(
        tools
            .iter()
            .map(|tool| {
                json!({
                    "type": tool.tool_type,
                    "name": tool.function.name,
                    "description": tool.function.description,
                    "parameters": tool.function.parameters,
                    "strict": false,
                })
            })
            .collect(),
    )
}

fn tool_choice_to_responses(choice: &str) -> Value {
    if matches!(choice, "auto" | "none" | "required") {
        return Value::String(choice.to_string());
    }
    serde_json::from_str(choice).unwrap_or_else(|_| json!({"type": "function", "name": choice}))
}

fn response_format_to_responses(format: &ResponseFormat) -> Value {
    let format = match format {
        ResponseFormat::Text => json!({"type": "text"}),
        ResponseFormat::JsonObject => json!({"type": "json_object"}),
        ResponseFormat::JsonSchema { json_schema } => json!({
            "type": "json_schema",
            "name": json_schema.name,
            "schema": json_schema.schema,
            "strict": json_schema.strict,
        }),
    };
    json!({"format": format})
}

#[derive(Debug, Deserialize)]
struct ResponsesResponse {
    id: String,
    #[serde(default)]
    created_at: Option<u64>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    output: Vec<ResponsesOutputItem>,
    #[serde(default)]
    usage: Option<ResponsesUsage>,
    #[serde(default)]
    error: Option<ResponsesError>,
    #[serde(default)]
    incomplete_details: Option<IncompleteDetails>,
}

#[derive(Debug, Deserialize)]
struct ResponsesError {
    #[serde(default)]
    code: Option<String>,
    message: String,
}

#[derive(Debug, Deserialize)]
struct IncompleteDetails {
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ResponsesOutputItem {
    Message {
        #[serde(default)]
        content: Vec<ResponsesContent>,
    },
    FunctionCall {
        #[serde(default)]
        call_id: String,
        #[serde(default)]
        name: String,
        #[serde(default)]
        arguments: String,
    },
    Reasoning {
        #[serde(default)]
        id: String,
        #[serde(default)]
        encrypted_content: Option<String>,
        #[serde(default)]
        summary: Vec<ReasoningText>,
        #[serde(default)]
        content: Vec<ReasoningText>,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ResponsesContent {
    OutputText {
        text: String,
    },
    Refusal {
        refusal: String,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
struct ReasoningText {
    text: String,
}

#[derive(Debug, Deserialize)]
struct ResponsesUsage {
    input_tokens: u32,
    output_tokens: u32,
    total_tokens: u32,
    #[serde(default)]
    input_tokens_details: Option<TokenUsageDetails>,
    #[serde(default)]
    output_tokens_details: Option<TokenUsageDetails>,
}

impl From<ResponsesUsage> for Usage {
    fn from(value: ResponsesUsage) -> Self {
        Self {
            prompt_tokens: Some(value.input_tokens),
            completion_tokens: Some(value.output_tokens),
            total_tokens: Some(value.total_tokens),
            input_tokens_details: value.input_tokens_details,
            output_tokens_details: value.output_tokens_details,
            ..Default::default()
        }
    }
}

fn response_to_chat(raw: Value) -> Result<ChatResponse> {
    let response: ResponsesResponse = serde_json::from_value(raw.clone()).map_err(|error| {
        LlmError::InvalidResponse(format!("invalid Responses response: {error}"))
    })?;
    if let Some(error) = response.error {
        let code = error
            .code
            .map(|code| format!("{code}: "))
            .unwrap_or_default();
        return Err(LlmError::InvalidResponse(format!("{code}{}", error.message)).into());
    }

    let mut text = String::new();
    let mut tool_calls = Vec::new();
    let mut reasoning_text = Vec::new();
    let mut reasoning_blocks = Vec::new();
    for item in response.output {
        match item {
            ResponsesOutputItem::Message { content } => {
                for part in content {
                    match part {
                        ResponsesContent::OutputText { text: value } => text.push_str(&value),
                        ResponsesContent::Refusal { refusal } => text.push_str(&refusal),
                        ResponsesContent::Other => {}
                    }
                }
            }
            ResponsesOutputItem::FunctionCall {
                call_id,
                name,
                arguments,
            } => tool_calls.push(ToolCall {
                id: call_id,
                call_type: "function".to_string(),
                function: FunctionCall { name, arguments },
            }),
            ResponsesOutputItem::Reasoning {
                id,
                encrypted_content,
                summary,
                content,
            } => {
                let summaries = summary
                    .into_iter()
                    .map(|part| part.text)
                    .collect::<Vec<_>>();
                reasoning_text.extend(content.into_iter().map(|part| part.text));
                if let Some(data) = encrypted_content {
                    reasoning_blocks.push(ReasoningBlock::Opaque {
                        provider: "openai_responses".to_string(),
                        id,
                        data,
                        summary: summaries.clone(),
                    });
                }
                if reasoning_text.is_empty() {
                    reasoning_text.extend(summaries);
                }
            }
            ResponsesOutputItem::Other => {}
        }
    }

    let has_tool_calls = !tool_calls.is_empty();
    let message = Message {
        role: Role::Assistant,
        content: if text.is_empty() {
            MessageContent::Empty
        } else {
            MessageContent::Text(text)
        },
        tool_calls: has_tool_calls.then_some(tool_calls),
        reasoning_content: (!reasoning_text.is_empty()).then(|| reasoning_text.join("")),
        reasoning_blocks: (!reasoning_blocks.is_empty()).then_some(reasoning_blocks),
        ..Default::default()
    };
    let usage = response.usage.map(Usage::from);
    let finish_reason = finish_reason(
        response.status.as_deref(),
        has_tool_calls,
        response.incomplete_details.as_ref(),
    );
    let compat_raw = ChatCompletionResponse {
        id: response.id,
        choices: vec![Choice {
            message: message.clone(),
            finish_reason: finish_reason.clone(),
            index: Some(0),
        }],
        created: response.created_at,
        model: response.model,
        usage: usage.clone(),
        extra: Some(raw),
    };
    Ok(ChatResponse {
        message,
        finish_reason,
        usage,
        raw: compat_raw,
    })
}

fn finish_reason(
    status: Option<&str>,
    has_tool_calls: bool,
    incomplete: Option<&IncompleteDetails>,
) -> Option<String> {
    if status == Some("incomplete") {
        let reason = incomplete.and_then(|details| details.reason.as_deref());
        return Some(match reason {
            Some("max_output_tokens") | None => "length".to_string(),
            Some(value) => value.to_string(),
        });
    }
    Some(if has_tool_calls { "tool_calls" } else { "stop" }.to_string())
}

fn stream_event_to_chunk(event: Value) -> Result<Option<ChatChunk>> {
    let event_type = event
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match event_type {
        "response.output_text.delta" => Ok(Some(text_chunk(&event, false))),
        "response.reasoning_text.delta" | "response.reasoning_summary_text.delta" => {
            Ok(Some(text_chunk(&event, true)))
        }
        "response.output_item.added" => output_item_added_chunk(&event),
        "response.output_item.done" => output_item_done_chunk(&event),
        "response.function_call_arguments.delta" => Ok(Some(ChatChunk {
            delta: DeltaMessage {
                tool_calls: Some(vec![DeltaToolCall {
                    index: event_u32(&event, "output_index"),
                    id: None,
                    call_type: None,
                    function: Some(DeltaFunctionCall {
                        name: None,
                        arguments: event_string(&event, "delta"),
                    }),
                }]),
                ..Default::default()
            },
            finish_reason: None,
            usage: None,
        })),
        "response.completed" | "response.incomplete" => terminal_chunk(&event),
        "response.failed" => Err(stream_failure(&event).into()),
        "error" => Err(LlmError::InvalidResponse(
            event_string(&event, "message").unwrap_or_else(|| "Responses stream error".to_string()),
        )
        .into()),
        _ => Ok(None),
    }
}

fn text_chunk(event: &Value, reasoning: bool) -> ChatChunk {
    let delta = event_string(event, "delta");
    ChatChunk {
        delta: DeltaMessage {
            content: (!reasoning).then_some(delta.clone()).flatten(),
            reasoning_content: reasoning.then_some(delta).flatten(),
            ..Default::default()
        },
        finish_reason: None,
        usage: None,
    }
}

fn output_item_added_chunk(event: &Value) -> Result<Option<ChatChunk>> {
    let Some(item) = event.get("item") else {
        return Ok(None);
    };
    if item.get("type").and_then(Value::as_str) != Some("function_call") {
        return Ok(None);
    }
    Ok(Some(ChatChunk {
        delta: DeltaMessage {
            tool_calls: Some(vec![DeltaToolCall {
                index: event_u32(event, "output_index"),
                id: event_string(item, "call_id"),
                call_type: Some("function".to_string()),
                function: Some(DeltaFunctionCall {
                    name: event_string(item, "name"),
                    arguments: None,
                }),
            }]),
            ..Default::default()
        },
        finish_reason: None,
        usage: None,
    }))
}

fn output_item_done_chunk(event: &Value) -> Result<Option<ChatChunk>> {
    let Some(item) = event.get("item") else {
        return Ok(None);
    };
    if item.get("type").and_then(Value::as_str) != Some("reasoning") {
        return Ok(None);
    }
    let Some(data) = event_string(item, "encrypted_content") else {
        return Ok(None);
    };
    let summary = item
        .get("summary")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|part| event_string(part, "text"))
        .collect();
    Ok(Some(ChatChunk {
        delta: DeltaMessage {
            reasoning_blocks: Some(vec![ReasoningBlock::Opaque {
                provider: "openai_responses".to_string(),
                id: event_string(item, "id").unwrap_or_default(),
                data,
                summary,
            }]),
            ..Default::default()
        },
        finish_reason: None,
        usage: None,
    }))
}

fn terminal_chunk(event: &Value) -> Result<Option<ChatChunk>> {
    let Some(response) = event.get("response") else {
        return Err(LlmError::InvalidResponse(
            "Responses terminal event omitted response".to_string(),
        )
        .into());
    };
    let parsed: ResponsesResponse = serde_json::from_value(response.clone()).map_err(|error| {
        LlmError::InvalidResponse(format!("invalid terminal Responses event: {error}"))
    })?;
    let has_tool_calls = parsed
        .output
        .iter()
        .any(|item| matches!(item, ResponsesOutputItem::FunctionCall { .. }));
    Ok(Some(ChatChunk {
        delta: DeltaMessage::default(),
        finish_reason: finish_reason(
            parsed.status.as_deref(),
            has_tool_calls,
            parsed.incomplete_details.as_ref(),
        ),
        usage: parsed.usage.map(Usage::from),
    }))
}

fn stream_failure(event: &Value) -> LlmError {
    let message = event
        .get("response")
        .and_then(|response| response.get("error"))
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("Responses stream failed");
    LlmError::InvalidResponse(message.to_string())
}

fn event_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn event_u32(value: &Value, key: &str) -> u32 {
    value
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|number| u32::try_from(number).ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::llm::types::{FunctionSpec, ImageUrl};

    fn test_config() -> LlmConfig {
        LlmConfig::openai("test-key", "gpt-test")
    }

    #[test]
    fn request_maps_messages_tools_multimodal_and_structured_output() -> Result<()> {
        let client = ResponsesClient::new(test_config())?;
        let mut assistant = Message::assistant("checking".to_string());
        assistant.tool_calls = Some(vec![ToolCall {
            id: "call_1".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "lookup".to_string(),
                arguments: "{\"q\":\"rust\"}".to_string(),
            },
        }]);
        let mut image = Message::user(String::new());
        image.content = MessageContent::Parts(vec![ContentPart::ImageUrl {
            image_url: ImageUrl {
                url: "https://example.com/image.png".to_string(),
                detail: Some("high".to_string()),
            },
        }]);
        let request = ChatRequest {
            messages: vec![
                Message::system("system".to_string()),
                image,
                assistant,
                Message::tool_result(
                    "call_1".to_string(),
                    "lookup".to_string(),
                    "result".to_string(),
                ),
            ],
            tools: Some(vec![ToolDefinition {
                tool_type: "function".to_string(),
                function: FunctionSpec {
                    name: "lookup".to_string(),
                    description: "Lookup".to_string(),
                    parameters: json!({"type": "object"}),
                },
            }]),
            response_format: Some(ResponseFormat::json_schema(
                "answer",
                json!({"type": "object"}),
            )),
            user_id: Some("cache-key".to_string()),
            ..Default::default()
        };
        let body = client.request_body(&request, false);
        assert_eq!(body["store"], false);
        assert_eq!(body["prompt_cache_key"], "cache-key");
        assert_eq!(body["tools"][0]["name"], "lookup");
        assert_eq!(body["text"]["format"]["type"], "json_schema");
        let input = body["input"]
            .as_array()
            .ok_or_else(|| LlmError::InvalidResponse("input was not an array".to_string()))?;
        assert!(input.iter().any(|item| item["type"] == "function_call"));
        assert!(
            input
                .iter()
                .any(|item| item["type"] == "function_call_output")
        );
        assert!(
            input
                .iter()
                .any(|item| item["content"][0]["type"] == "input_image")
        );
        Ok(())
    }

    #[test]
    fn response_maps_text_tools_reasoning_and_usage() -> Result<()> {
        let raw = json!({
            "id": "resp_1",
            "status": "completed",
            "model": "gpt-test",
            "output": [
                {"type":"reasoning","id":"rs_1","summary":[{"type":"summary_text","text":"summary"}],"encrypted_content":"encrypted"},
                {"type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":"hello","annotations":[]}]},
                {"type":"function_call","call_id":"call_1","name":"lookup","arguments":"{}","status":"completed"}
            ],
            "usage": {"input_tokens":10,"output_tokens":4,"total_tokens":14,"input_tokens_details":{"cached_tokens":6,"cache_write_tokens":2},"output_tokens_details":{"reasoning_tokens":2}}
        });
        let response = response_to_chat(raw)?;
        assert_eq!(response.content().as_deref(), Some("hello"));
        assert_eq!(response.finish_reason.as_deref(), Some("tool_calls"));
        assert_eq!(
            response
                .tool_calls()
                .and_then(|calls| calls.first())
                .map(|call| call.id.as_str()),
            Some("call_1")
        );
        assert_eq!(
            response.usage.as_ref().map(Usage::cached_prompt_tokens),
            Some(6)
        );
        assert_eq!(
            response
                .usage
                .as_ref()
                .and_then(|usage| usage.output_tokens_details.as_ref())
                .and_then(|details| details.reasoning_tokens),
            Some(2)
        );
        assert!(response.message.reasoning_blocks.as_ref().is_some_and(|blocks| {
            matches!(blocks.first(), Some(ReasoningBlock::Opaque { id, .. }) if id == "rs_1")
        }));
        Ok(())
    }

    #[test]
    fn semantic_stream_events_map_without_done_sentinel() -> Result<()> {
        let text = stream_event_to_chunk(json!({
            "type":"response.output_text.delta","delta":"你"
        }))?
        .ok_or_else(|| LlmError::InvalidResponse("text event was dropped".to_string()))?;
        assert_eq!(text.delta.content.as_deref(), Some("你"));

        let call = stream_event_to_chunk(json!({
            "type":"response.output_item.added","output_index":2,
            "item":{"type":"function_call","call_id":"call_7","name":"read","arguments":""}
        }))?
        .ok_or_else(|| LlmError::InvalidResponse("tool event was dropped".to_string()))?;
        let tool_delta = call
            .delta
            .tool_calls
            .as_ref()
            .and_then(|calls| calls.first())
            .ok_or_else(|| LlmError::InvalidResponse("tool delta missing".to_string()))?;
        assert_eq!(tool_delta.index, 2);
        assert_eq!(tool_delta.id.as_deref(), Some("call_7"));

        let terminal = stream_event_to_chunk(json!({
            "type":"response.completed",
            "response":{"id":"resp_1","status":"completed","output":[],"usage":{"input_tokens":3,"output_tokens":2,"total_tokens":5,"input_tokens_details":{"cached_tokens":0,"cache_write_tokens":0},"output_tokens_details":{"reasoning_tokens":0}}}
        }))?
        .ok_or_else(|| LlmError::InvalidResponse("terminal event was dropped".to_string()))?;
        assert_eq!(terminal.finish_reason.as_deref(), Some("stop"));
        assert_eq!(terminal.usage.and_then(|usage| usage.total_tokens), Some(5));
        Ok(())
    }
}
