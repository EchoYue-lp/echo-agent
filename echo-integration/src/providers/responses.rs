//! OpenAI Responses API adapter.
//!
//! The adapter keeps the framework's provider-neutral `LlmClient` contract and
//! translates full local conversation history into Responses input items. embedding application
//! therefore remains file/local-history authoritative and does not depend on
//! `previous_response_id`, hosted conversations, or server-side storage.

use echo_core::error::{LlmError, Result};
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
use std::collections::HashMap;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, info_span};

use super::client::{JsonSseEvent, post_json, stream_json_sse};
use super::config::LlmConfig;
use super::openai::assemble_req_header;
use super::thinking_translate::translate_thinking_openai_compat;

/// Client for the OpenAI Responses API.
pub struct ResponsesClient {
    client: Arc<Client>,
    config: LlmConfig,
    header_map: HeaderMap,
}

impl ResponsesClient {
    /// Create a Responses client from an injected provider configuration.
    pub fn new(config: LlmConfig) -> Result<Self> {
        let header_map = assemble_req_header(&config)?;
        Ok(Self {
            client: Arc::new(Self::build_http_client()),
            config,
            header_map,
        })
    }

    /// Create a Responses client with a shared HTTP client.
    pub fn with_client(client: Arc<Client>, config: LlmConfig) -> Result<Self> {
        let header_map = assemble_req_header(&config)?;
        Ok(Self {
            client,
            config,
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
            &self.config.base_url,
            self.config.timeouts,
        )
        .await
    }

    /// Stream complete semantic Responses events without narrowing the event schema.
    pub async fn create_raw_stream(
        &self,
        request: Value,
        cancel_token: Option<CancellationToken>,
    ) -> Result<BoxStream<'static, Result<Value>>> {
        let request = self
            .client
            .post(&self.config.base_url)
            .headers(self.header_map.clone())
            .json(&request);
        let raw_stream = stream_json_sse(
            request,
            self.config.model.clone(),
            self.config.timeouts,
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
        Client::new()
    }

    fn request_body(&self, request: &ChatRequest, stream: bool) -> Value {
        let thinking = translate_thinking_openai_compat(
            &self.config.model,
            self.config.api_protocol,
            self.config.thinking_protocol,
            &request.thinking,
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
                self.config.validate_input_modalities(&request.messages)?;
                let timeouts = request.timeouts.unwrap_or(self.config.timeouts);
                let body = self.request_body(&request, false);
                let raw = post_json(
                    self.client.clone(),
                    body,
                    self.header_map.clone(),
                    &self.config.base_url,
                    timeouts,
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
                self.config.validate_input_modalities(&request.messages)?;
                let timeouts = request.timeouts.unwrap_or(self.config.timeouts);
                let cancel_token = request.cancel_token.clone();
                let body = self.request_body(&request, true);
                let request = self
                    .client
                    .post(&self.config.base_url)
                    .headers(self.header_map.clone())
                    .json(&body);
                let raw_stream =
                    stream_json_sse(request, self.config.model.clone(), timeouts, cancel_token)
                        .await?;
                Ok(adapt_responses_stream(raw_stream))
            }
            .instrument(info_span!("openai_responses_stream", model = %model)),
        )
    }

    fn model_name(&self) -> &str {
        &self.config.model
    }
}

fn adapt_responses_stream<S>(raw_stream: S) -> BoxStream<'static, Result<ChatChunk>>
where
    S: futures::Stream<Item = Result<JsonSseEvent>> + Send + 'static,
{
    let stream = async_stream::try_stream! {
        let mut adapter = ResponsesStreamAdapter::default();
        futures::pin_mut!(raw_stream);
        while let Some(event) = raw_stream.next().await {
            match event? {
                JsonSseEvent::Done => {
                    adapter.validate_transport_end("SSE [DONE]")?;
                    return;
                }
                JsonSseEvent::Data(value) => {
                    if let Some(chunk) = adapter.map_event(value)? {
                        yield chunk;
                    }
                }
            }
        }
        adapter.validate_transport_end("SSE EOF")?;
    };
    Box::pin(stream)
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
                    ContentPart::ResourceLink { resource } => json!({
                        "type": "input_text",
                        "text": resource.model_text(),
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
    status: String,
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
        call_id: String,
        name: String,
        arguments: String,
    },
    Reasoning {
        id: String,
        encrypted_content: String,
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

fn invalid_response(message: impl Into<String>) -> echo_core::error::ReactError {
    LlmError::InvalidResponse(message.into()).into()
}

fn require_non_empty(value: &str, field: &str) -> Result<()> {
    if value.is_empty() {
        return Err(invalid_response(format!(
            "Responses response omitted required {field}"
        )));
    }
    Ok(())
}

fn ensure_completed_response(response: &ResponsesResponse, context: &str) -> Result<()> {
    require_non_empty(&response.id, "response id")?;
    if let Some(error) = response.error.as_ref() {
        let code = error
            .code
            .as_deref()
            .map(|code| format!("{code}: "))
            .unwrap_or_default();
        return Err(invalid_response(format!("{code}{}", error.message)));
    }
    if response.status != "completed" {
        let detail = response
            .incomplete_details
            .as_ref()
            .and_then(|details| details.reason.as_deref())
            .map(|reason| format!(" ({reason})"))
            .unwrap_or_default();
        return Err(invalid_response(format!(
            "{context} ended with non-completed status '{}'{}",
            response.status, detail
        )));
    }
    Ok(())
}

fn response_to_chat(raw: Value) -> Result<ChatResponse> {
    let response: ResponsesResponse = serde_json::from_value(raw.clone()).map_err(|error| {
        LlmError::InvalidResponse(format!("invalid Responses response: {error}"))
    })?;
    ensure_completed_response(&response, "Responses response")?;

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
            } => {
                require_non_empty(&call_id, "function_call.call_id")?;
                require_non_empty(&name, "function_call.name")?;
                require_non_empty(&arguments, "function_call.arguments")?;
                tool_calls.push(ToolCall {
                    id: call_id,
                    call_type: "function".to_string(),
                    function: FunctionCall { name, arguments },
                });
            }
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
                require_non_empty(&id, "reasoning.id")?;
                require_non_empty(&encrypted_content, "reasoning.encrypted_content")?;
                reasoning_blocks.push(ReasoningBlock::Opaque {
                    provider: "openai_responses".to_string(),
                    id,
                    data: encrypted_content,
                    summary: summaries.clone(),
                });
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
    let finish_reason = finish_reason(has_tool_calls);
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

fn finish_reason(has_tool_calls: bool) -> Option<String> {
    Some(if has_tool_calls { "tool_calls" } else { "stop" }.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum StreamTextKind {
    Output,
    Refusal,
    Reasoning,
    ReasoningSummary,
}

impl StreamTextKind {
    fn index_field(self) -> &'static str {
        match self {
            Self::ReasoningSummary => "summary_index",
            Self::Output | Self::Refusal | Self::Reasoning => "content_index",
        }
    }

    fn is_reasoning(self) -> bool {
        matches!(self, Self::Reasoning | Self::ReasoningSummary)
    }
}

#[derive(Debug, PartialEq, Eq, Hash)]
struct StreamTextKey {
    kind: StreamTextKind,
    output_index: u32,
    content_index: u32,
    item_id: String,
}

#[derive(Debug)]
struct StreamFunctionCall {
    item_id: String,
    call_id: String,
    name: String,
    arguments: String,
}

#[derive(Default)]
struct ResponsesStreamAdapter {
    function_calls: HashMap<u32, StreamFunctionCall>,
    text: HashMap<StreamTextKey, String>,
    completed: bool,
}

impl ResponsesStreamAdapter {
    fn map_event(&mut self, event: Value) -> Result<Option<ChatChunk>> {
        let event_type = required_non_empty_string(&event, "type", "Responses stream event")?;
        if self.completed {
            return Err(invalid_response(format!(
                "Responses stream emitted {event_type} after response.completed"
            )));
        }
        match event_type {
            "response.output_text.delta" => self.text_delta(&event, StreamTextKind::Output),
            "response.refusal.delta" => self.text_delta(&event, StreamTextKind::Refusal),
            "response.reasoning_text.delta" => self.text_delta(&event, StreamTextKind::Reasoning),
            "response.reasoning_summary_text.delta" => {
                self.text_delta(&event, StreamTextKind::ReasoningSummary)
            }
            "response.output_text.done" => self.text_done(&event, StreamTextKind::Output, "text"),
            "response.refusal.done" => self.text_done(&event, StreamTextKind::Refusal, "refusal"),
            "response.reasoning_text.done" => {
                self.text_done(&event, StreamTextKind::Reasoning, "text")
            }
            "response.reasoning_summary_text.done" => {
                self.text_done(&event, StreamTextKind::ReasoningSummary, "text")
            }
            "response.output_item.added" => self.output_item_added(&event),
            "response.output_item.done" => self.output_item_done(&event),
            "response.function_call_arguments.delta" => self.function_arguments_delta(&event),
            "response.function_call_arguments.done" => self.function_arguments_done(&event),
            "response.completed" => {
                let chunk = terminal_chunk(&event)?;
                self.completed = true;
                Ok(chunk)
            }
            "response.incomplete" | "response.failed" | "response.cancelled" => {
                Err(stream_failure(&event).into())
            }
            "error" => Err(invalid_response(
                event_string(&event, "message")
                    .unwrap_or_else(|| "Responses stream error".to_string()),
            )),
            _ => Ok(None),
        }
    }

    fn validate_transport_end(&self, boundary: &str) -> Result<()> {
        if !self.completed {
            return Err(invalid_response(format!(
                "Responses stream reached {boundary} before response.completed"
            )));
        }
        Ok(())
    }

    fn text_delta(&mut self, event: &Value, kind: StreamTextKind) -> Result<Option<ChatChunk>> {
        let key = stream_text_key(event, kind)?;
        let delta = required_string(event, "delta", "Responses text delta")?;
        self.text.entry(key).or_default().push_str(delta);
        if delta.is_empty() {
            return Ok(None);
        }
        Ok(Some(text_chunk(delta.to_string(), kind.is_reasoning())))
    }

    fn text_done(
        &mut self,
        event: &Value,
        kind: StreamTextKind,
        value_field: &str,
    ) -> Result<Option<ChatChunk>> {
        let key = stream_text_key(event, kind)?;
        let completed = required_string(event, value_field, "Responses text done event")?;
        let accumulated = self.text.entry(key).or_default();
        let suffix = reconcile_terminal_text(accumulated, completed, value_field)?;
        Ok(suffix.map(|text| text_chunk(text, kind.is_reasoning())))
    }

    fn output_item_added(&mut self, event: &Value) -> Result<Option<ChatChunk>> {
        let output_index = required_u32(event, "output_index", "response.output_item.added")?;
        let item = required_object(event, "item", "response.output_item.added")?;
        let item_id = required_non_empty_string(item, "id", "Responses output item")?;
        let item_type = required_non_empty_string(item, "type", "Responses output item")?;
        if item_type != "function_call" {
            return Ok(None);
        }
        if self.function_calls.contains_key(&output_index) {
            return Err(invalid_response(format!(
                "duplicate function_call output_index {output_index}"
            )));
        }

        let call_id = required_non_empty_string(item, "call_id", "Responses function_call")?;
        let name = required_non_empty_string(item, "name", "Responses function_call")?;
        let arguments = required_string(item, "arguments", "Responses function_call")?;
        let initial_arguments = (!arguments.is_empty()).then(|| arguments.to_string());
        self.function_calls.insert(
            output_index,
            StreamFunctionCall {
                item_id: item_id.to_string(),
                call_id: call_id.to_string(),
                name: name.to_string(),
                arguments: arguments.to_string(),
            },
        );
        Ok(Some(tool_chunk(
            output_index,
            Some(call_id.to_string()),
            Some(name.to_string()),
            initial_arguments,
        )))
    }

    fn function_arguments_delta(&mut self, event: &Value) -> Result<Option<ChatChunk>> {
        let output_index = required_u32(
            event,
            "output_index",
            "response.function_call_arguments.delta",
        )?;
        let item_id =
            required_non_empty_string(event, "item_id", "response.function_call_arguments.delta")?;
        let delta = required_string(event, "delta", "response.function_call_arguments.delta")?;
        let state = self.function_calls.get_mut(&output_index).ok_or_else(|| {
            invalid_response(format!(
                "function arguments delta preceded function_call item {output_index}"
            ))
        })?;
        validate_item_id(state, item_id, output_index)?;
        state.arguments.push_str(delta);
        if delta.is_empty() {
            return Ok(None);
        }
        Ok(Some(tool_chunk(
            output_index,
            None,
            None,
            Some(delta.to_string()),
        )))
    }

    fn function_arguments_done(&mut self, event: &Value) -> Result<Option<ChatChunk>> {
        let output_index = required_u32(
            event,
            "output_index",
            "response.function_call_arguments.done",
        )?;
        let item_id =
            required_non_empty_string(event, "item_id", "response.function_call_arguments.done")?;
        let arguments =
            required_non_empty_string(event, "arguments", "response.function_call_arguments.done")?;
        let state = self.function_calls.get_mut(&output_index).ok_or_else(|| {
            invalid_response(format!(
                "function arguments done preceded function_call item {output_index}"
            ))
        })?;
        validate_item_id(state, item_id, output_index)?;
        let suffix = reconcile_terminal_text(&mut state.arguments, arguments, "arguments")?;
        Ok(suffix.map(|arguments| tool_chunk(output_index, None, None, Some(arguments))))
    }

    fn output_item_done(&mut self, event: &Value) -> Result<Option<ChatChunk>> {
        let output_index = required_u32(event, "output_index", "response.output_item.done")?;
        let item = required_object(event, "item", "response.output_item.done")?;
        let item_id = required_non_empty_string(item, "id", "Responses output item")?;
        let item_type = required_non_empty_string(item, "type", "Responses output item")?;
        match item_type {
            "function_call" => self.function_call_item_done(output_index, item_id, item),
            "message" => self.message_item_done(output_index, item_id, item),
            "reasoning" => reasoning_item_done(item),
            _ => Ok(None),
        }
    }

    fn message_item_done(
        &mut self,
        output_index: u32,
        item_id: &str,
        item: &Value,
    ) -> Result<Option<ChatChunk>> {
        let content = required_array(item, "content", "Responses message item")?;
        let mut unseen = String::new();
        for (index, part) in content.iter().enumerate() {
            let content_index = u32::try_from(index)
                .map_err(|_| invalid_response("Responses message content index exceeded u32"))?;
            let part_type =
                required_non_empty_string(part, "type", "Responses message content part")?;
            let (kind, field) = match part_type {
                "output_text" => (StreamTextKind::Output, "text"),
                "refusal" => (StreamTextKind::Refusal, "refusal"),
                _ => continue,
            };
            let completed =
                required_string(part, field, "Responses terminal message content part")?;
            let key = StreamTextKey {
                kind,
                output_index,
                content_index,
                item_id: item_id.to_string(),
            };
            let accumulated = self.text.entry(key).or_default();
            if let Some(suffix) = reconcile_terminal_text(accumulated, completed, field)? {
                unseen.push_str(&suffix);
            }
        }
        Ok((!unseen.is_empty()).then(|| text_chunk(unseen, false)))
    }

    fn function_call_item_done(
        &mut self,
        output_index: u32,
        item_id: &str,
        item: &Value,
    ) -> Result<Option<ChatChunk>> {
        let call_id = required_non_empty_string(item, "call_id", "Responses function_call")?;
        let name = required_non_empty_string(item, "name", "Responses function_call")?;
        let arguments = required_non_empty_string(item, "arguments", "Responses function_call")?;
        let Some(state) = self.function_calls.get_mut(&output_index) else {
            self.function_calls.insert(
                output_index,
                StreamFunctionCall {
                    item_id: item_id.to_string(),
                    call_id: call_id.to_string(),
                    name: name.to_string(),
                    arguments: arguments.to_string(),
                },
            );
            return Ok(Some(tool_chunk(
                output_index,
                Some(call_id.to_string()),
                Some(name.to_string()),
                Some(arguments.to_string()),
            )));
        };
        validate_item_id(state, item_id, output_index)?;
        if state.call_id != call_id || state.name != name {
            return Err(invalid_response(format!(
                "function_call identity changed at output_index {output_index}"
            )));
        }
        let suffix = reconcile_terminal_text(&mut state.arguments, arguments, "arguments")?;
        Ok(suffix.map(|arguments| tool_chunk(output_index, None, None, Some(arguments))))
    }
}

fn stream_text_key(event: &Value, kind: StreamTextKind) -> Result<StreamTextKey> {
    Ok(StreamTextKey {
        kind,
        output_index: required_u32(event, "output_index", "Responses text event")?,
        content_index: required_u32(event, kind.index_field(), "Responses text event")?,
        item_id: required_non_empty_string(event, "item_id", "Responses text event")?.to_string(),
    })
}

fn reconcile_terminal_text(
    accumulated: &mut String,
    completed: &str,
    field: &str,
) -> Result<Option<String>> {
    if accumulated.as_str() == completed {
        return Ok(None);
    }
    let Some(suffix) = completed.strip_prefix(accumulated.as_str()) else {
        return Err(invalid_response(format!(
            "Responses {field} done payload disagreed with accumulated deltas"
        )));
    };
    let suffix = suffix.to_string();
    *accumulated = completed.to_string();
    Ok((!suffix.is_empty()).then_some(suffix))
}

fn validate_item_id(state: &StreamFunctionCall, item_id: &str, output_index: u32) -> Result<()> {
    if state.item_id != item_id {
        return Err(invalid_response(format!(
            "function_call item_id changed at output_index {output_index}"
        )));
    }
    Ok(())
}

fn text_chunk(text: String, reasoning: bool) -> ChatChunk {
    ChatChunk {
        delta: DeltaMessage {
            content: (!reasoning).then_some(text.clone()),
            reasoning_content: reasoning.then_some(text),
            ..Default::default()
        },
        finish_reason: None,
        usage: None,
    }
}

fn tool_chunk(
    index: u32,
    id: Option<String>,
    name: Option<String>,
    arguments: Option<String>,
) -> ChatChunk {
    ChatChunk {
        delta: DeltaMessage {
            tool_calls: Some(vec![DeltaToolCall {
                index,
                id,
                call_type: Some("function".to_string()),
                function: Some(DeltaFunctionCall { name, arguments }),
            }]),
            ..Default::default()
        },
        finish_reason: None,
        usage: None,
    }
}

fn reasoning_item_done(item: &Value) -> Result<Option<ChatChunk>> {
    let id = required_non_empty_string(item, "id", "Responses reasoning item")?;
    let data = required_non_empty_string(item, "encrypted_content", "Responses reasoning item")?;
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
                id: id.to_string(),
                data: data.to_string(),
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
    ensure_completed_response(&parsed, "Responses terminal event")?;
    let has_tool_calls = parsed
        .output
        .iter()
        .any(|item| matches!(item, ResponsesOutputItem::FunctionCall { .. }));
    Ok(Some(ChatChunk {
        delta: DeltaMessage::default(),
        finish_reason: finish_reason(has_tool_calls),
        usage: parsed.usage.map(Usage::from),
    }))
}

fn stream_failure(event: &Value) -> LlmError {
    let explicit_message = event
        .get("response")
        .and_then(|response| response.get("error"))
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str);
    let event_type = event
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("unknown terminal event");
    let status = event
        .get("response")
        .and_then(|response| response.get("status"))
        .and_then(Value::as_str)
        .unwrap_or(event_type);
    LlmError::InvalidResponse(
        explicit_message
            .map(ToString::to_string)
            .unwrap_or_else(|| format!("Responses stream ended with {status}")),
    )
}

fn event_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn required_object<'a>(value: &'a Value, key: &str, context: &str) -> Result<&'a Value> {
    value
        .get(key)
        .filter(|field| field.is_object())
        .ok_or_else(|| invalid_response(format!("{context} omitted required object {key}")))
}

fn required_array<'a>(value: &'a Value, key: &str, context: &str) -> Result<&'a [Value]> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| invalid_response(format!("{context} omitted required array {key}")))
}

fn required_string<'a>(value: &'a Value, key: &str, context: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_response(format!("{context} omitted required string {key}")))
}

fn required_non_empty_string<'a>(value: &'a Value, key: &str, context: &str) -> Result<&'a str> {
    let field = required_string(value, key, context)?;
    if field.is_empty() {
        return Err(invalid_response(format!(
            "{context} supplied empty required string {key}"
        )));
    }
    Ok(field)
}

fn required_u32(value: &Value, key: &str, context: &str) -> Result<u32> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|number| u32::try_from(number).ok())
        .ok_or_else(|| invalid_response(format!("{context} omitted required integer {key}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::llm::types::{FunctionSpec, ImageUrl};

    fn test_config() -> LlmConfig {
        LlmConfig {
            provider_name: Some("test-provider".to_string()),
            api_protocol: echo_core::llm::LlmApiProtocol::Responses,
            base_url: "https://api.example.test/v1/responses".to_string(),
            api_key: "test-key".to_string(),
            model: "gpt-test".to_string(),
            input_modalities: echo_core::llm::ModelInputModality::all_supported(),
            thinking_protocol: echo_core::llm::ThinkingProtocol::OpenaiReasoningEffort,
            timeouts: echo_core::llm::LlmTimeouts::default(),
        }
    }

    fn is_invalid_response<T>(result: &Result<T>) -> bool {
        matches!(
            result,
            Err(echo_core::error::ReactError::Llm(error))
                if matches!(error.as_ref(), LlmError::InvalidResponse(_))
        )
    }

    fn required_chunk(result: Result<Option<ChatChunk>>, context: &str) -> Result<ChatChunk> {
        result?.ok_or_else(|| invalid_response(format!("{context} was dropped")))
    }

    fn first_tool_delta(chunk: &ChatChunk) -> Result<&DeltaToolCall> {
        chunk
            .delta
            .tool_calls
            .as_ref()
            .and_then(|calls| calls.first())
            .ok_or_else(|| invalid_response("test chunk omitted tool delta"))
    }

    fn tool_arguments(chunk: &ChatChunk) -> Option<&str> {
        chunk
            .delta
            .tool_calls
            .as_ref()
            .and_then(|calls| calls.first())
            .and_then(|call| call.function.as_ref())
            .and_then(|function| function.arguments.as_deref())
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
        assert_eq!(body.get("store").and_then(Value::as_bool), Some(false));
        assert_eq!(
            body.get("prompt_cache_key").and_then(Value::as_str),
            Some("cache-key")
        );
        assert_eq!(
            body.get("tools")
                .and_then(Value::as_array)
                .and_then(|tools| tools.first())
                .and_then(|tool| tool.get("name"))
                .and_then(Value::as_str),
            Some("lookup")
        );
        assert_eq!(
            body.get("text")
                .and_then(|text| text.get("format"))
                .and_then(|format| format.get("type"))
                .and_then(Value::as_str),
            Some("json_schema")
        );
        let input = body
            .get("input")
            .and_then(Value::as_array)
            .ok_or_else(|| LlmError::InvalidResponse("input was not an array".to_string()))?;
        assert!(
            input
                .iter()
                .any(|item| { item.get("type").and_then(Value::as_str) == Some("function_call") })
        );
        let function_output = input
            .iter()
            .find(|item| item.get("type").and_then(Value::as_str) == Some("function_call_output"))
            .ok_or_else(|| {
                LlmError::InvalidResponse("function call output was not mapped".to_string())
            })?;
        assert!(function_output.get("name").is_none());
        assert!(input.iter().any(|item| {
            item.get("content")
                .and_then(Value::as_array)
                .and_then(|content| content.first())
                .and_then(|part| part.get("type"))
                .and_then(Value::as_str)
                == Some("input_image")
        }));
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
    fn nonstream_rejects_every_non_completed_or_missing_status() {
        for status in ["failed", "cancelled", "incomplete", "in_progress"] {
            let result = response_to_chat(json!({
                "id": "resp_1",
                "status": status,
                "output": []
            }));
            assert!(is_invalid_response(&result), "status {status} was accepted");
        }
        let missing_status = response_to_chat(json!({"id": "resp_1", "output": []}));
        assert!(is_invalid_response(&missing_status));
    }

    #[test]
    fn nonstream_rejects_missing_or_empty_function_identity() {
        let invalid_items = [
            json!({"type":"function_call","name":"lookup","arguments":"{}"}),
            json!({"type":"function_call","call_id":"","name":"lookup","arguments":"{}"}),
            json!({"type":"function_call","call_id":"call_1","arguments":"{}"}),
            json!({"type":"function_call","call_id":"call_1","name":"","arguments":"{}"}),
            json!({"type":"function_call","call_id":"call_1","name":"lookup"}),
            json!({"type":"function_call","call_id":"call_1","name":"lookup","arguments":""}),
        ];
        for item in invalid_items {
            let result = response_to_chat(json!({
                "id": "resp_1",
                "status": "completed",
                "output": [item]
            }));
            assert!(is_invalid_response(&result));
        }
    }

    #[test]
    fn nonstream_rejects_missing_or_empty_opaque_reasoning_identity() {
        let invalid_items = [
            json!({"type":"reasoning","encrypted_content":"encrypted"}),
            json!({"type":"reasoning","id":"","encrypted_content":"encrypted"}),
            json!({"type":"reasoning","id":"rs_1"}),
            json!({"type":"reasoning","id":"rs_1","encrypted_content":""}),
        ];
        for item in invalid_items {
            let result = response_to_chat(json!({
                "id": "resp_1",
                "status": "completed",
                "output": [item]
            }));
            assert!(is_invalid_response(&result));
        }
    }

    #[test]
    fn function_argument_terminal_events_emit_only_unseen_suffix() -> Result<()> {
        let mut adapter = ResponsesStreamAdapter::default();
        let added = required_chunk(
            adapter.map_event(json!({
                "type":"response.output_item.added",
                "output_index":2,
                "item":{"id":"fc_7","type":"function_call","call_id":"call_7","name":"read","arguments":""}
            })),
            "function_call added event",
        )?;
        let added_delta = first_tool_delta(&added)?;
        assert_eq!(added_delta.index, 2);
        assert_eq!(added_delta.id.as_deref(), Some("call_7"));
        assert_eq!(
            added_delta
                .function
                .as_ref()
                .and_then(|function| function.name.as_deref()),
            Some("read")
        );
        assert_eq!(tool_arguments(&added), None);

        let delta = required_chunk(
            adapter.map_event(json!({
                "type":"response.function_call_arguments.delta",
                "item_id":"fc_7",
                "output_index":2,
                "delta":"{\"q\":"
            })),
            "function arguments delta",
        )?;
        assert_eq!(tool_arguments(&delta), Some("{\"q\":"));

        let done = required_chunk(
            adapter.map_event(json!({
                "type":"response.function_call_arguments.done",
                "item_id":"fc_7",
                "output_index":2,
                "arguments":"{\"q\":\"rust\"}"
            })),
            "function arguments done suffix",
        )?;
        assert_eq!(tool_arguments(&done), Some("\"rust\"}"));

        let item_done = adapter.map_event(json!({
            "type":"response.output_item.done",
            "output_index":2,
            "item":{"id":"fc_7","type":"function_call","call_id":"call_7","name":"read","arguments":"{\"q\":\"rust\"}"}
        }))?;
        assert!(item_done.is_none());
        Ok(())
    }

    #[test]
    fn function_argument_terminal_event_rejects_mismatch() -> Result<()> {
        let mut adapter = ResponsesStreamAdapter::default();
        let _added = adapter.map_event(json!({
            "type":"response.output_item.added",
            "output_index":1,
            "item":{"id":"fc_1","type":"function_call","call_id":"call_1","name":"read","arguments":""}
        }))?;
        let _delta = adapter.map_event(json!({
            "type":"response.function_call_arguments.delta",
            "item_id":"fc_1",
            "output_index":1,
            "delta":"not-json"
        }))?;
        let result = adapter.map_event(json!({
            "type":"response.function_call_arguments.done",
            "item_id":"fc_1",
            "output_index":1,
            "arguments":"{}"
        }));
        assert!(is_invalid_response(&result));
        Ok(())
    }

    #[test]
    fn refusal_and_reasoning_terminal_events_map_without_duplication() -> Result<()> {
        let mut adapter = ResponsesStreamAdapter::default();
        let refusal_delta = required_chunk(
            adapter.map_event(json!({
                "type":"response.refusal.delta",
                "item_id":"msg_1",
                "output_index":0,
                "content_index":0,
                "delta":"can"
            })),
            "refusal delta",
        )?;
        assert_eq!(refusal_delta.delta.content.as_deref(), Some("can"));
        let refusal_done = required_chunk(
            adapter.map_event(json!({
                "type":"response.refusal.done",
                "item_id":"msg_1",
                "output_index":0,
                "content_index":0,
                "refusal":"cannot"
            })),
            "refusal done suffix",
        )?;
        assert_eq!(refusal_done.delta.content.as_deref(), Some("not"));
        let repeated_refusal_done = adapter.map_event(json!({
            "type":"response.refusal.done",
            "item_id":"msg_1",
            "output_index":0,
            "content_index":0,
            "refusal":"cannot"
        }))?;
        assert!(repeated_refusal_done.is_none());

        let reasoning_delta = required_chunk(
            adapter.map_event(json!({
                "type":"response.reasoning_text.delta",
                "item_id":"rs_1",
                "output_index":1,
                "content_index":0,
                "delta":"think"
            })),
            "reasoning text delta",
        )?;
        assert_eq!(
            reasoning_delta.delta.reasoning_content.as_deref(),
            Some("think")
        );
        let reasoning_done = required_chunk(
            adapter.map_event(json!({
                "type":"response.reasoning_text.done",
                "item_id":"rs_1",
                "output_index":1,
                "content_index":0,
                "text":"thinking"
            })),
            "reasoning text done suffix",
        )?;
        assert_eq!(
            reasoning_done.delta.reasoning_content.as_deref(),
            Some("ing")
        );
        Ok(())
    }

    #[test]
    fn output_item_terminals_map_tool_and_opaque_reasoning() -> Result<()> {
        let mut adapter = ResponsesStreamAdapter::default();
        let tool = required_chunk(
            adapter.map_event(json!({
                "type":"response.output_item.done",
                "output_index":3,
                "item":{"id":"fc_3","type":"function_call","call_id":"call_3","name":"lookup","arguments":"{}"}
            })),
            "terminal function_call item",
        )?;
        let tool_delta = first_tool_delta(&tool)?;
        assert_eq!(tool_delta.id.as_deref(), Some("call_3"));
        assert_eq!(tool_arguments(&tool), Some("{}"));

        let reasoning = required_chunk(
            adapter.map_event(json!({
                "type":"response.output_item.done",
                "output_index":4,
                "item":{"id":"rs_4","type":"reasoning","encrypted_content":"encrypted","summary":[{"type":"summary_text","text":"summary"}]}
            })),
            "terminal reasoning item",
        )?;
        assert!(reasoning.delta.reasoning_blocks.as_ref().is_some_and(|blocks| {
            matches!(
                blocks.first(),
                Some(ReasoningBlock::Opaque { id, data, summary, .. })
                    if id == "rs_4" && data == "encrypted" && summary.first().is_some_and(|text| text == "summary")
            )
        }));

        let _partial = adapter.map_event(json!({
            "type":"response.output_text.delta",
            "item_id":"msg_5",
            "output_index":5,
            "content_index":0,
            "delta":"hel"
        }))?;
        let message_suffix = required_chunk(
            adapter.map_event(json!({
                "type":"response.output_item.done",
                "output_index":5,
                "item":{"id":"msg_5","type":"message","content":[{"type":"output_text","text":"hello"}]}
            })),
            "terminal message item suffix",
        )?;
        assert_eq!(message_suffix.delta.content.as_deref(), Some("lo"));
        let repeated_message = adapter.map_event(json!({
            "type":"response.output_item.done",
            "output_index":5,
            "item":{"id":"msg_5","type":"message","content":[{"type":"output_text","text":"hello"}]}
        }))?;
        assert!(repeated_message.is_none());

        let message_without_delta = required_chunk(
            adapter.map_event(json!({
                "type":"response.output_item.done",
                "output_index":6,
                "item":{"id":"msg_6","type":"message","content":[{"type":"refusal","refusal":"cannot comply"}]}
            })),
            "terminal message item without deltas",
        )?;
        assert_eq!(
            message_without_delta.delta.content.as_deref(),
            Some("cannot comply")
        );
        Ok(())
    }

    #[test]
    fn stream_events_reject_missing_required_identity() {
        let invalid_events = [
            json!({
                "type":"response.output_text.delta",
                "item_id":"msg_1",
                "content_index":0,
                "delta":"text"
            }),
            json!({
                "type":"response.output_item.added",
                "output_index":0,
                "item":{"type":"function_call","call_id":"call_1","name":"read","arguments":""}
            }),
            json!({
                "type":"response.output_item.added",
                "output_index":0,
                "item":{"id":"fc_1","type":"function_call","call_id":"","name":"read","arguments":""}
            }),
            json!({
                "type":"response.output_item.added",
                "output_index":0,
                "item":{"id":"fc_1","type":"function_call","call_id":"call_1","arguments":""}
            }),
            json!({
                "type":"response.output_item.added",
                "output_index":0,
                "item":{"id":"fc_1","type":"function_call","call_id":"call_1","name":"read"}
            }),
            json!({
                "type":"response.output_item.done",
                "output_index":0,
                "item":{"id":"rs_1","type":"reasoning"}
            }),
        ];
        for event in invalid_events {
            let mut adapter = ResponsesStreamAdapter::default();
            let result = adapter.map_event(event);
            assert!(is_invalid_response(&result));
        }
    }

    #[test]
    fn terminal_events_require_completed_response_even_without_error() -> Result<()> {
        let mut adapter = ResponsesStreamAdapter::default();
        for status in ["failed", "cancelled", "incomplete", "in_progress"] {
            let result = adapter.map_event(json!({
                "type":"response.completed",
                "response":{"id":"resp_1","status":status,"output":[]}
            }));
            assert!(is_invalid_response(&result), "status {status} was accepted");
        }
        for event_type in [
            "response.failed",
            "response.cancelled",
            "response.incomplete",
        ] {
            let result = adapter.map_event(json!({
                "type":event_type,
                "response":{"id":"resp_1","status":event_type,"output":[]}
            }));
            assert!(is_invalid_response(&result));
        }

        let terminal = required_chunk(
            adapter.map_event(json!({
                "type":"response.completed",
                "response":{"id":"resp_1","status":"completed","output":[],"usage":{"input_tokens":3,"output_tokens":2,"total_tokens":5,"input_tokens_details":{"cached_tokens":0,"cache_write_tokens":0},"output_tokens_details":{"reasoning_tokens":0}}}
            })),
            "completed terminal event",
        )?;
        assert_eq!(terminal.finish_reason.as_deref(), Some("stop"));
        assert_eq!(terminal.usage.and_then(|usage| usage.total_tokens), Some(5));
        Ok(())
    }

    #[tokio::test]
    async fn delta_then_done_without_completed_fails() -> Result<()> {
        let raw_stream = futures::stream::iter(vec![
            Ok(JsonSseEvent::Data(json!({
                "type":"response.output_text.delta",
                "item_id":"msg_1",
                "output_index":0,
                "content_index":0,
                "delta":"partial"
            }))),
            Ok(JsonSseEvent::Done),
        ]);
        let mut stream = adapt_responses_stream(raw_stream);
        let first = stream.next().await.ok_or_else(|| {
            invalid_response("adapted stream omitted the text delta before [DONE]")
        })??;
        assert_eq!(first.delta.content.as_deref(), Some("partial"));
        let terminal = stream.next().await.ok_or_else(|| {
            invalid_response("adapted stream omitted missing-completed error at [DONE]")
        })?;
        assert!(is_invalid_response(&terminal));
        assert!(stream.next().await.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn delta_then_eof_without_completed_fails() -> Result<()> {
        let raw_stream = futures::stream::iter(vec![Ok(JsonSseEvent::Data(json!({
            "type":"response.output_text.delta",
            "item_id":"msg_1",
            "output_index":0,
            "content_index":0,
            "delta":"partial"
        })))]);
        let mut stream = adapt_responses_stream(raw_stream);
        let first = stream.next().await.ok_or_else(|| {
            invalid_response("adapted stream omitted the text delta before EOF")
        })??;
        assert_eq!(first.delta.content.as_deref(), Some("partial"));
        let terminal = stream.next().await.ok_or_else(|| {
            invalid_response("adapted stream omitted missing-completed error at EOF")
        })?;
        assert!(is_invalid_response(&terminal));
        assert!(stream.next().await.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn completed_then_done_emits_single_finish() -> Result<()> {
        let raw_stream = futures::stream::iter(vec![
            Ok(JsonSseEvent::Data(json!({
                "type":"response.completed",
                "response":{"id":"resp_1","status":"completed","output":[]}
            }))),
            Ok(JsonSseEvent::Done),
        ]);
        let items = adapt_responses_stream(raw_stream).collect::<Vec<_>>().await;
        assert_eq!(items.len(), 1);
        let Some(Ok(terminal)) = items.first() else {
            return Err(invalid_response(
                "completed stream did not emit one successful terminal chunk",
            ));
        };
        assert_eq!(terminal.finish_reason.as_deref(), Some("stop"));
        Ok(())
    }

    #[test]
    fn completed_rejects_following_semantic_or_duplicate_terminal_event() -> Result<()> {
        let completed = json!({
            "type":"response.completed",
            "response":{"id":"resp_1","status":"completed","output":[]}
        });
        let mut semantic_adapter = ResponsesStreamAdapter::default();
        let _terminal = semantic_adapter.map_event(completed.clone())?;
        let semantic = semantic_adapter.map_event(json!({
            "type":"response.output_text.delta",
            "item_id":"msg_1",
            "output_index":0,
            "content_index":0,
            "delta":"late"
        }));
        assert!(is_invalid_response(&semantic));

        let mut duplicate_adapter = ResponsesStreamAdapter::default();
        let _terminal = duplicate_adapter.map_event(completed.clone())?;
        let duplicate = duplicate_adapter.map_event(completed);
        assert!(is_invalid_response(&duplicate));
        Ok(())
    }
}
