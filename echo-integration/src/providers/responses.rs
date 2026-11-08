//! OpenAI Responses API adapter.
//!
//! The adapter keeps the framework's provider-neutral `LlmClient` contract and
//! translates full local conversation history into Responses input items. embedding application
//! therefore remains file/local-history authoritative and does not depend on
//! `previous_response_id`, hosted conversations, or server-side storage.

use echo_core::agent::{AssistantMessageBlock, AssistantPhase};
use echo_core::error::{LlmError, Result};
use echo_core::llm::capabilities::{
    ModelFactInputs, ModelProfileResolution, ModelProfileResolver, ProviderCapabilityOverride,
};
use echo_core::llm::types::{
    ChatCompletionResponse, Choice, ContentPart, DeltaFunctionCall, DeltaMessage, DeltaToolCall,
    FunctionCall, Message, MessageContent, ProviderResponseMetadata, ReasoningBlock,
    ResponseFormat, Role, TokenUsageDetails, ToolCall, ToolDefinition, Usage,
};
use echo_core::llm::{
    ChatChunk, ChatRequest, ChatResponse, LlmClient, ThinkingConfig, ThinkingLevel,
    ThinkingProtocol, ToolChoice,
};
use futures::StreamExt;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use reqwest::Client;
use reqwest::header::HeaderMap;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, info_span};

use super::client::{JsonSseEvent, ensure_request_not_cancelled, post_json, stream_json_sse};
use super::config::LlmConfig;
use super::native_message::output_source;
use super::openai::assemble_req_header;
use super::thinking_translate::translate_thinking_openai_compat;
use echo_core::llm::LlmApiProtocol;

mod diagnostics;
use diagnostics::ResponsesDiagnostics;

/// Client for the OpenAI Responses API.
pub struct ResponsesClient {
    client: Arc<Client>,
    config: LlmConfig,
    model_profile_resolver: ModelProfileResolver,
    header_map: HeaderMap,
}

/// Responses-compatible endpoints do not share one response dialect. The
/// OpenAI endpoint returns encrypted reasoning items, while DeepSeek returns
/// plaintext reasoning and has a narrower input surface. Keep this decision
/// tied to the configured vendor/model rather than guessing from payload text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum ResponsesDialect {
    #[default]
    OpenAi,
    DeepSeekFlash,
    DeepSeekV4Pro,
}

impl ResponsesClient {
    /// Create a Responses client from an injected provider configuration.
    pub fn new(config: LlmConfig) -> Result<Self> {
        let header_map = assemble_req_header(&config)?;
        let model_profile_resolver = config
            .model_profile_resolver_at(std::time::SystemTime::now(), &ModelFactInputs::default());
        Ok(Self {
            client: Arc::new(Self::build_http_client()),
            config,
            model_profile_resolver,
            header_map,
        })
    }

    pub(crate) fn with_model_profile_resolver(mut self, resolver: ModelProfileResolver) -> Self {
        self.model_profile_resolver = resolver;
        self
    }

    /// Create a Responses client with a shared HTTP client.
    pub fn with_client(client: Arc<Client>, config: LlmConfig) -> Result<Self> {
        let header_map = assemble_req_header(&config)?;
        let model_profile_resolver = config
            .model_profile_resolver_at(std::time::SystemTime::now(), &ModelFactInputs::default());
        Ok(Self {
            client,
            config,
            model_profile_resolver,
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
            None,
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

    fn responses_dialect(&self) -> Result<ResponsesDialect> {
        let provider = self
            .config
            .provider_name
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let endpoint = reqwest::Url::parse(&self.config.base_url)
            .map_err(|_| request_error("invalid Responses endpoint URL"))?;
        let model = self.config.model.to_ascii_lowercase();
        let deepseek_endpoint =
            provider == "deepseek" || endpoint.host_str() == Some("api.deepseek.com");
        if !deepseek_endpoint {
            return Ok(ResponsesDialect::OpenAi);
        }
        match model.as_str() {
            "deepseek-flash" => Ok(ResponsesDialect::DeepSeekFlash),
            "deepseek-v4-pro" => Ok(ResponsesDialect::DeepSeekV4Pro),
            _ => Err(request_error(format!(
                "DeepSeek Responses model '{}' has no admitted dialect",
                self.config.model
            ))),
        }
    }

    fn request_body(&self, request: &ChatRequest, stream: bool) -> Result<Value> {
        let dialect = self.responses_dialect()?;
        self.config.validate_input_modalities(&request.messages)?;
        validate_response_tools(request.tools.as_deref(), dialect)?;
        if let Some(choice) = request.tool_choice.as_ref() {
            validate_response_tool_choice(choice, request.tools.as_deref())?;
        }
        let protocol = self.model_profile_resolution().profile.thinking_protocol;
        validate_responses_thinking(dialect, protocol, &request.thinking)?;
        let thinking = translate_thinking_openai_compat(
            &self.config.model,
            self.config.api_protocol,
            protocol,
            &request.thinking,
        );
        // 显式采样参数不能在thinking转换中静默消失；默认None继续沿模型默认。
        let deepseek = dialect != ResponsesDialect::OpenAi;
        let reasoning_enabled = if deepseek {
            !matches!(
                request.thinking,
                Some(ThinkingConfig::Disabled | ThinkingConfig::Level(ThinkingLevel::None))
            )
        } else {
            thinking.drop_temperature
        };
        if reasoning_enabled && request.temperature.is_some() {
            return Err(request_error(
                "temperature cannot be honored with this reasoning configuration",
            ));
        }
        if request
            .temperature
            .is_some_and(|value| !value.is_finite() || !(0.0..=2.0).contains(&value))
        {
            return Err(request_error("temperature must be finite and in [0,2]"));
        }
        if request.max_tokens == Some(0) {
            return Err(request_error("max_output_tokens must be greater than zero"));
        }
        let mut body = json!({
            "model": self.config.model,
            "input": messages_to_input_for_dialect(&request.messages, dialect)?,
            "store": false,
            "stream": stream,
        });
        let Some(object) = body.as_object_mut() else {
            return Ok(body);
        };

        if dialect == ResponsesDialect::OpenAi {
            object.insert(
                "include".to_string(),
                json!(["reasoning.encrypted_content"]),
            );
        }

        insert_option(object, "max_output_tokens", request.max_tokens);
        insert_option(object, "temperature", request.temperature);
        if let Some(tools) = request.tools.as_ref() {
            object.insert("tools".to_string(), tools_to_responses(tools));
        }
        if let Some(choice) = request.tool_choice.as_ref() {
            object.insert("tool_choice".to_string(), tool_choice_to_responses(choice));
        }
        if let Some(format) = request
            .response_format
            .as_ref()
            .filter(|format| !matches!(format, ResponseFormat::Text))
        {
            object.insert("text".to_string(), response_format_to_responses(format));
        }
        // DeepSeek只声明effort；不继承Chat别名clamp，不请求其忽略的summary。
        let effort = if deepseek {
            match request.thinking.as_ref() {
                Some(ThinkingConfig::Disabled | ThinkingConfig::Level(ThinkingLevel::None)) => {
                    Some("none".to_string())
                }
                Some(config) => config.to_reasoning_effort().map(str::to_string),
                None => None,
            }
        } else {
            thinking.reasoning_effort
        };
        if let Some(effort) = effort {
            object.insert(
                "reasoning".to_string(),
                if deepseek {
                    json!({"effort":effort})
                } else {
                    json!({"effort":effort,"summary":"auto"})
                },
            );
        }
        if let Some(user_id) = request.user_id.as_ref() {
            // DeepSeek的user承担已配置的用户隔离语义；不发送被忽略的prompt_cache_key。
            object.insert(
                if deepseek { "user" } else { "prompt_cache_key" }.to_string(),
                json!(user_id),
            );
        }
        echo_core::llm::cache::wire::log_wire_prompt(&body, "responses");
        Ok(body)
    }
}

impl LlmClient for ResponsesClient {
    fn chat(&self, request: ChatRequest) -> BoxFuture<'_, Result<ChatResponse>> {
        let model = self.config.model.clone();
        Box::pin(
            async move {
                self.config.validate_input_modalities(&request.messages)?;
                let timeouts = request.timeouts.unwrap_or(self.config.timeouts);
                let body = self.request_body(&request, false)?;
                let raw = post_json(
                    self.client.clone(),
                    body,
                    self.header_map.clone(),
                    &self.config.base_url,
                    timeouts,
                    request.cancel_token.clone(),
                )
                .await?;
                let response = response_to_chat_for_provider(
                    raw,
                    self.config
                        .provider_name
                        .as_deref()
                        .unwrap_or("openai_responses"),
                )?;
                ensure_request_not_cancelled(request.cancel_token.as_ref())?;
                Ok(response)
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
                let body = self.request_body(&request, true)?;
                let mut diagnostics = ResponsesDiagnostics::start(
                    &self.config,
                    &body,
                    &format!("{:?}", self.responses_dialect()?),
                );
                let request = self
                    .client
                    .post(&self.config.base_url)
                    .headers(self.header_map.clone())
                    .json(&body);
                let raw_stream = match stream_json_sse(
                    request,
                    self.config.model.clone(),
                    timeouts,
                    cancel_token,
                )
                .await
                {
                    Ok(stream) => stream,
                    Err(error) => {
                        diagnostics.finish("transport_start_error", true);
                        return Err(error);
                    }
                };
                Ok(adapt_responses_stream(
                    raw_stream,
                    self.config
                        .provider_name
                        .clone()
                        .unwrap_or_else(|| "openai_responses".into()),
                    Some(diagnostics),
                ))
            }
            .instrument(info_span!("openai_responses_stream", model = %model)),
        )
    }

    fn model_name(&self) -> &str {
        &self.config.model
    }

    fn provider_name(&self) -> Option<&str> {
        self.config.provider_name.as_deref()
    }

    fn reported_usage(
        &self,
        observation: &ProviderResponseMetadata,
        _previous: Option<&Usage>,
    ) -> Result<Option<Usage>> {
        if observation.protocol != echo_core::llm::LlmApiProtocol::Responses
            || observation.provider
                != self
                    .config
                    .provider_name
                    .as_deref()
                    .unwrap_or("openai_responses")
        {
            return Ok(None);
        }
        let Some(raw) = observation.usage.as_ref().filter(|raw| {
            raw.is_object() && !raw.as_object().is_none_or(|fields| fields.is_empty())
        }) else {
            return Ok(None);
        };
        let usage = serde_json::from_value::<ResponsesUsage>(raw.clone())
            .map_err(|_| invalid_response("reported Responses usage is not representable"))?;
        Ok(Some(Usage::from(usage)))
    }

    fn protocol_capabilities(&self) -> ProviderCapabilityOverride {
        ProviderCapabilityOverride::openai_responses_protocol()
    }

    fn model_profile_resolution(&self) -> ModelProfileResolution {
        self.model_profile_resolver.resolve_for_protocol_at(
            self.config.provider_name.as_deref().unwrap_or(""),
            &self.config.model,
            self.config.api_protocol,
            Some(&self.config.base_url),
            std::time::SystemTime::now(),
        )
    }

    fn capabilities(&self) -> echo_core::llm::capabilities::ProviderCapabilities {
        self.model_profile_resolution().profile.capabilities
    }
}

fn adapt_responses_stream<S>(
    raw_stream: S,
    provider: String,
    mut diagnostics: Option<ResponsesDiagnostics>,
) -> BoxStream<'static, Result<ChatChunk>>
where
    S: futures::Stream<Item = Result<JsonSseEvent>> + Send + 'static,
{
    let stream = async_stream::try_stream! {
        let mut adapter = ResponsesStreamAdapter { provider, ..Default::default() };
        futures::pin_mut!(raw_stream);
        while let Some(event) = raw_stream.next().await {
            let event = match event {
                Ok(event) => event,
                Err(error) => {
                    if let Some(diagnostics) = &mut diagnostics {
                        diagnostics.finish("transport_error", true);
                    }
                    Err(error)?;
                    return;
                }
            };
            match event {
                JsonSseEvent::Done => {
                    let result = adapter.validate_transport_end("SSE [DONE]");
                    if let Some(diagnostics) = &mut diagnostics {
                        diagnostics.finish("transport_done", result.is_err());
                    }
                    result?;
                    return;
                }
                JsonSseEvent::Data(value) => {
                    if let Some(diagnostics) = &mut diagnostics {
                        diagnostics.event(&value, &adapter.raw_phases);
                    }
                    let mapped = match adapter.map_event(value) {
                        Ok(chunk) => chunk,
                        Err(error) => {
                            if let Some(diagnostics) = &mut diagnostics {
                                diagnostics.finish("adapter_error", true);
                            }
                            Err(error)?;
                            return;
                        }
                    };
                    if let Some(chunk) = mapped {
                        yield chunk;
                    }
                }
            }
        }
        let result = adapter.validate_transport_end("SSE EOF");
        if let Some(diagnostics) = &mut diagnostics {
            diagnostics.finish("transport_eof", result.is_err());
        }
        result?;
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

#[cfg(test)]
fn messages_to_input(messages: &[Message]) -> Result<Value> {
    messages_to_input_for_dialect(messages, ResponsesDialect::OpenAi)
}

fn messages_to_input_for_dialect(messages: &[Message], dialect: ResponsesDialect) -> Result<Value> {
    let mut input = Vec::new();
    for message in messages {
        // Frame失败结算只耐久保存这组精确形状；不把它变成空assistant wire。
        if message.is_provider_response_observation_only() {
            continue;
        }
        if !matches!(message.role, Role::Tool) {
            validate_message_modalities(message, dialect)?;
        }
        if matches!(message.role, Role::Assistant) {
            if let Some(native) = native_history_to_responses(message)? {
                if dialect != ResponsesDialect::OpenAi
                    && native.iter().any(|item| {
                        item.get("type").and_then(Value::as_str) == Some("reasoning")
                            && (item.get("encrypted_content").is_some()
                                || item
                                    .get("summary")
                                    .and_then(Value::as_array)
                                    .is_some_and(|summary| !summary.is_empty()))
                    })
                {
                    return Err(request_error(
                        "DeepSeek Responses cannot replay encrypted reasoning or summaries from another dialect",
                    ));
                }
                input.extend(native);
                continue;
            }
            if dialect != ResponsesDialect::OpenAi
                && message.reasoning_blocks.as_deref().unwrap_or_default().iter().any(|block| matches!(block, ReasoningBlock::Opaque { provider, .. } if provider == "openai_responses"))
            {
                return Err(request_error("DeepSeek Responses cannot replay OpenAI encrypted reasoning"));
            }
            append_opaque_reasoning(&mut input, message.reasoning_blocks.as_deref());
        }

        if matches!(message.role, Role::Tool) {
            let call_id = message
                .tool_call_id
                .as_deref()
                .filter(|id| !id.is_empty())
                .ok_or_else(|| {
                    request_error("Responses tool result requires a nonempty call_id")
                })?;
            input.push(json!({
                "type":"function_call_output", "call_id":call_id,
                "output":tool_result_to_responses(&message.content, dialect)?,
            }));
            continue;
        }
        if matches!(&message.role, Role::Custom(role) if role != "developer") {
            return Err(request_error("Responses message role is unsupported"));
        }

        if !matches!(message.content, MessageContent::Empty) {
            input.push(json!({
                "type": "message",
                "role": responses_role(&message.role),
                "content": content_to_responses(&message.content, dialect)?,
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
    Ok(Value::Array(input))
}

/// Rebuild native Responses items from the existing canonical message/tool
/// provenance. Runtime approvals have no provider source and are not wire data.
fn native_history_to_responses(message: &Message) -> Result<Option<Vec<Value>>> {
    let blocks = message.assistant_blocks.as_deref().unwrap_or_default();
    let tools = message.tool_calls.as_deref().unwrap_or_default();
    let native = blocks.iter().any(|block| {
        block
            .provider_source
            .as_ref()
            .is_some_and(|source| source.protocol == LlmApiProtocol::Responses)
    }) || tools.iter().any(|tool| {
        tool.provider_source
            .as_ref()
            .is_some_and(|source| source.protocol == LlmApiProtocol::Responses)
    }) || message
        .provider_response
        .as_ref()
        .is_some_and(|record| record.protocol == LlmApiProtocol::Responses)
        && (blocks.iter().any(|block| block.provider_source.is_some())
            || tools.iter().any(|tool| tool.provider_source.is_some()));
    if !native {
        if blocks
            .iter()
            .any(|block| block.provider.as_deref() == Some("openai_responses"))
        {
            return Err(invalid_response(
                "Responses replay lacks actual native response identity",
            ));
        }
        return Ok(None);
    }
    let observation = message.provider_response.as_ref();
    if let Some(record) = observation
        && (record.protocol != LlmApiProtocol::Responses
            || record.provider.trim().is_empty()
            || record
                .request_id
                .as_deref()
                .is_none_or(|id| id.trim().is_empty()))
    {
        return Err(invalid_response(
            "Responses replay observation lacks matching actual protocol/provider/request",
        ));
    }
    let mut groups: Vec<Vec<&AssistantMessageBlock>> = Vec::new();
    for block in blocks {
        if let Some(group) = groups.iter_mut().find(|group| {
            group.first().is_some_and(|previous| {
                previous.request_id == block.request_id && previous.block_id == block.block_id
            })
        }) {
            group.push(block);
        } else {
            groups.push(vec![block]);
        }
    }
    let mut merged: Vec<AssistantMessageBlock> = Vec::new();
    for group in groups {
        // 未被native来源升级的runtime/approved诊断组从不进入vendor wire。
        if group.iter().all(|block| block.provider_source.is_none()) {
            if group
                .iter()
                .any(|block| block.provider.as_deref() == Some("openai_responses"))
            {
                return Err(invalid_response(
                    "Responses native block never received actual source",
                ));
            }
            continue;
        }
        let mut previous = (*group
            .first()
            .ok_or_else(|| invalid_response("Responses replay group missing"))?)
        .clone();
        for block in group.into_iter().skip(1) {
            match (&previous.provider, &block.provider) {
                (Some(before), Some(after)) if before != after => {
                    return Err(invalid_response(
                        "Responses replay canonical provider changed",
                    ));
                }
                (Some(_), None) => {
                    return Err(invalid_response(
                        "Responses replay canonical provider cleared",
                    ));
                }
                (None, Some(_)) => previous.provider = block.provider.clone(),
                _ => {}
            }
            match (&previous.provider_source, &block.provider_source) {
                (None, Some(_)) => previous.provider_source = block.provider_source.clone(),
                (Some(_), None) => {
                    return Err(invalid_response(
                        "Responses known native source cannot be cleared during replay",
                    ));
                }
                (Some(before), Some(after)) if before != after => {
                    let mut before = before.clone();
                    let mut after = after.clone();
                    let before_phase = before.phase.take();
                    let after_phase = after.phase.take();
                    if before != after
                        || before_phase
                            .as_ref()
                            .is_some_and(|phase| after_phase.as_ref() != Some(phase))
                    {
                        return Err(invalid_response(
                            "Responses canonical source changed during replay",
                        ));
                    }
                    if before_phase.is_none() && after_phase.is_some() {
                        previous.provider_source = block.provider_source.clone();
                    }
                }
                _ => {}
            }
            if block.phase != AssistantPhase::Unknown {
                previous.phase = block.phase;
            }
            previous.delta.push_str(&block.delta);
            previous.complete |= block.complete;
            if block.reasoning.is_some() {
                previous.reasoning = block.reasoning.clone();
            }
        }
        let source = previous
            .provider_source
            .as_ref()
            .ok_or_else(|| invalid_response("Responses replay native source missing"))?;
        if source.protocol != LlmApiProtocol::Responses {
            return Err(invalid_response(
                "Responses replay contains foreign protocol source",
            ));
        }
        if let Some(provider) = previous.provider.as_deref() {
            // block.provider是adapter标签或历史实际vendor；不强绑当前client配置的别名。
            if provider != "openai_responses"
                && observation.is_none_or(|record| record.provider != provider)
            {
                return Err(invalid_response(
                    "Responses replay contains foreign provider observation",
                ));
            }
        }
        if observation
            .is_some_and(|record| record.request_id.as_deref() != Some(source.request_id.as_str()))
        {
            return Err(invalid_response(
                "Responses replay source disagrees with actual observed request",
            ));
        }
        merged.push(previous);
    }
    let mut items: std::collections::BTreeMap<u32, Value> = std::collections::BTreeMap::new();
    let mut parts: std::collections::BTreeMap<(u32, u32), Value> =
        std::collections::BTreeMap::new();
    let opaque_summaries: HashMap<_, _> = merged
        .iter()
        .filter_map(|block| {
            let source = block.provider_source.as_ref()?;
            let Some(ReasoningBlock::Opaque { summary, .. }) = &block.reasoning else {
                return None;
            };
            Some((
                (
                    source.request_id.clone(),
                    source.item_id.clone(),
                    source.output_index,
                ),
                summary.clone(),
            ))
        })
        .collect();
    let mut reasoning_parts: std::collections::BTreeMap<(u32, String, u32), Value> =
        std::collections::BTreeMap::new();
    let mut owner = None::<String>;
    for block in merged {
        let source = block
            .provider_source
            .as_ref()
            .ok_or_else(|| invalid_response("native block source missing"))?;
        if owner
            .as_ref()
            .is_some_and(|owner| owner != &source.request_id)
        {
            return Err(invalid_response("Responses replay mixes request owners"));
        }
        owner.get_or_insert_with(|| source.request_id.clone());
        if source.item_id.is_empty() || source.request_id.is_empty() {
            return Err(invalid_response("Responses replay source identity missing"));
        }
        if let Some(ReasoningBlock::Opaque {
            provider,
            id,
            data,
            summary,
        }) = block.reasoning
        {
            if provider != "openai_responses" || id != source.item_id {
                return Err(invalid_response(
                    "Responses replay reasoning source mismatched",
                ));
            }
            // 同一item的明文可能先于encrypted原件到达；合并原字段，不覆盖其content。
            let item = items
                .entry(source.output_index)
                .or_insert_with(|| json!({"type":"reasoning","id":id}));
            if item.get("type").and_then(Value::as_str) != Some("reasoning")
                || item.get("id").and_then(Value::as_str) != Some(id.as_str())
            {
                return Err(invalid_response(
                    "Responses replay reasoning item metadata conflicts",
                ));
            }
            let object = item.as_object_mut().ok_or_else(|| {
                invalid_response("Responses replay reasoning item was not an object")
            })?;
            if object.contains_key("encrypted_content") {
                return Err(invalid_response(
                    "Responses replay has duplicate output index",
                ));
            }
            object.insert("encrypted_content".into(), json!(data));
            object.insert(
                "summary".into(),
                json!(
                    summary
                        .iter()
                        .map(|text| json!({"type":"summary_text","text":text}))
                        .collect::<Vec<_>>()
                ),
            );
            continue;
        }
        if block.phase == AssistantPhase::Reasoning {
            let index = source.content_index.ok_or_else(|| {
                invalid_response("Responses plaintext reasoning content index missing")
            })?;
            let kind = source
                .content_kind
                .as_deref()
                .ok_or_else(|| invalid_response("Responses plaintext reasoning kind missing"))?;
            let field = match kind {
                "reasoning_text" => "content",
                "summary_text" => "summary",
                _ => {
                    return Err(invalid_response(
                        "unsupported Responses plaintext reasoning kind",
                    ));
                }
            };
            let item = items
                .entry(source.output_index)
                .or_insert_with(|| json!({"type":"reasoning","id":source.item_id}));
            if item.get("type").and_then(Value::as_str) != Some("reasoning")
                || item.get("id").and_then(Value::as_str) != Some(source.item_id.as_str())
            {
                return Err(invalid_response(
                    "Responses plaintext reasoning item metadata conflicts",
                ));
            }
            // 仅去掉Opaque已携带的同源同索引summary；同文content也是独立原件。
            if field == "summary"
                && let Some(carried) = opaque_summaries
                    .get(&(
                        source.request_id.clone(),
                        source.item_id.clone(),
                        source.output_index,
                    ))
                    .and_then(|summary| {
                        usize::try_from(index)
                            .ok()
                            .and_then(|index| summary.get(index))
                    })
            {
                if carried != &block.delta {
                    return Err(invalid_response(
                        "Responses replay summary disagreed with opaque summary",
                    ));
                }
                continue;
            }
            if reasoning_parts
                .insert(
                    (source.output_index, field.to_string(), index),
                    json!({"type":kind,"text":block.delta}),
                )
                .is_some()
            {
                return Err(invalid_response(
                    "Responses plaintext reasoning duplicated content index",
                ));
            }
            continue;
        }
        let content_index = source
            .content_index
            .ok_or_else(|| invalid_response("Responses replay message content index missing"))?;
        let kind = source
            .content_kind
            .as_deref()
            .ok_or_else(|| invalid_response("Responses replay content kind missing"))?;
        let part = match kind {
            "output_text" => json!({"type":"output_text","text":block.delta,"annotations":[]}),
            "refusal" => json!({"type":"refusal","refusal":block.delta}),
            _ => {
                return Err(invalid_response(
                    "Responses replay unsupported message kind",
                ));
            }
        };
        if parts
            .insert((source.output_index, content_index), part)
            .is_some()
        {
            return Err(invalid_response(
                "Responses replay duplicated content index",
            ));
        }
        let item = items.entry(source.output_index).or_insert_with(|| {
            let mut value =
                json!({"type":"message","id":source.item_id,"role":"assistant","content":[]});
            if let (Some(phase), Some(object)) = (&source.phase, value.as_object_mut()) {
                object.insert("phase".into(), json!(phase));
            }
            value
        });
        if item.get("type").and_then(Value::as_str) != Some("message")
            || item.get("id").and_then(Value::as_str) != Some(source.item_id.as_str())
            || item.get("phase").and_then(Value::as_str) != source.phase.as_deref()
        {
            return Err(invalid_response("Responses replay item metadata conflicts"));
        }
    }
    for ((output_index, _), part) in parts {
        let content = items
            .get_mut(&output_index)
            .and_then(|item| item.get_mut("content"))
            .and_then(Value::as_array_mut)
            .ok_or_else(|| invalid_response("Responses replay message item missing"))?;
        content.push(part);
    }
    for ((output_index, field, _), part) in reasoning_parts {
        let object = items
            .get_mut(&output_index)
            .and_then(Value::as_object_mut)
            .ok_or_else(|| invalid_response("Responses plaintext reasoning item missing"))?;
        let content = object
            .entry(field)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .ok_or_else(|| {
                invalid_response("Responses plaintext reasoning field was not an array")
            })?;
        content.push(part);
    }
    for tool in tools {
        let source = tool
            .provider_source
            .as_ref()
            .filter(|source| source.protocol == LlmApiProtocol::Responses)
            .ok_or_else(|| invalid_response("Responses replay tool source missing"))?;
        if observation
            .is_some_and(|record| record.request_id.as_deref() != Some(source.request_id.as_str()))
        {
            return Err(invalid_response(
                "Responses replay tool source disagrees with observed request",
            ));
        }
        if owner
            .as_ref()
            .is_some_and(|owner| owner != &source.request_id)
        {
            return Err(invalid_response(
                "Responses replay tool belongs to another response",
            ));
        }
        owner.get_or_insert_with(|| source.request_id.clone());
        if source.request_id.is_empty()
            || source.item_id.is_empty()
            || source.content_kind.as_deref() != Some("function_call")
        {
            return Err(invalid_response("Responses replay invalid tool source"));
        }
        let item = json!({"type":"function_call","id":source.item_id,"call_id":tool.id,"name":tool.function.name,"arguments":tool.function.arguments});
        if items.insert(source.output_index, item).is_some() {
            return Err(invalid_response(
                "Responses replay tool index conflicts with native item",
            ));
        }
    }
    Ok(Some(items.into_values().collect()))
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

fn request_error(message: impl Into<String>) -> echo_core::error::ReactError {
    LlmError::SerializationError(format!("Responses request: {}", message.into())).into()
}

fn validate_message_modalities(message: &Message, dialect: ResponsesDialect) -> Result<()> {
    let (ResponsesDialect::DeepSeekFlash | ResponsesDialect::DeepSeekV4Pro) = dialect else {
        return Ok(());
    };
    let Some(parts) = message.content.parts() else {
        return Ok(());
    };
    let developer_or_user = matches!(message.role, Role::User)
        || matches!(&message.role, Role::Custom(role) if role == "developer");
    for part in parts {
        match part {
            ContentPart::File { .. } => {
                return Err(request_error(
                    "DeepSeek Responses does not support input_file content",
                ));
            }
            ContentPart::ImageUrl { .. }
                if dialect == ResponsesDialect::DeepSeekV4Pro || !developer_or_user =>
            {
                return Err(request_error(
                    "DeepSeek input_image requires deepseek-flash user/developer content",
                ));
            }
            ContentPart::Text { .. }
            | ContentPart::ResourceLink { .. }
            | ContentPart::ImageUrl { .. } => {}
        }
    }
    Ok(())
}

fn content_to_responses(content: &MessageContent, dialect: ResponsesDialect) -> Result<Value> {
    match content {
        MessageContent::Text(text) => Ok(json!([{ "type":"input_text", "text":text }])),
        MessageContent::Empty => Ok(json!([])),
        MessageContent::Parts(parts) => {
            let mut values = Vec::with_capacity(parts.len());
            for part in parts {
                let value = match part {
                    ContentPart::Text { text } => json!({
                        "type": "input_text",
                        "text": text,
                    }),
                    ContentPart::ImageUrl { image_url } => {
                        let detail = image_url.detail.as_deref().unwrap_or("auto");
                        if image_url.url.is_empty()
                            || !matches!(detail, "auto" | "low" | "high" | "original")
                        {
                            return Err(request_error(
                                "input_image requires image_url and a supported detail value",
                            ));
                        }
                        json!({"type":"input_image", "detail":detail, "image_url":image_url.url})
                    }
                    ContentPart::File { .. }
                        if matches!(
                            dialect,
                            ResponsesDialect::DeepSeekFlash | ResponsesDialect::DeepSeekV4Pro
                        ) =>
                    {
                        return Err(request_error(
                            "DeepSeek Responses does not support input_file content",
                        ));
                    }
                    ContentPart::File { name, content } => {
                        if name.is_empty() || content.is_empty() {
                            return Err(request_error(
                                "input_file requires filename and base64 file data",
                            ));
                        }
                        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, content)
                            .map_err(|_| request_error("input_file content is not valid base64"))?;
                        let mime = if name.to_ascii_lowercase().ends_with(".pdf") {
                            "application/pdf"
                        } else {
                            "application/octet-stream"
                        };
                        json!({"type":"input_file", "filename":name, "file_data":format!("data:{mime};base64,{content}")})
                    }
                    ContentPart::ResourceLink { resource } => json!({
                        "type": "input_text",
                        "text": resource.model_text(),
                    }),
                };
                values.push(value);
            }
            Ok(Value::Array(values))
        }
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
                    // Responses uses a flat function tool shape. `None` keeps
                    // the existing permissive default; an explicit value is
                    // preserved so strict tool validation is not conflated
                    // with structured final-output strictness.
                    "strict": tool.function.strict.unwrap_or(false),
                })
            })
            .collect(),
    )
}

fn validate_response_tools(
    tools: Option<&[ToolDefinition]>,
    dialect: ResponsesDialect,
) -> Result<()> {
    let mut names = HashSet::new();
    for tool in tools.unwrap_or_default() {
        if !names.insert(tool.function.name.as_str()) {
            return Err(request_error(
                "Responses function tool names must be unique",
            ));
        }
        if dialect != ResponsesDialect::OpenAi && tool.function.strict == Some(true) {
            return Err(request_error(
                "DeepSeek Responses function strict=true has no admitted support; text.format support is separate",
            ));
        }
        if tool.tool_type != "function" {
            return Err(request_error(format!(
                "Responses does not support tool type '{}'",
                tool.tool_type
            )));
        }
        if tool.function.name.is_empty() {
            return Err(request_error("Responses function tool name is empty"));
        }
        if !tool.function.parameters.is_object() {
            return Err(request_error(format!(
                "Responses function '{}' parameters must be a JSON object",
                tool.function.name
            )));
        }
    }
    Ok(())
}

fn validate_response_tool_choice(
    choice: &ToolChoice,
    tools: Option<&[ToolDefinition]>,
) -> Result<()> {
    match choice {
        ToolChoice::Auto | ToolChoice::None => Ok(()),
        ToolChoice::Required => {
            if tools.unwrap_or_default().is_empty() {
                return Err(request_error(
                    "Responses required tool choice needs at least one function tool",
                ));
            }
            Ok(())
        }
        ToolChoice::Function { name } => {
            if name.is_empty() {
                return Err(request_error(
                    "Responses selected function tool name is empty",
                ));
            }
            if tools
                .unwrap_or_default()
                .iter()
                .any(|tool| tool.function.name == *name)
            {
                Ok(())
            } else {
                Err(request_error(format!(
                    "Responses selected function '{name}' is not exposed"
                )))
            }
        }
    }
}

fn validate_responses_thinking(
    dialect: ResponsesDialect,
    protocol: ThinkingProtocol,
    thinking: &Option<ThinkingConfig>,
) -> Result<()> {
    if thinking.is_none() {
        return Ok(());
    }
    if matches!(thinking, Some(ThinkingConfig::BudgetTokens(_))) {
        return Err(request_error(
            "Responses supports reasoning.effort, not an exact token budget",
        ));
    }
    if dialect == ResponsesDialect::OpenAi && protocol != ThinkingProtocol::OpenaiReasoningEffort {
        return Err(request_error(
            "the resolved model has no Responses reasoning.effort control",
        ));
    }
    if !matches!(
        dialect,
        ResponsesDialect::DeepSeekFlash | ResponsesDialect::DeepSeekV4Pro
    ) {
        return Ok(());
    }
    match thinking {
        Some(ThinkingConfig::Level(ThinkingLevel::Xhigh)) => Err(request_error(
            "DeepSeek Responses does not document xhigh reasoning effort; refusing the Chat alias",
        )),
        _ => Ok(()),
    }
}

fn tool_choice_to_responses(choice: &ToolChoice) -> Value {
    match choice {
        ToolChoice::Auto => Value::String("auto".to_string()),
        ToolChoice::None => Value::String("none".to_string()),
        ToolChoice::Required => Value::String("required".to_string()),
        ToolChoice::Function { name } => {
            json!({"type": "function", "name": name})
        }
    }
}

/// Convert a tool result without routing through `MessageContent::as_text`.
/// Responses accepts text, image, and file output parts natively; flattening
/// those parts would silently discard a tool observation needed for replay.
fn tool_result_to_responses(content: &MessageContent, dialect: ResponsesDialect) -> Result<Value> {
    // 文本工具输出仍用原生字符串；Parts共用同一映射，维持文字/图片/文件原序。
    match content {
        MessageContent::Text(text) => Ok(Value::String(text.clone())),
        MessageContent::Empty => Ok(Value::String(String::new())),
        MessageContent::Parts(parts) => {
            if dialect == ResponsesDialect::DeepSeekV4Pro
                && parts
                    .iter()
                    .any(|part| matches!(part, ContentPart::ImageUrl { .. }))
            {
                return Err(request_error(
                    "DeepSeek v4 Responses does not support image tool output",
                ));
            }
            content_to_responses(content, dialect)
        }
    }
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
    #[serde(default)]
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
        id: String,
        call_id: String,
        name: String,
        arguments: String,
    },
    Reasoning {
        id: String,
        /// 不透明推理负载。部分兼容网关没有加密推理，会显式返回 `null`
        /// 或空串；两者都表示"没有可回放的不透明内容"，不能当成协议错误。
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

/// 只从真实response外层选定字段构造唯一Core记录，不接服务伪造的同名扩展。
fn response_metadata(raw: &Value, provider: &str) -> Result<ProviderResponseMetadata> {
    let object = raw
        .as_object()
        .ok_or_else(|| invalid_response("Responses observation requires a response object"))?;
    let mut value = json!({"protocol":LlmApiProtocol::Responses,"provider":provider});
    let target = value
        .as_object_mut()
        .ok_or_else(|| invalid_response("observation object missing"))?;
    if let Some(id) = object.get("id") {
        match id {
            Value::String(id) if !id.is_empty() => {
                target.insert("request_id".into(), json!(id));
            }
            Value::Null => {}
            _ => {
                return Err(invalid_response(
                    "Responses observation id must be a nonempty string or null",
                ));
            }
        }
    }
    for key in ["model", "status", "incomplete_details", "usage"] {
        if let Some(raw) = object.get(key) {
            target.insert(key.into(), raw.clone());
        }
    }
    serde_json::from_value(value)
        .map_err(|error| invalid_response(format!("invalid Responses observation: {error}")))
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
    if response.status.as_deref() != Some("completed") {
        let detail = response
            .incomplete_details
            .as_ref()
            .and_then(|details| details.reason.as_deref())
            .map(|reason| format!(" ({reason})"))
            .unwrap_or_default();
        return Err(invalid_response(format!(
            "{context} ended with non-completed status '{}'{}",
            response.status.as_deref().unwrap_or("missing/null"),
            detail
        )));
    }
    Ok(())
}

/// Convert assistant-bearing Responses items to the Framework canonical block
/// shape. Tool calls remain in `ChatResponse.message.tool_calls`; this helper
/// only carries assistant text/reasoning identity, order and phase.
fn native_assistant_blocks(raw: &Value, response_id: &str) -> Result<Vec<AssistantMessageBlock>> {
    let output = raw
        .get("output")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid_response("Responses response omitted output array"))?;
    let native_request_id = raw
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty());
    let mut blocks = Vec::new();
    let mut sequence = 0_u64;
    let response_completed = raw.get("status").and_then(Value::as_str) == Some("completed");
    for (output_index, item) in output.iter().enumerate() {
        let item_type = required_non_empty_string(item, "type", "Responses output item")?;
        if response_completed
            && item
                .get("status")
                .and_then(Value::as_str)
                .is_some_and(|status| status != "completed")
        {
            return Err(invalid_response(
                "Responses completed response contains unfinished item",
            ));
        }
        let output_index = u32::try_from(output_index)
            .map_err(|_| invalid_response("Responses output index exceeded u32"))?;
        match item_type {
            "message" => {
                let item_id = item
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                    .map(ToString::to_string)
                    .unwrap_or_else(|| format!("{response_id}:message:{output_index}"));
                let phase = assistant_phase(item.get("phase"));
                let complete = item
                    .get("status")
                    .and_then(Value::as_str)
                    .map_or(response_completed, |status| status == "completed");
                let content = required_array(item, "content", "Responses message item")?;
                for (content_index, part) in content.iter().enumerate() {
                    let content_index = u32::try_from(content_index)
                        .map_err(|_| invalid_response("Responses content index exceeded u32"))?;
                    let part_type =
                        required_non_empty_string(part, "type", "Responses message content part")?;
                    let field = match part_type {
                        "output_text" => "text",
                        "refusal" => "refusal",
                        _ => continue,
                    };
                    let delta = required_string(part, field, "Responses message content part")?;
                    blocks.push(AssistantMessageBlock {
                        provider_source: native_request_id.map(|native| {
                            output_source(
                                LlmApiProtocol::Responses,
                                native,
                                &item_id,
                                output_index,
                                Some(content_index),
                                part_type,
                                item.get("phase").and_then(Value::as_str),
                            )
                        }),
                        block_id: format!("{item_id}:content:{content_index}"),
                        request_id: response_id.to_string(),
                        sequence,
                        phase,
                        delta: delta.to_string(),
                        complete,
                        provider: Some("openai_responses".to_string()),
                        reasoning: None,
                    });
                    sequence = sequence.saturating_add(1);
                }
            }
            "reasoning" => {
                let id = required_non_empty_string(item, "id", "Responses reasoning item")?;
                // 同一wire协议可返回encrypted原件或公开明文content；复用item校验，不伪造opaque数据。
                if !response_completed
                    && item.get("encrypted_content").is_none()
                    && !["summary", "content"].iter().any(|field| {
                        item.get(field)
                            .and_then(Value::as_array)
                            .is_some_and(|parts| !parts.is_empty())
                    })
                {
                    continue;
                }
                let opaque = reasoning_item_done(item)?
                    .and_then(|chunk| chunk.delta.reasoning_blocks)
                    .and_then(|blocks| blocks.into_iter().next());
                for (field, part_name) in [("summary", "summary"), ("content", "content")] {
                    for (index, part) in item
                        .get(field)
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .enumerate()
                    {
                        if let Some(text) = part.get("text").and_then(Value::as_str) {
                            let content_index = u32::try_from(index).map_err(|_| {
                                invalid_response("Responses reasoning content index exceeds u32")
                            })?;
                            let kind = part.get("type").and_then(Value::as_str).unwrap_or(
                                if field == "summary" {
                                    "summary_text"
                                } else {
                                    "reasoning_text"
                                },
                            );
                            blocks.push(AssistantMessageBlock {
                                provider_source: native_request_id.map(|native| {
                                    output_source(
                                        LlmApiProtocol::Responses,
                                        native,
                                        id,
                                        output_index,
                                        Some(content_index),
                                        kind,
                                        None,
                                    )
                                }),
                                block_id: format!("{id}:{part_name}:{index}"),
                                request_id: response_id.to_string(),
                                sequence,
                                phase: AssistantPhase::Reasoning,
                                delta: text.to_string(),
                                complete: response_completed
                                    || item.get("status").and_then(Value::as_str)
                                        == Some("completed"),
                                provider: Some("openai_responses".into()),
                                reasoning: None,
                            });
                            sequence = sequence.saturating_add(1);
                        }
                    }
                }
                if let Some(reasoning) = opaque {
                    blocks.push(AssistantMessageBlock {
                        provider_source: native_request_id.map(|native| {
                            output_source(
                                LlmApiProtocol::Responses,
                                native,
                                id,
                                output_index,
                                None,
                                "reasoning",
                                None,
                            )
                        }),
                        block_id: id.to_string(),
                        request_id: response_id.to_string(),
                        sequence,
                        phase: AssistantPhase::Reasoning,
                        delta: String::new(),
                        complete: response_completed
                            || item.get("status").and_then(Value::as_str) == Some("completed"),
                        provider: Some("openai_responses".to_string()),
                        reasoning: Some(reasoning),
                    });
                    sequence = sequence.saturating_add(1);
                }
            }
            _ => {}
        }
    }
    Ok(blocks)
}

fn assistant_phase(value: Option<&Value>) -> AssistantPhase {
    match value.and_then(Value::as_str) {
        Some("commentary") => AssistantPhase::Commentary,
        Some("partial_answer") => AssistantPhase::PartialAnswer,
        Some("final_answer") => AssistantPhase::FinalAnswer,
        _ => AssistantPhase::Unknown,
    }
}

#[cfg(test)]
fn response_to_chat(raw: Value) -> Result<ChatResponse> {
    response_to_chat_for_provider(raw, "openai_responses")
}

fn response_to_chat_for_provider(raw: Value, provider: &str) -> Result<ChatResponse> {
    // 原error首因先于其它字段解码；复用typed8 helper，原type/code不压成字符串。
    if let Some(error) = raw.get("error").filter(|error| !error.is_null()) {
        return Err(super::client::reported_provider_error(provider, error).into());
    }
    let response: ResponsesResponse = serde_json::from_value(raw.clone()).map_err(|error| {
        LlmError::InvalidResponse(format!("invalid Responses response: {error}"))
    })?;
    // 真实服务error仍沿原错误边界；不得被派生的D2 partial覆盖。
    if response.error.is_some() {
        ensure_completed_response(&response, "Responses response")?;
    }
    let incomplete = response.status.as_deref() != Some("completed");
    if !incomplete {
        require_non_empty(&response.id, "response id")?;
    }
    let metadata = response_metadata(&raw, provider)?;
    if !incomplete && response.output.is_empty() {
        return Err(invalid_response(
            "Responses response completed with empty output",
        ));
    }
    let canonical_id = if response.id.is_empty() {
        super::native_message::fallback_request_id("responses")
    } else {
        response.id.clone()
    };
    let native_blocks = native_assistant_blocks(&raw, &canonical_id)?;

    let mut text = String::new();
    let mut tool_calls = Vec::new();
    let mut reasoning_text = Vec::new();
    let mut reasoning_blocks = Vec::new();
    for (output_index, item) in response.output.into_iter().enumerate() {
        let output_index = u32::try_from(output_index)
            .map_err(|_| invalid_response("Responses output index exceeds u32"))?;
        match item {
            ResponsesOutputItem::Message { content, .. } => {
                for part in content {
                    match part {
                        ResponsesContent::OutputText { text: value } => text.push_str(&value),
                        ResponsesContent::Refusal { refusal } => text.push_str(&refusal),
                        ResponsesContent::Other => {}
                    }
                }
            }
            ResponsesOutputItem::FunctionCall {
                id,
                call_id,
                name,
                arguments,
                ..
            } => {
                require_non_empty(&call_id, "function_call.call_id")?;
                require_non_empty(&name, "function_call.name")?;
                if !incomplete {
                    require_non_empty(&arguments, "function_call.arguments")?;
                }
                require_non_empty(&id, "function_call.id")?;
                tool_calls.push(ToolCall {
                    provider_source: Some(output_source(
                        LlmApiProtocol::Responses,
                        &response.id,
                        &id,
                        output_index,
                        None,
                        "function_call",
                        None,
                    )),
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
                let contents = content
                    .into_iter()
                    .map(|part| part.text)
                    .collect::<Vec<_>>();
                reasoning_text.extend(if contents.is_empty() {
                    summaries.clone()
                } else {
                    contents
                });
                require_non_empty(&id, "reasoning.id")?;
                let encrypted = encrypted_content.unwrap_or_default();
                if !encrypted.is_empty() {
                    reasoning_blocks.push(ReasoningBlock::Opaque {
                        provider: "openai_responses".to_string(),
                        id,
                        data: encrypted,
                        summary: summaries.clone(),
                    });
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
        assistant_blocks: (!native_blocks.is_empty()).then_some(native_blocks.clone()),
        provider_response: Some(metadata),
        ..Default::default()
    };
    if incomplete {
        return Err(LlmError::IncompleteResponse {
            partial: Box::new(message),
        }
        .into());
    }
    let usage = response.usage.map(Usage::from);
    let finish_reason = finish_reason(has_tool_calls);
    super::native_message::validate_completion(&message, finish_reason.as_deref())?;
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

type NativeTextCoordinate = (String, u32, Option<u32>, String);
type EmittedText = (String, Option<NativeTextCoordinate>);

#[derive(Default)]
struct ResponsesStreamAdapter {
    provider: String,
    partial: Message,
    function_calls: HashMap<u32, StreamFunctionCall>,
    text: HashMap<StreamTextKey, String>,
    completed: bool,
    request_id: String,
    phases: HashMap<String, AssistantPhase>,
    raw_phases: HashMap<String, String>,
    // 同一既有block map保文本和原始坐标；坐标不含伪native request ID，不新建记录权威。
    emitted_text: HashMap<String, EmittedText>,
    closed_blocks: HashSet<String>,
    metadata_completed: HashSet<String>,
    sequence: u64,
    // 内容块事件只给出父item坐标，不声明父item类型；缺失类型保持未知。
    observed_items: HashMap<u32, (String, Option<String>)>,
    closed_arguments: HashSet<u32>,
}

impl ResponsesStreamAdapter {
    fn map_event(&mut self, event: Value) -> Result<Option<ChatChunk>> {
        let event_type = required_non_empty_string(&event, "type", "Responses stream event")?;
        // 有效原ProviderError、坏JSON/字段、实际传输失败保持首因，不改成D2。
        if event_type == "error"
            || matches!(
                event_type,
                "response.failed" | "response.incomplete" | "response.cancelled"
            ) && event
                .get("response")
                .and_then(|raw| raw.get("error"))
                .is_some_and(|error| !error.is_null())
        {
            return self.map_native_event(event);
        }
        if self.completed {
            return self.map_native_event(event);
        }
        let native_was_unknown = self.native_request_id().is_none();
        let observed = if let Some(raw) = event.get("response") {
            self.observe_response(raw)?;
            true
        } else {
            false
        };
        if matches!(
            event_type,
            "response.incomplete" | "response.failed" | "response.cancelled"
        ) || event_type == "response.completed"
            && event
                .get("response")
                .and_then(|response| response.get("status"))
                .is_none_or(|status| status.as_str() != Some("completed"))
        {
            let raw = required_object(&event, "response", "Responses incomplete event")?;
            let partial = self.partial_from_response(raw)?;
            return Err(LlmError::IncompleteResponse {
                partial: Box::new(partial),
            }
            .into());
        }
        let block_sequence_start = self.sequence;
        let mut chunk = match self.map_native_event(event) {
            Err(echo_core::error::ReactError::Llm(error)) => match *error {
                LlmError::IncompleteResponse { mut partial } => {
                    partial.provider_response = self.partial.provider_response.clone();
                    return Err(LlmError::IncompleteResponse { partial }.into());
                }
                error => return Err(error.into()),
            },
            result => result?,
        };
        if observed && chunk.is_none() {
            chunk = Some(ChatChunk {
                delta: DeltaMessage::default(),
                finish_reason: None,
                usage: None,
            });
        }
        if let Some(chunk) = &mut chunk {
            if native_was_unknown && self.native_request_id().is_some() {
                self.bind_late_sources(chunk)?;
            }
            // 本chunk尚未公开，可按依赖重排后统一续接原全局sequence；既发观察身份不改。
            self.sequence = block_sequence_start;
            for block in chunk.delta.assistant_blocks.iter_mut().flatten() {
                block.sequence = self.sequence;
                self.sequence = self.sequence.saturating_add(1);
            }
            chunk.delta.provider_response = self.partial.provider_response.clone();
            self.partial.role = Role::Assistant;
            if let Some(text) = &chunk.delta.content {
                let mut content = self.partial.content.as_text().unwrap_or_default();
                content.push_str(text);
                self.partial.content = MessageContent::Text(content);
            }
            if let Some(text) = &chunk.delta.reasoning_content {
                self.partial
                    .reasoning_content
                    .get_or_insert_with(String::new)
                    .push_str(text);
            }
            if let Some(blocks) = &chunk.delta.assistant_blocks {
                self.partial
                    .assistant_blocks
                    .get_or_insert_with(Vec::new)
                    .extend(blocks.iter().cloned());
            }
            for block in chunk.delta.reasoning_blocks.iter().flatten() {
                let blocks = self.partial.reasoning_blocks.get_or_insert_with(Vec::new);
                if !blocks.contains(block) {
                    blocks.push(block.clone());
                }
            }
        }
        Ok(chunk)
    }

    /// 失败full response可以晚到或省略先前已给的reasoning原件；只合并同item真实已观察字段。
    fn native_request_id(&self) -> Option<&str> {
        self.partial
            .provider_response
            .as_ref()
            .and_then(|record| record.request_id.as_deref())
    }

    fn block_source(
        &mut self,
        block_id: &str,
        item_id: &str,
        index: u32,
        part: Option<u32>,
        kind: &str,
    ) -> Option<echo_core::llm::types::ProviderOutputSource> {
        self.emitted_text.entry(block_id.to_string()).or_default().1 =
            Some((item_id.to_string(), index, part, kind.to_string()));
        self.native_request_id().map(|native| {
            output_source(
                LlmApiProtocol::Responses,
                native,
                item_id,
                index,
                part,
                kind,
                self.raw_phases.get(item_id).map(String::as_str),
            )
        })
    }

    /// 晚ID只修正既有来源，不重发正文/参数或修改已发canonical request/block身份。
    fn bind_late_sources(&mut self, chunk: &mut ChatChunk) -> Result<()> {
        let native = self
            .native_request_id()
            .ok_or_else(|| invalid_response("late source binding omitted native ID"))?
            .to_string();
        let mut updates = std::collections::BTreeMap::new();
        for block in self.partial.assistant_blocks.iter_mut().flatten() {
            if block.provider_source.is_some() {
                continue;
            }
            let (item, index, part, kind) = self
                .emitted_text
                .get(&block.block_id)
                .and_then(|entry| entry.1.as_ref())
                .ok_or_else(|| {
                    invalid_response("late source binding lost original block coordinates")
                })?;
            let source = output_source(
                LlmApiProtocol::Responses,
                &native,
                item,
                *index,
                *part,
                kind,
                self.raw_phases.get(item).map(String::as_str),
            );
            block.provider_source = Some(source.clone());
            let mut update = block.clone();
            update.provider_source = Some(source);
            update.delta.clear();
            update.reasoning = None;
            update.complete = false;
            updates.insert((block.request_id.clone(), block.block_id.clone()), update);
        }
        let mut prefix_updates = Vec::new();
        for (_, update) in updates {
            if chunk.delta.assistant_blocks.as_ref().is_some_and(|blocks| {
                blocks.iter().any(|block| {
                    block.request_id == update.request_id
                        && block.block_id == update.block_id
                        && block.provider_source.is_some()
                })
            }) {
                continue;
            }
            prefix_updates.push(update);
        }
        if !prefix_updates.is_empty() {
            let blocks = chunk.delta.assistant_blocks.get_or_insert_with(Vec::new);
            // 已呈现child先迁到真实native坐标，依赖该prefix的Opaque parent随后才可投影。
            prefix_updates.append(blocks);
            *blocks = prefix_updates;
        }
        let mut indexes = self.function_calls.keys().copied().collect::<Vec<_>>();
        indexes.sort_unstable();
        for index in indexes {
            let call = self
                .function_calls
                .get(&index)
                .ok_or_else(|| invalid_response("late source tool disappeared"))?;
            let calls = chunk.delta.tool_calls.get_or_insert_with(Vec::new);
            if calls.iter().any(|call| call.index == index) {
                continue;
            }
            calls.push(DeltaToolCall {
                provider_source: Some(output_source(
                    LlmApiProtocol::Responses,
                    &native,
                    &call.item_id,
                    index,
                    None,
                    "function_call",
                    None,
                )),
                index,
                id: None,
                call_type: None,
                function: None,
            });
        }
        Ok(())
    }

    fn partial_from_response(&self, raw: &Value) -> Result<Message> {
        let mut native = raw.clone();
        // 同请求省略ID不清除已观察真ID；仅partial解码复用，不放宽成功缺ID错误。
        if native.get("id").is_none_or(Value::is_null)
            && let Some(actual) = self.native_request_id()
        {
            native
                .as_object_mut()
                .ok_or_else(|| invalid_response("partial response object missing"))?
                .insert("id".into(), json!(actual));
        }
        let output = native
            .get_mut("output")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| invalid_response("incomplete response output missing"))?;
        for item in output.iter_mut() {
            if item.get("type").and_then(Value::as_str) == Some("message")
                && item.get("phase").is_none()
                && let Some(phase) = item
                    .get("id")
                    .and_then(Value::as_str)
                    .and_then(|id| self.raw_phases.get(id))
            {
                item.as_object_mut()
                    .ok_or_else(|| invalid_response("incomplete item missing"))?
                    .insert("phase".into(), json!(phase));
            }
        }
        for block in self.partial.reasoning_blocks.iter().flatten() {
            let ReasoningBlock::Opaque {
                id, data, summary, ..
            } = block
            else {
                continue;
            };
            let item = output
                .iter_mut()
                .find(|item| item.get("id").and_then(Value::as_str) == Some(id.as_str()))
                .and_then(Value::as_object_mut)
                .ok_or_else(|| {
                    invalid_response("incomplete response omitted observed reasoning")
                })?;
            match item.get("encrypted_content") {
                Some(value) if value.as_str() != Some(data.as_str()) => {
                    return Err(invalid_response(
                        "incomplete response changed observed encrypted reasoning",
                    ));
                }
                None => {
                    item.insert("encrypted_content".into(), json!(data));
                }
                _ => {}
            }
            if let Some(parts) = item.get("summary").and_then(Value::as_array) {
                if parts.len() < summary.len()
                    || summary.iter().enumerate().any(|(index, text)| {
                        parts
                            .get(index)
                            .and_then(|part| part.get("text"))
                            .and_then(Value::as_str)
                            != Some(text.as_str())
                    })
                {
                    return Err(invalid_response(
                        "incomplete response changed observed reasoning summary",
                    ));
                }
            } else if item.get("summary").is_none() {
                item.insert(
                    "summary".into(),
                    json!(
                        summary
                            .iter()
                            .map(|text| json!({"type":"summary_text","text":text}))
                            .collect::<Vec<_>>()
                    ),
                );
            }
        }
        self.validate_partial_response(&native)?;
        let provider = if self.provider.is_empty() {
            "openai_responses"
        } else {
            &self.provider
        };
        match response_to_chat_for_provider(native, provider) {
            Err(echo_core::error::ReactError::Llm(error)) => match *error {
                LlmError::IncompleteResponse { mut partial } => {
                    partial.provider_response = self.partial.provider_response.clone();
                    for block in partial.assistant_blocks.iter_mut().flatten() {
                        if !self.request_id.is_empty() {
                            block.request_id = self.request_id.clone();
                        }
                    }
                    Ok(*partial)
                }
                error => Err(error.into()),
            },
            Err(error) => Err(error),
            Ok(_) => Err(invalid_response(
                "incomplete event contains completed response",
            )),
        }
    }

    /// 完整失败响应仍须与已解码片段同源；协议损坏不能被D2包装成可信partial。
    fn validate_partial_response(&self, raw: &Value) -> Result<()> {
        let output = required_array(raw, "output", "Responses incomplete response")?;
        for (index, (id, kind)) in &self.observed_items {
            let item = usize::try_from(*index)
                .ok()
                .and_then(|index| output.get(index))
                .ok_or_else(|| invalid_response("incomplete response omitted observed item"))?;
            if item.get("id").and_then(Value::as_str) != Some(id.as_str())
                || kind
                    .as_deref()
                    .is_some_and(|kind| item.get("type").and_then(Value::as_str) != Some(kind))
            {
                return Err(invalid_response(
                    "incomplete response changed observed item",
                ));
            }
            if self.raw_phases.contains_key(id)
                && item.get("phase").is_some_and(|phase| !phase.is_string())
            {
                return Err(invalid_response(
                    "incomplete response replaced observed phase with null or invalid value",
                ));
            }
            if let Some(previous) = self.phases.get(id) {
                let current = assistant_phase(item.get("phase"));
                if *previous != AssistantPhase::Unknown
                    && current != AssistantPhase::Unknown
                    && current != *previous
                {
                    return Err(invalid_response(
                        "incomplete response changed observed phase",
                    ));
                }
            }
            if let Some(current) = item.get("phase").and_then(Value::as_str)
                && self
                    .raw_phases
                    .get(id)
                    .is_some_and(|previous| previous != current)
            {
                return Err(invalid_response("incomplete response changed raw phase"));
            }
        }
        // item.done也会建立canonical原件而没有text.delta；按同一source坐标核验，不能覆盖。
        for block in self.partial.assistant_blocks.iter().flatten() {
            let (output_index, content_index, content_kind) = match block.provider_source.as_ref() {
                Some(source) => (
                    source.output_index,
                    source.content_index,
                    source.content_kind.as_deref(),
                ),
                None => {
                    let Some((_, index, part, kind)) = self
                        .emitted_text
                        .get(&block.block_id)
                        .and_then(|entry| entry.1.as_ref())
                    else {
                        continue;
                    };
                    (*index, *part, Some(kind.as_str()))
                }
            };
            let Some(index) = content_index else {
                continue;
            };
            let (field, value_field) = match content_kind {
                Some("summary_text") => ("summary", "text"),
                Some("reasoning_text" | "output_text") => ("content", "text"),
                Some("refusal") => ("content", "refusal"),
                _ => continue,
            };
            let part = usize::try_from(output_index)
                .ok()
                .and_then(|index| output.get(index))
                .and_then(|item| item.get(field))
                .and_then(Value::as_array)
                .and_then(|parts| {
                    usize::try_from(index)
                        .ok()
                        .and_then(|index| parts.get(index))
                })
                .ok_or_else(|| invalid_response("incomplete response omitted canonical content"))?;
            if part.get("type").and_then(Value::as_str) != content_kind {
                return Err(invalid_response(
                    "incomplete response changed canonical content kind",
                ));
            }
            let completed =
                required_string(part, value_field, "Responses incomplete canonical content")?;
            let observed = self
                .emitted_text
                .get(&block.block_id)
                .map(|entry| entry.0.as_str())
                .unwrap_or_default();
            if self.closed_blocks.contains(&block.block_id) && observed != completed {
                return Err(invalid_response(
                    "incomplete response changed closed canonical content",
                ));
            }
            let mut before = observed.to_string();
            reconcile_terminal_text(&mut before, completed, "incomplete canonical content")?;
        }
        for (key, text) in &self.text {
            let field = if key.kind == StreamTextKind::ReasoningSummary {
                "summary"
            } else {
                "content"
            };
            let value_field = if key.kind == StreamTextKind::Refusal {
                "refusal"
            } else {
                "text"
            };
            let part = usize::try_from(key.output_index)
                .ok()
                .and_then(|index| output.get(index))
                .and_then(|item| item.get(field))
                .and_then(Value::as_array)
                .and_then(|parts| {
                    usize::try_from(key.content_index)
                        .ok()
                        .and_then(|index| parts.get(index))
                })
                .ok_or_else(|| invalid_response("incomplete response omitted observed text"))?;
            let completed = required_string(part, value_field, "Responses incomplete content")?;
            let mut seen = text.clone();
            reconcile_terminal_text(&mut seen, completed, "incomplete content")?;
        }
        for (index, call) in &self.function_calls {
            let item = usize::try_from(*index)
                .ok()
                .and_then(|index| output.get(index))
                .ok_or_else(|| invalid_response("incomplete response omitted observed tool"))?;
            if item.get("id").and_then(Value::as_str) != Some(call.item_id.as_str())
                || item.get("call_id").and_then(Value::as_str) != Some(call.call_id.as_str())
                || item.get("name").and_then(Value::as_str) != Some(call.name.as_str())
            {
                return Err(invalid_response(
                    "incomplete response changed tool identity",
                ));
            }
            let completed = required_string(item, "arguments", "Responses incomplete tool")?;
            let mut seen = call.arguments.clone();
            reconcile_terminal_text(&mut seen, completed, "incomplete arguments")?;
        }
        Ok(())
    }

    fn observe_response(&mut self, raw: &Value) -> Result<()> {
        let provider = if self.provider.is_empty() {
            "openai_responses"
        } else {
            &self.provider
        };
        if !["id", "model", "status", "incomplete_details", "usage"]
            .iter()
            .any(|field| raw.get(field).is_some())
        {
            return Err(invalid_response(
                "Responses observation contained no native fields",
            ));
        }
        let patch = response_metadata(raw, provider)?;
        if let Some(previous) = &self.partial.provider_response {
            if previous
                .request_id
                .as_ref()
                .zip(patch.request_id.as_ref())
                .is_some_and(|(a, b)| a != b)
            {
                return Err(invalid_response(
                    "Responses observation changed native request id",
                ));
            }
            let mut snapshot = serde_json::to_value(previous)?;
            let incoming = serde_json::to_value(patch)?;
            let target = snapshot
                .as_object_mut()
                .ok_or_else(|| invalid_response("observation snapshot missing"))?;
            for (key, value) in incoming
                .as_object()
                .ok_or_else(|| invalid_response("observation patch missing"))?
            {
                if key != "request_id" || !value.is_null() {
                    target.insert(key.clone(), value.clone());
                }
            }
            self.partial.provider_response = Some(serde_json::from_value(snapshot)?);
        } else {
            self.partial.provider_response = Some(patch);
        }
        self.partial.role = Role::Assistant;
        Ok(())
    }

    fn incomplete_error(&self) -> echo_core::error::ReactError {
        let mut partial = self.partial.clone();
        partial.role = Role::Assistant;
        partial.execution_status = None;
        partial.approved_assistant_blocks = None;
        let mut indexes = self.function_calls.keys().copied().collect::<Vec<_>>();
        indexes.sort_unstable();
        let calls = indexes
            .into_iter()
            .filter_map(|index| {
                self.function_calls.get(&index).map(|call| ToolCall {
                    provider_source: Some(output_source(
                        LlmApiProtocol::Responses,
                        self.native_request_id().unwrap_or(""),
                        &call.item_id,
                        index,
                        None,
                        "function_call",
                        None,
                    )),
                    id: call.call_id.clone(),
                    call_type: "function".into(),
                    function: FunctionCall {
                        name: call.name.clone(),
                        arguments: call.arguments.clone(),
                    },
                })
            })
            .collect::<Vec<_>>();
        partial.tool_calls = (!calls.is_empty()).then_some(calls);
        LlmError::IncompleteResponse {
            partial: Box::new(partial),
        }
        .into()
    }

    fn map_native_event(&mut self, event: Value) -> Result<Option<ChatChunk>> {
        let event_type =
            required_non_empty_string(&event, "type", "Responses stream event")?.to_string();
        if event_type.ends_with(".delta")
            && event_type != "response.function_call_arguments.delta"
            && let (Some(id), Some(index)) = (
                event.get("item_id").and_then(Value::as_str),
                event
                    .get(if event_type.contains("summary") {
                        "summary_index"
                    } else {
                        "content_index"
                    })
                    .and_then(Value::as_u64),
            )
        {
            let part = if event_type.contains("summary") {
                "summary"
            } else {
                "content"
            };
            if self.closed_blocks.contains(&format!("{id}:{part}:{index}")) {
                return Err(invalid_response(
                    "Responses emitted delta after block completion",
                ));
            }
        }
        if event_type.starts_with("response.function_call_arguments.") {
            let index = required_u32(&event, "output_index", "Responses tool arguments")?;
            if event_type.ends_with(".delta") && self.closed_arguments.contains(&index) {
                return Err(invalid_response(
                    "Responses emitted arguments after arguments.done",
                ));
            }
            if event_type.ends_with(".done") {
                self.closed_arguments.insert(index);
            }
        }
        if let Some(index) = event
            .get("output_index")
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
        {
            let (id, kind) = if matches!(
                event_type.as_str(),
                "response.content_part.added" | "response.content_part.done"
            ) {
                // 该事件的part是内容块，不是message父项；不能把reasoning父项
                // 改猜成message。ID仍立即登记，等待output_item或明确delta绑定类型。
                (event.get("item_id").and_then(Value::as_str), None)
            } else if let Some(item) = event.get("item") {
                (
                    item.get("id").and_then(Value::as_str),
                    item.get("type").and_then(Value::as_str),
                )
            } else {
                (
                    event.get("item_id").and_then(Value::as_str),
                    Some(if event_type.contains("function_call") {
                        "function_call"
                    } else if event_type.contains("reasoning") {
                        "reasoning"
                    } else {
                        "message"
                    }),
                )
            };
            if let Some(id) = id {
                if let Some(previous) = self.observed_items.get(&index)
                    && (previous.0 != id
                        || previous
                            .1
                            .as_deref()
                            .zip(kind)
                            .is_some_and(|(before, after)| before != after))
                {
                    // 只记录事件类别与身份变化类型，不输出正文、arguments或完整响应；
                    // 保留原拒绝条件，便于区分上游ID漂移和条目类型不一致。
                    return Err(invalid_response(format!(
                        "Responses output item identity changed (event={}, output_index={index}, id_changed={}, previous_type={}, current_type={})",
                        event_type.chars().take(96).collect::<String>(),
                        previous.0 != id,
                        previous
                            .1
                            .as_deref()
                            .unwrap_or("unknown")
                            .chars()
                            .take(64)
                            .collect::<String>(),
                        kind.unwrap_or("unknown")
                            .chars()
                            .take(64)
                            .collect::<String>(),
                    )));
                }
                if self
                    .observed_items
                    .iter()
                    .any(|(other, item)| *other != index && item.0 == id)
                {
                    return Err(invalid_response(
                        "Responses reused item id at different output index",
                    ));
                }
                let resolved_kind = kind.map(ToString::to_string).or_else(|| {
                    self.observed_items
                        .get(&index)
                        .and_then(|previous| previous.1.clone())
                });
                self.observed_items
                    .insert(index, (id.to_string(), resolved_kind));
            }
        }
        if self.request_id.is_empty() {
            self.request_id = event
                .get("response")
                .and_then(|response| response.get("id"))
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .map(ToString::to_string)
                .unwrap_or_else(|| super::native_message::fallback_request_id("responses"));
        }
        let mut discovered_phase = false;
        if let Some(item) = event.get("item")
            && item.get("type").and_then(Value::as_str) == Some("message")
            && let Some(id) = item.get("id").and_then(Value::as_str)
        {
            if let Some(raw_phase) = item.get("phase").and_then(Value::as_str) {
                discovered_phase = !self.raw_phases.contains_key(id);
                if self
                    .raw_phases
                    .get(id)
                    .is_some_and(|previous| previous != raw_phase)
                {
                    return Err(invalid_response("Responses raw phase changed"));
                }
                self.raw_phases
                    .insert(id.to_string(), raw_phase.to_string());
            }
            let phase = assistant_phase(item.get("phase"));
            if let Some(previous) = self.phases.get(id)
                && *previous != AssistantPhase::Unknown
                && phase != AssistantPhase::Unknown
                && *previous != phase
            {
                return Err(invalid_response("Responses known message phase changed"));
            }
            if phase != AssistantPhase::Unknown || !self.phases.contains_key(id) {
                self.phases.insert(id.to_string(), phase);
            }
        }
        if event_type == "response.completed" {
            let response = required_object(&event, "response", "Responses completed event")?;
            let output = required_array(response, "output", "Responses response")?;
            for (index, (id, kind)) in &self.observed_items {
                let item = usize::try_from(*index)
                    .ok()
                    .and_then(|index| output.get(index))
                    .ok_or_else(|| {
                        invalid_response("Responses completed output omitted observed item")
                    })?;
                if item.get("id").and_then(Value::as_str) != Some(id.as_str())
                    || kind
                        .as_deref()
                        .is_some_and(|kind| item.get("type").and_then(Value::as_str) != Some(kind))
                {
                    return Err(invalid_response(
                        "Responses completed output changed observed item",
                    ));
                }
                if let Some(previous) = self.phases.get(id) {
                    let current = assistant_phase(item.get("phase"));
                    if *previous != AssistantPhase::Unknown && current != *previous {
                        return Err(invalid_response("Responses completed output changed phase"));
                    }
                }
            }
            // Validate the same full response as the non-streaming adapter.
            let full = response_to_chat_for_provider(
                response.clone(),
                if self.provider.is_empty() {
                    "openai_responses"
                } else {
                    &self.provider
                },
            )?;
            let mut terminal = self
                .map_legacy_event(event.clone())?
                .ok_or_else(|| invalid_response("terminal chunk was omitted"))?;
            let mut native = Vec::new();
            for block in native_assistant_blocks(response, &self.request_id)? {
                if self.metadata_completed.contains(&block.block_id) {
                    let previous = self
                        .emitted_text
                        .get(&block.block_id)
                        .map(|entry| entry.0.as_str())
                        .unwrap_or_default();
                    if previous != block.delta {
                        return Err(invalid_response(
                            "Responses completed output disagreed with closed block",
                        ));
                    }
                    continue;
                }
                let accumulated = &mut self
                    .emitted_text
                    .entry(block.block_id.clone())
                    .or_default()
                    .0;
                let suffix = reconcile_terminal_text(accumulated, &block.delta, "canonical block")?
                    .unwrap_or_default();
                let mut completed = block;
                completed.delta = suffix;
                completed.sequence = self.sequence;
                self.sequence = self.sequence.saturating_add(1);
                self.closed_blocks.insert(completed.block_id.clone());
                self.metadata_completed.insert(completed.block_id.clone());
                if !completed.delta.is_empty() {
                    let text = if completed.phase == AssistantPhase::Reasoning {
                        &mut terminal.delta.reasoning_content
                    } else {
                        &mut terminal.delta.content
                    };
                    text.get_or_insert_with(String::new)
                        .push_str(&completed.delta);
                }
                if let Some(reasoning) = completed.reasoning.clone() {
                    terminal
                        .delta
                        .reasoning_blocks
                        .get_or_insert_with(Vec::new)
                        .push(reasoning);
                }
                native.push(completed);
            }
            for (index, item) in required_array(response, "output", "Responses response")?
                .iter()
                .enumerate()
            {
                if item.get("type").and_then(Value::as_str) == Some("function_call") {
                    let index = u32::try_from(index)
                        .map_err(|_| invalid_response("Responses output index exceeds u32"))?;
                    if let Some(chunk) = self.function_call_item_done(
                        index,
                        required_non_empty_string(item, "id", "Responses tool item")?,
                        item,
                    )? {
                        terminal
                            .delta
                            .tool_calls
                            .get_or_insert_with(Vec::new)
                            .extend(chunk.delta.tool_calls.unwrap_or_default());
                    }
                }
            }
            if !native.is_empty() {
                terminal.delta.assistant_blocks = Some(native);
            }
            self.attach_tool_sources(&mut terminal);
            let _ = full;
            return Ok(Some(terminal));
        }
        let mut chunk = self.map_legacy_event(event.clone())?;
        if let Some(chunk) = chunk.as_mut() {
            self.attach_tool_sources(chunk);
        }
        let mut native = Vec::new();
        if matches!(
            event_type.as_str(),
            "response.output_text.delta"
                | "response.refusal.delta"
                | "response.reasoning_text.delta"
                | "response.reasoning_summary_text.delta"
                | "response.output_text.done"
                | "response.refusal.done"
                | "response.reasoning_text.done"
                | "response.reasoning_summary_text.done"
        ) {
            let item_id = required_non_empty_string(&event, "item_id", "Responses text event")?;
            let reasoning = event_type.contains("reasoning");
            let index_field = if event_type.contains("summary") {
                "summary_index"
            } else {
                "content_index"
            };
            let index = required_u32(&event, index_field, "Responses text event")?;
            let part = if event_type.contains("summary") {
                "summary"
            } else {
                "content"
            };
            let block_id = format!("{item_id}:{part}:{index}");
            let text_done = event_type.ends_with(".done");
            let text = chunk
                .as_ref()
                .and_then(|value| {
                    if reasoning {
                        value.delta.reasoning_content.clone()
                    } else {
                        value.delta.content.clone()
                    }
                })
                .unwrap_or_default();
            let phase = if reasoning {
                AssistantPhase::Reasoning
            } else {
                self.phases
                    .get(item_id)
                    .copied()
                    .unwrap_or(AssistantPhase::Unknown)
            };
            if !self.closed_blocks.contains(&block_id) {
                self.emitted_text
                    .entry(block_id.clone())
                    .or_default()
                    .0
                    .push_str(&text);
                if text_done {
                    self.closed_blocks.insert(block_id.clone());
                }
                let native_kind = if event_type.contains("refusal") {
                    "refusal"
                } else if event_type.contains("summary") {
                    "summary_text"
                } else if reasoning {
                    "reasoning_text"
                } else {
                    "output_text"
                };
                native.push(AssistantMessageBlock {
                    provider_source: self.block_source(
                        &block_id,
                        item_id,
                        required_u32(&event, "output_index", "Responses text event")?,
                        Some(index),
                        native_kind,
                    ),
                    block_id,
                    request_id: self.request_id.clone(),
                    sequence: self.sequence,
                    phase,
                    delta: text,
                    complete: false,
                    provider: Some("openai_responses".into()),
                    reasoning: None,
                });
            }
        } else if event_type == "response.output_item.done" {
            let item = required_object(&event, "item", "Responses item done")?;
            let item_id = required_non_empty_string(item, "id", "Responses item")?;
            match item.get("type").and_then(Value::as_str) {
                Some("message") => {
                    let phase = assistant_phase(item.get("phase"));
                    for (index, part) in required_array(item, "content", "Responses item")?
                        .iter()
                        .enumerate()
                    {
                        let field = match part.get("type").and_then(Value::as_str) {
                            Some("output_text") => "text",
                            Some("refusal") => "refusal",
                            _ => continue,
                        };
                        let block_id = format!("{item_id}:content:{index}");
                        let already_closed = !self.closed_blocks.insert(block_id.clone());
                        // Native item content is closed, but optional message
                        // metadata can still arrive on the complete response.
                        if self.metadata_completed.contains(&block_id) {
                            continue;
                        }
                        let text = required_string(part, field, "Responses content")?;
                        let accumulated =
                            &mut self.emitted_text.entry(block_id.clone()).or_default().0;
                        let suffix =
                            reconcile_terminal_text(accumulated, text, field)?.unwrap_or_default();
                        // 相同item.done重入没有新正文/阶段时保持幂等；晚到的首次
                        // provider阶段仍必须运输，空content槽首次出现也不能丢失。
                        if already_closed && suffix.is_empty() && !discovered_phase {
                            continue;
                        }
                        native.push(AssistantMessageBlock {
                            provider_source: self.block_source(
                                &block_id,
                                item_id,
                                required_u32(&event, "output_index", "Responses message done")?,
                                Some(u32::try_from(index).map_err(|_| {
                                    invalid_response("Responses content index exceeds u32")
                                })?),
                                if field == "text" {
                                    "output_text"
                                } else {
                                    "refusal"
                                },
                            ),
                            block_id,
                            request_id: self.request_id.clone(),
                            sequence: self.sequence,
                            phase,
                            delta: suffix,
                            complete: false,
                            provider: Some("openai_responses".into()),
                            reasoning: None,
                        });
                    }
                }
                Some("reasoning") if !self.metadata_completed.contains(item_id) => {
                    let reasoning = chunk
                        .as_ref()
                        .and_then(|chunk| chunk.delta.reasoning_blocks.as_ref())
                        .and_then(|blocks| blocks.first())
                        .cloned();
                    if reasoning.is_some() {
                        self.metadata_completed.insert(item_id.to_string());
                        native.push(AssistantMessageBlock {
                            provider_source: self.block_source(
                                item_id,
                                item_id,
                                required_u32(&event, "output_index", "Responses reasoning done")?,
                                None,
                                "reasoning",
                            ),
                            block_id: item_id.to_string(),
                            request_id: self.request_id.clone(),
                            sequence: self.sequence,
                            phase: AssistantPhase::Reasoning,
                            delta: String::new(),
                            complete: true,
                            provider: Some("openai_responses".into()),
                            reasoning,
                        });
                    }
                    // Opaque与公开parts是同一item的独立原件；item.done先到也不能等completed才记住。
                    {
                        let mut unseen_reasoning = String::new();
                        for field in ["summary", "content"] {
                            for (index, part) in item
                                .get(field)
                                .and_then(Value::as_array)
                                .into_iter()
                                .flatten()
                                .enumerate()
                            {
                                let text =
                                    required_string(part, "text", "Responses reasoning text")?;
                                let block_id = format!("{item_id}:{field}:{index}");
                                if self.metadata_completed.contains(&block_id) {
                                    continue;
                                }
                                let accumulated =
                                    &mut self.emitted_text.entry(block_id.clone()).or_default().0;
                                let suffix =
                                    reconcile_terminal_text(accumulated, text, "reasoning text")?
                                        .unwrap_or_default();
                                unseen_reasoning.push_str(&suffix);
                                self.closed_blocks.insert(block_id.clone());
                                self.metadata_completed.insert(block_id.clone());
                                native.push(AssistantMessageBlock {
                                    provider_source: self.block_source(
                                        &block_id,
                                        item_id,
                                        required_u32(
                                            &event,
                                            "output_index",
                                            "Responses reasoning done",
                                        )?,
                                        Some(u32::try_from(index).map_err(|_| {
                                            invalid_response("reasoning content index exceeds u32")
                                        })?),
                                        part.get("type").and_then(Value::as_str).unwrap_or(
                                            if field == "summary" {
                                                "summary_text"
                                            } else {
                                                "reasoning_text"
                                            },
                                        ),
                                    ),
                                    block_id,
                                    request_id: self.request_id.clone(),
                                    sequence: self.sequence,
                                    phase: AssistantPhase::Reasoning,
                                    delta: suffix,
                                    complete: true,
                                    provider: Some("openai_responses".into()),
                                    reasoning: None,
                                });
                            }
                        }
                        if !unseen_reasoning.is_empty() {
                            let value = chunk.get_or_insert_with(|| ChatChunk {
                                delta: DeltaMessage::default(),
                                finish_reason: None,
                                usage: None,
                            });
                            value
                                .delta
                                .reasoning_content
                                .get_or_insert_with(String::new)
                                .push_str(&unseen_reasoning);
                        }
                    }
                }
                _ => {}
            }
        }
        if !native.is_empty() {
            for block in &mut native {
                block.sequence = self.sequence;
                self.sequence = self.sequence.saturating_add(1);
            }
            let value = chunk.get_or_insert_with(|| ChatChunk {
                delta: DeltaMessage::default(),
                finish_reason: None,
                usage: None,
            });
            value.delta.assistant_blocks = Some(native);
        }
        Ok(chunk)
    }

    fn attach_tool_sources(&self, chunk: &mut ChatChunk) {
        for tool in chunk.delta.tool_calls.as_mut().into_iter().flatten() {
            if let Some(state) = self.function_calls.get(&tool.index) {
                tool.provider_source = self.native_request_id().map(|native| {
                    output_source(
                        LlmApiProtocol::Responses,
                        native,
                        &state.item_id,
                        tool.index,
                        None,
                        "function_call",
                        None,
                    )
                });
            }
        }
    }

    fn map_legacy_event(&mut self, event: Value) -> Result<Option<ChatChunk>> {
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
                if let Some(error) = event
                    .get("response")
                    .and_then(|response| response.get("error"))
                    .filter(|error| !error.is_null())
                {
                    Err(super::client::reported_provider_error(
                        if self.provider.is_empty() {
                            "openai_responses"
                        } else {
                            &self.provider
                        },
                        error,
                    )
                    .into())
                } else {
                    Err(stream_failure(&event).into())
                }
            }
            "error" => Err(super::client::reported_provider_error(
                if self.provider.is_empty() {
                    "openai_responses"
                } else {
                    &self.provider
                },
                event.get("error").unwrap_or(&event),
            )
            .into()),
            _ => Ok(None),
        }
    }

    fn validate_transport_end(&self, boundary: &str) -> Result<()> {
        if !self.completed {
            if self.partial.provider_response.is_some()
                || self.partial.content.as_text().is_some()
                || self
                    .partial
                    .assistant_blocks
                    .as_ref()
                    .is_some_and(|blocks| !blocks.is_empty())
                || !self.function_calls.is_empty()
            {
                return Err(self.incomplete_error());
            }
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
        // `added` 只声明调用身份。OpenAI 会带一个空串，部分兼容网关直接省略
        // 该键（参数随后由 `response.function_call_arguments.delta` 补齐）；
        // 缺失/null 都等价于空串。完整性仍由 `item.done` 强制，不放宽终态。
        let arguments = match item.get("arguments") {
            None | Some(Value::Null) => "",
            Some(Value::String(value)) => value.as_str(),
            Some(_) => {
                return Err(invalid_response(
                    "Responses function_call arguments was not a string",
                ));
            }
        };
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
                provider_source: None,
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
    // `null` 与空串都表示"没有不透明负载"：兼容网关没有加密推理时会这样返回，
    // 公开文本仍通过 reasoning_text 事件单独下发。只有出现非字符串、非 null 的
    // 形状才是真正的协议错误。
    let data = match item.get("encrypted_content") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) if value.is_empty() => None,
        Some(Value::String(value)) => Some(value.as_str()),
        Some(_) => {
            return Err(invalid_response(
                "Responses reasoning encrypted_content was not a string",
            ));
        }
    };
    let Some(data) = data else {
        // 没有不透明负载时仍要求这条 reasoning 至少带公开文本：一个
        // `item.done` 既无不透明数据也无公开文本，说明这一轮丢了内容，
        // 不能静默跳过（原合同保持不变）。
        let has_public_text = ["summary", "content"].into_iter().any(|field| {
            item.get(field)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .any(|part| {
                    part.get("text")
                        .and_then(Value::as_str)
                        .is_some_and(|text| !text.is_empty())
                })
        });
        if !has_public_text {
            return Err(invalid_response(
                "Responses reasoning omitted both opaque data and public text",
            ));
        }
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
    if parsed.output.is_empty() {
        return Err(invalid_response(
            "Responses terminal event completed with empty output",
        ));
    }
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

    #[test]
    fn reported_usage_observation_keeps_source_absence_and_reported_zero() -> Result<()> {
        let client = ResponsesClient::new(LlmConfig::for_provider(
            "usage-fixture",
            "https://example.invalid/v1/responses",
            "fixture-key",
            "fixture-model",
            echo_core::llm::LlmApiProtocol::Responses,
        )?)?;
        let mut observation: echo_core::llm::types::ProviderResponseMetadata =
            serde_json::from_value(serde_json::json!({
                "protocol":"responses", "provider":"usage-fixture", "request_id":"native-original",
                "usage":{"input_tokens":0,"output_tokens":0,"total_tokens":0}
            }))?;
        let original = serde_json::to_value(&observation)?;
        assert_eq!(
            client
                .reported_usage(&observation, None)?
                .map(|usage| usage.effective_total_tokens()),
            Some(0)
        );
        assert_eq!(
            serde_json::to_value(&observation)?,
            original,
            "不改raw/native观察"
        );
        observation.provider = "foreign-provider".into();
        assert!(client.reported_usage(&observation, None)?.is_none());
        observation.provider = "usage-fixture".into();
        observation.usage = None;
        assert!(client.reported_usage(&observation, None)?.is_none());
        observation.usage = Some(serde_json::json!({}));
        assert!(client.reported_usage(&observation, None)?.is_none());
        observation.usage = Some(serde_json::Value::Null);
        assert!(client.reported_usage(&observation, None)?.is_none());
        Ok(())
    }

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

    #[test]
    fn responses_protocol_facts_do_not_follow_the_configured_provider_label()
    -> std::result::Result<(), String> {
        let custom = ResponsesClient::new(test_config()).map_err(|error| error.to_string())?;
        assert_eq!(
            custom.protocol_capabilities(),
            ProviderCapabilityOverride::openai_responses_protocol()
        );
        assert!(!custom.capabilities().structured_output);
        assert!(!custom.capabilities().tool_support);

        let mut openai_config = test_config();
        openai_config.provider_name = Some("openai".to_string());
        let openai = ResponsesClient::new(openai_config).map_err(|error| error.to_string())?;
        assert_eq!(
            openai.protocol_capabilities(),
            custom.protocol_capabilities()
        );
        assert!(openai.capabilities().structured_output);
        assert!(openai.capabilities().tool_support);
        Ok(())
    }

    fn is_invalid_response<T>(result: &Result<T>) -> bool {
        matches!(
            result,
            Err(echo_core::error::ReactError::Llm(error))
                if matches!(error.as_ref(), LlmError::InvalidResponse(_))
        )
    }

    fn is_incomplete_response<T>(result: &Result<T>) -> bool {
        matches!(result, Err(echo_core::error::ReactError::Llm(error)) if matches!(error.as_ref(), LlmError::IncompleteResponse { .. }))
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
            provider_source: None,
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
                    strict: None,
                },
            }]),
            response_format: Some(ResponseFormat::json_schema(
                "answer",
                json!({"type": "object"}),
            )),
            user_id: Some("cache-key".to_string()),
            ..Default::default()
        };
        let body = client.request_body(&request, false)?;
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
                {"type":"function_call","id":"fc_1","call_id":"call_1","name":"lookup","arguments":"{}","status":"completed"}
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
    fn nonstream_preserves_multiple_output_identity_order_and_phase() -> Result<()> {
        let raw: Value = serde_json::from_str(include_str!(
            "../../fixtures/providers/responses/multi-output.json"
        ))
        .map_err(|error| invalid_response(format!("fixture JSON was invalid: {error}")))?;
        let response = response_to_chat(raw)?;
        assert_eq!(
            response.content().as_deref(),
            Some("先检查工具。工具结果已读取。")
        );
        assert_eq!(response.finish_reason.as_deref(), Some("tool_calls"));
        assert_eq!(
            response.message.reasoning_content.as_deref(),
            Some("provider summary")
        );
        assert!(
            !response
                .message
                .reasoning_content
                .as_deref()
                .is_some_and(|reasoning| reasoning.contains("先检查工具"))
        );

        let blocks = response
            .message
            .assistant_blocks
            .as_ref()
            .ok_or_else(|| invalid_response("canonical assistant blocks were omitted"))?;
        let blocks = blocks
            .iter()
            .map(serde_json::to_value)
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|error| invalid_response(error.to_string()))?;
        assert_eq!(blocks.len(), 4);
        assert_eq!(
            blocks
                .first()
                .and_then(|block| block.get("block_id"))
                .and_then(Value::as_str),
            Some("msg_commentary:content:0")
        );
        assert_eq!(
            blocks
                .first()
                .and_then(|block| block.get("phase"))
                .and_then(Value::as_str),
            Some("commentary")
        );
        assert_eq!(
            blocks
                .get(1)
                .and_then(|block| block.get("phase"))
                .and_then(Value::as_str),
            Some("unknown")
        );
        assert_eq!(
            blocks
                .get(2)
                .and_then(|block| block.get("phase"))
                .and_then(Value::as_str),
            Some("reasoning")
        );
        assert_eq!(
            blocks
                .get(2)
                .and_then(|block| block.get("complete"))
                .and_then(Value::as_bool),
            Some(true)
        );
        Ok(())
    }

    #[test]
    fn nonstream_empty_completed_output_is_not_success() {
        let result = response_to_chat(json!({
            "id": "resp_empty",
            "status": "completed",
            "output": []
        }));
        assert!(is_invalid_response(&result));
    }

    #[test]
    fn stream_mixed_output_keeps_text_tool_and_reasoning_channels_distinct() -> Result<()> {
        let mut adapter = ResponsesStreamAdapter::default();
        let text = required_chunk(
            adapter.map_event(json!({
                "type":"response.output_text.delta",
                "item_id":"msg_1",
                "output_index":0,
                "content_index":0,
                "delta":"progress"
            })),
            "mixed text delta",
        )?;
        assert_eq!(text.delta.content.as_deref(), Some("progress"));
        assert!(text.delta.reasoning_content.is_none());

        let tool = required_chunk(
            adapter.map_event(json!({
                "type":"response.output_item.added",
                "output_index":1,
                "item":{"id":"fc_1","type":"function_call","call_id":"call_1","name":"lookup","arguments":""}
            })),
            "mixed tool item",
        )?;
        assert!(tool.delta.content.is_none());
        assert!(tool.delta.reasoning_content.is_none());
        assert_eq!(first_tool_delta(&tool)?.id.as_deref(), Some("call_1"));

        let reasoning = required_chunk(
            adapter.map_event(json!({
                "type":"response.reasoning_text.delta",
                "item_id":"rs_1",
                "output_index":2,
                "content_index":0,
                "delta":"private"
            })),
            "mixed reasoning delta",
        )?;
        assert_eq!(
            reasoning.delta.reasoning_content.as_deref(),
            Some("private")
        );
        assert!(reasoning.delta.content.is_none());
        assert!(reasoning.delta.tool_calls.is_none());
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
            assert!(
                is_incomplete_response(&result),
                "status {status} was accepted"
            );
        }
        let missing_status = response_to_chat(json!({"id": "resp_1", "output": []}));
        assert!(is_incomplete_response(&missing_status));
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
        // 身份缺失/为空才是协议错误。
        let invalid_items = [
            json!({"type":"reasoning","encrypted_content":"encrypted"}),
            json!({"type":"reasoning","id":"","encrypted_content":"encrypted"}),
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

    /// 兼容网关（实测 token.longshine.com 的 deepseek-v4.1-flash）在 Responses
    /// 流里把推理条目写成 `encrypted_content: null`，思考文本放在 `content[].text`。
    /// 这是"没有加密推理"，不是协议错误：必须转成公开推理文本并继续这一轮。
    #[test]
    fn reasoning_without_opaque_payload_still_accepts_public_text() -> Result<()> {
        let response = response_to_chat(json!({
            "id": "resp_1",
            "status": "completed",
            "output": [
                {
                    "type": "reasoning",
                    "id": "msg_1",
                    "encrypted_content": null,
                    "summary": [],
                    "content": [{"type": "reasoning_text", "text": "先算 17*20 再加 17*3"}]
                },
                {
                    "type": "message",
                    "id": "msg_2",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": "391"}]
                }
            ]
        }))?;
        let message = response.message;
        assert_eq!(message.content.as_text().as_deref(), Some("391"));
        assert!(
            message
                .reasoning_blocks
                .as_deref()
                .unwrap_or_default()
                .is_empty(),
            "没有不透明负载时不得伪造 Opaque 回放块"
        );
        assert_eq!(
            message.reasoning_content.as_deref(),
            Some("先算 17*20 再加 17*3")
        );
        Ok(())
    }

    /// 兼容网关（实测 token.longshine.com）在 `response.output_item.added`
    /// 里省略 `arguments`，参数只在 delta 与 `item.done` 上出现。身份事件必须
    /// 接受这种形状，否则一次工具调用会让整轮失败。
    #[test]
    fn function_call_added_without_arguments_is_accepted() -> Result<()> {
        let mut adapter = ResponsesStreamAdapter::default();
        let added = required_chunk(
            adapter.map_event(json!({
                "type":"response.output_item.added",
                "output_index":1,
                "item":{
                    "id":"fc_1",
                    "type":"function_call",
                    "call_id":"call_1",
                    "name":"read_file",
                    "status":"in_progress"
                }
            })),
            "function_call added without arguments",
        )?;
        assert_eq!(first_tool_delta(&added)?.id.as_deref(), Some("call_1"));
        assert_eq!(
            tool_arguments(&added),
            None,
            "没有初始参数时不得发出空 delta"
        );

        let delta = required_chunk(
            adapter.map_event(json!({
                "type":"response.function_call_arguments.delta",
                "output_index":1,
                "item_id":"fc_1",
                "delta":"{\"path\":\"/etc/hostname\"}"
            })),
            "arguments delta",
        )?;
        assert_eq!(tool_arguments(&delta), Some("{\"path\":\"/etc/hostname\"}"));

        let done = adapter.map_event(json!({
            "type":"response.output_item.done",
            "output_index":1,
            "item":{
                "id":"fc_1",
                "type":"function_call",
                "call_id":"call_1",
                "name":"read_file",
                "status":"completed",
                "arguments":"{\"path\":\"/etc/hostname\"}"
            }
        }))?;
        assert!(done.is_none(), "终态参数已由 delta 补齐，不应重复发出增量");
        Ok(())
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
            assert!(
                is_incomplete_response(&result),
                "status {status} was accepted"
            );
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
            assert!(is_incomplete_response(&result));
        }

        let terminal = required_chunk(
            adapter.map_event(json!({
                "type":"response.completed",
                "response":{"id":"resp_1","status":"completed","output":[{"type":"message","id":"msg_1","role":"assistant","content":[{"type":"output_text","text":"done"}]}],"usage":{"input_tokens":3,"output_tokens":2,"total_tokens":5,"input_tokens_details":{"cached_tokens":0,"cache_write_tokens":0},"output_tokens_details":{"reasoning_tokens":0}}}
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
        let mut stream = adapt_responses_stream(raw_stream, "test-provider".into(), None);
        let first = stream.next().await.ok_or_else(|| {
            invalid_response("adapted stream omitted the text delta before [DONE]")
        })??;
        assert_eq!(first.delta.content.as_deref(), Some("partial"));
        let terminal = stream.next().await.ok_or_else(|| {
            invalid_response("adapted stream omitted missing-completed error at [DONE]")
        })?;
        assert!(is_incomplete_response(&terminal));
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
        let mut stream = adapt_responses_stream(raw_stream, "test-provider".into(), None);
        let first = stream.next().await.ok_or_else(|| {
            invalid_response("adapted stream omitted the text delta before EOF")
        })??;
        assert_eq!(first.delta.content.as_deref(), Some("partial"));
        let terminal = stream.next().await.ok_or_else(|| {
            invalid_response("adapted stream omitted missing-completed error at EOF")
        })?;
        assert!(is_incomplete_response(&terminal));
        assert!(stream.next().await.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn completed_then_done_emits_single_finish() -> Result<()> {
        let raw_stream = futures::stream::iter(vec![
            Ok(JsonSseEvent::Data(json!({
                "type":"response.completed",
                "response":{"id":"resp_1","status":"completed","output":[{"type":"message","id":"msg_1","role":"assistant","content":[{"type":"output_text","text":"done"}]}]}
            }))),
            Ok(JsonSseEvent::Done),
        ]);
        let items = adapt_responses_stream(raw_stream, "test-provider".into(), None)
            .collect::<Vec<_>>()
            .await;
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
            "response":{"id":"resp_1","status":"completed","output":[{"type":"message","id":"msg_1","role":"assistant","content":[{"type":"output_text","text":"done"}]}]}
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

#[cfg(test)]
mod native_boundary_tests {
    use super::*;

    #[test]
    fn provider_native_responses_replay_groups_content_under_original_message_id() -> Result<()> {
        let response = response_to_chat(
            json!({"id":"resp_multi_part","status":"completed","output":[{"type":"message","id":"msg_parts","phase":"commentary","content":[{"type":"output_text","text":"一"},{"type":"output_text","text":"二"}]}]}),
        )?;
        let input = messages_to_input(&[response.message])?;
        let items = input
            .as_array()
            .ok_or_else(|| invalid_response("input array missing"))?;
        assert_eq!(items.len(), 1);
        assert_eq!(
            items
                .first()
                .and_then(|i| i.get("id"))
                .and_then(Value::as_str),
            Some("msg_parts")
        );
        assert_eq!(
            items
                .first()
                .and_then(|i| i.get("content"))
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(2)
        );
        assert_eq!(
            items
                .first()
                .and_then(|i| i.get("phase"))
                .and_then(Value::as_str),
            Some("commentary")
        );
        Ok(())
    }

    #[test]
    fn provider_native_responses_rejects_delta_after_block_done() -> Result<()> {
        let mut adapter = ResponsesStreamAdapter::default();
        adapter.map_event(json!({"type":"response.output_text.delta","item_id":"msg_1","output_index":0,"content_index":0,"delta":"hello"}))?;
        adapter.map_event(json!({"type":"response.output_text.done","item_id":"msg_1","output_index":0,"content_index":0,"text":"hello"}))?;
        assert!(adapter.map_event(json!({"type":"response.output_text.delta","item_id":"msg_1","output_index":0,"content_index":0,"delta":"late"})).is_err());
        Ok(())
    }

    #[test]
    fn provider_native_responses_rejects_full_response_omitting_observed_tool() -> Result<()> {
        let mut adapter = ResponsesStreamAdapter::default();
        adapter.map_event(json!({"type":"response.output_item.added","output_index":1,"item":{"id":"fc_1","type":"function_call","call_id":"call_1","name":"lookup","arguments":"{}"}}))?;
        let terminal = json!({"type":"response.completed","response":{"id":"resp_1","status":"completed","output":[{"type":"message","id":"msg_1","content":[{"type":"output_text","text":"done"}]}]}});
        assert!(adapter.map_event(terminal).is_err());
        Ok(())
    }
}

#[cfg(test)]
mod native_phase_tests {
    use super::*;
    #[test]
    fn provider_native_responses_rejects_known_phase_change() -> Result<()> {
        let mut adapter = ResponsesStreamAdapter::default();
        adapter.map_event(json!({"type":"response.output_item.added","output_index":0,"item":{"id":"msg_1","type":"message","phase":"commentary","content":[]}}))?;
        let changed=adapter.map_event(json!({"type":"response.output_item.done","output_index":0,"item":{"id":"msg_1","type":"message","phase":"final_answer","content":[{"type":"output_text","text":"done"}]}}));
        assert!(changed.is_err());
        Ok(())
    }
}

#[cfg(test)]
mod plaintext_reasoning_contract_tests {
    use super::*;
    fn response() -> Value {
        json!({"id":"plain-response","status":"completed","output":[
        {"id":"plain-reason","type":"reasoning","content":[{"type":"reasoning_text","text":"公开推理🙂"}]},
        {"id":"plain-answer","type":"message","status":"completed","role":"assistant","content":[{"type":"output_text","text":"answer"}]}]})
    }
    #[test]
    fn neutral_content_parts_still_reject_changed_and_reused_ids() -> Result<()> {
        let mut adapter = ResponsesStreamAdapter::default();
        adapter.map_native_event(json!({"type":"response.content_part.added","output_index":0,"item_id":"original","content_index":0,"part":{"type":"reasoning_text","text":""}}))?;
        assert!(adapter.map_native_event(json!({"type":"response.content_part.added","output_index":0,"item_id":"changed","content_index":0,"part":{"type":"reasoning_text","text":""}})).is_err());
        assert!(adapter.map_native_event(json!({"type":"response.content_part.added","output_index":1,"item_id":"original","content_index":0,"part":{"type":"reasoning_text","text":""}})).is_err());
        adapter.map_native_event(json!({"type":"response.output_item.added","output_index":0,"item":{"id":"original","type":"reasoning","content":[]}}))?;
        assert!(adapter.map_native_event(json!({"type":"response.output_item.done","output_index":0,"item":{"id":"original","type":"message","role":"assistant","content":[]}})).is_err());
        assert_eq!(
            adapter
                .observed_items
                .get(&0)
                .and_then(|item| item.1.as_deref()),
            Some("reasoning")
        );
        Ok(())
    }
    #[test]
    fn reasoning_content_part_events_keep_the_output_item_type() -> Result<()> {
        let full = response();
        let mut adapter = ResponsesStreamAdapter::default();
        let mut chunks = Vec::new();
        for event in [
            json!({"type":"response.created","response":{"id":"plain-response"}}),
            json!({"type":"response.output_item.added","output_index":0,"item":{"id":"plain-reason","type":"reasoning","content":[]}}),
            json!({"type":"response.content_part.added","output_index":0,"item_id":"plain-reason","content_index":0,"part":{"type":"reasoning_text","text":""}}),
            json!({"type":"response.reasoning_text.delta","output_index":0,"item_id":"plain-reason","content_index":0,"delta":"公开推理🙂"}),
            json!({"type":"response.content_part.done","output_index":0,"item_id":"plain-reason","content_index":0,"part":{"type":"reasoning_text","text":"公开推理🙂"}}),
            json!({"type":"response.output_item.done","output_index":0,"item":full.pointer("/output/0").cloned().ok_or_else(||invalid_response("missing reasoning fixture"))?}),
            json!({"type":"response.output_item.added","output_index":1,"item":{"id":"plain-answer","type":"message","role":"assistant","content":[]}}),
            json!({"type":"response.content_part.added","output_index":1,"item_id":"plain-answer","content_index":0,"part":{"type":"output_text","text":""}}),
            json!({"type":"response.output_text.delta","output_index":1,"item_id":"plain-answer","content_index":0,"delta":"answer"}),
            json!({"type":"response.output_item.done","output_index":1,"item":full.pointer("/output/1").cloned().ok_or_else(||invalid_response("missing answer fixture"))?}),
            json!({"type":"response.completed","response":full}),
        ] {
            if let Some(chunk) = adapter.map_event(event)? {
                chunks.push(chunk);
            }
        }
        let reasoning = chunks
            .iter()
            .flat_map(|c| c.delta.assistant_blocks.iter().flatten())
            .filter(|b| b.phase == AssistantPhase::Reasoning)
            .map(|b| b.delta.as_str())
            .collect::<String>();
        assert_eq!(reasoning, "公开推理🙂");
        assert_eq!(
            chunks
                .iter()
                .filter_map(|c| c.delta.content.as_deref())
                .collect::<String>(),
            "answer"
        );
        assert_eq!(
            chunks.iter().filter(|c| c.finish_reason.is_some()).count(),
            1
        );
        adapter.validate_transport_end("EOF")?;
        Ok(())
    }
    #[test]
    fn plaintext_reasoning_nonstream_preserves_original_content_identity_without_opaque()
    -> Result<()> {
        let original = response();
        let converted = response_to_chat(original.clone())?;
        assert_eq!(
            converted.message.reasoning_content.as_deref(),
            Some("公开推理🙂")
        );
        assert!(converted.message.reasoning_blocks.is_none());
        let blocks = converted
            .message
            .assistant_blocks
            .as_deref()
            .ok_or_else(|| invalid_response("missing native blocks"))?;
        assert!(
            blocks
                .iter()
                .any(|block| block.block_id == "plain-reason:content:0"
                    && block.request_id == "plain-response"
                    && block.phase == AssistantPhase::Reasoning
                    && block.delta == "公开推理🙂"
                    && block.reasoning.is_none())
        );
        assert_eq!(converted.raw.extra, Some(original));
        Ok(())
    }
    #[test]
    fn plaintext_reasoning_delta_item_done_and_response_complete_do_not_duplicate() -> Result<()> {
        let full = response();
        let mut adapter = ResponsesStreamAdapter::default();
        let mut chunks = Vec::new();
        for event in [
            json!({"type":"response.created","response":{"id":"plain-response"}}),
            json!({"type":"response.reasoning_text.delta","item_id":"plain-reason","output_index":0,"content_index":0,"delta":"公开"}),
            json!({"type":"response.output_item.done","output_index":0,"item":full.pointer("/output/0").cloned().ok_or_else(||invalid_response("missing fixture item"))?}),
            json!({"type":"response.completed","response":full}),
        ] {
            if let Some(chunk) = adapter.map_event(event)? {
                chunks.push(chunk);
            }
        }
        let text: String = chunks
            .iter()
            .flat_map(|chunk| chunk.delta.assistant_blocks.iter().flatten())
            .filter(|block| block.block_id == "plain-reason:content:0")
            .map(|block| block.delta.as_str())
            .collect();
        assert_eq!(text, "公开推理🙂");
        assert_eq!(
            chunks
                .iter()
                .filter(|chunk| chunk.finish_reason.is_some())
                .count(),
            1
        );
        assert!(
            chunks
                .iter()
                .all(|chunk| chunk.delta.reasoning_blocks.is_none())
        );
        adapter.validate_transport_end("EOF")?;
        Ok(())
    }
    #[test]
    fn plaintext_reasoning_terminal_must_not_override_observed_content() -> Result<()> {
        let mut adapter = ResponsesStreamAdapter::default();
        adapter.map_event(json!({"type":"response.reasoning_text.delta","item_id":"plain-reason","output_index":0,"content_index":0,"delta":"原始"}))?;
        let item = json!({"id":"plain-reason","type":"reasoning","content":[{"type":"reasoning_text","text":"冲突"}]});
        assert!(
            adapter
                .map_event(json!({"type":"response.output_item.done","output_index":0,"item":item}))
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn plaintext_reasoning_full_terminal_and_prefix_have_both_public_fields_in_parity() -> Result<()>
    {
        for prefix in [None, Some("公开")] {
            let mut adapter = ResponsesStreamAdapter::default();
            let mut chunks = Vec::new();
            adapter
                .map_event(json!({"type":"response.created","response":{"id":"plain-response"}}))?;
            if let Some(prefix) = prefix
                && let Some(chunk)=adapter.map_event(json!({"type":"response.reasoning_text.delta","item_id":"plain-reason","output_index":0,"content_index":0,"delta":prefix}))? {
                chunks.push(chunk);
            }
            if let Some(chunk) =
                adapter.map_event(json!({"type":"response.completed","response":response()}))?
            {
                chunks.push(chunk);
            }
            let compat: String = chunks
                .iter()
                .filter_map(|chunk| chunk.delta.reasoning_content.as_deref())
                .collect();
            let canonical: String = chunks
                .iter()
                .flat_map(|chunk| chunk.delta.assistant_blocks.iter().flatten())
                .filter(|block| block.phase == AssistantPhase::Reasoning)
                .map(|block| block.delta.as_str())
                .collect();
            assert_eq!(compat, "公开推理🙂");
            assert_eq!(canonical, compat);
        }
        Ok(())
    }

    #[test]
    fn plaintext_reasoning_original_items_roundtrip_to_next_request_in_stream_and_nonstream()
    -> Result<()> {
        let full = response();
        let response = response_to_chat(full.clone())?;
        let restored: Message = serde_json::from_value(serde_json::to_value(response.message)?)?;
        let input = messages_to_input(&[restored])?;
        assert_eq!(
            input.pointer("/0/id").and_then(Value::as_str),
            Some("plain-reason")
        );
        assert_eq!(
            input.pointer("/0/type").and_then(Value::as_str),
            Some("reasoning")
        );
        assert_eq!(
            input.pointer("/0/content/0/type").and_then(Value::as_str),
            Some("reasoning_text")
        );
        assert_eq!(
            input.pointer("/0/content/0/text").and_then(Value::as_str),
            Some("公开推理🙂")
        );
        assert!(input.pointer("/0/encrypted_content").is_none());
        assert_eq!(
            input.pointer("/1/id").and_then(Value::as_str),
            Some("plain-answer")
        );
        let mut adapter = ResponsesStreamAdapter::default();
        let mut blocks = Vec::new();
        for event in [
            json!({"type":"response.created","response":{"id":"plain-response"}}),
            json!({"type":"response.reasoning_text.delta","item_id":"plain-reason","output_index":0,"content_index":0,"delta":"公开"}),
            json!({"type":"response.completed","response":full}),
        ] {
            if let Some(chunk) = adapter.map_event(event)? {
                blocks.extend(chunk.delta.assistant_blocks.unwrap_or_default());
            }
        }
        let mut accumulated = Message::assistant("answer".to_string());
        accumulated.assistant_blocks = Some(blocks);
        let restored: Message = serde_json::from_value(serde_json::to_value(accumulated)?)?;
        let replay = messages_to_input(&[restored])?;
        assert_eq!(
            replay, input,
            "流/非流都必须保同一原item/content类型、文字与顺序"
        );
        Ok(())
    }

    #[test]
    fn terminal_can_supply_original_encrypted_payload_after_public_reasoning_item() -> Result<()> {
        let mut adapter = ResponsesStreamAdapter::default();
        adapter.map_event(json!({"type":"response.created","response":{"id":"plain-response"}}))?;
        let mut full = response();
        adapter.map_event(json!({"type":"response.output_item.done","output_index":0,"item":full.pointer("/output/0").cloned().ok_or_else(||invalid_response("missing fixture item"))?}))?;
        full.pointer_mut("/output/0")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| invalid_response("missing reasoning object"))?
            .insert("encrypted_content".into(), json!("original-encrypted"));
        let terminal = adapter
            .map_event(json!({"type":"response.completed","response":full}))?
            .ok_or_else(|| invalid_response("missing terminal"))?;
        assert!(terminal.delta.reasoning_blocks.as_deref().unwrap_or_default().iter().any(|block|matches!(block,ReasoningBlock::Opaque {data,..}if data=="original-encrypted")));
        Ok(())
    }
}

#[cfg(test)]
mod mixed_reasoning_replay_contract_tests {
    use super::*;

    fn original_input() -> Value {
        json!([
            {"type":"message","id":"before","role":"assistant","phase":"commentary",
             "content":[{"type":"output_text","text":"before","annotations":[]}]},
            {"type":"reasoning","id":"mixed-reasoning","encrypted_content":"sealed-original",
             "summary":[{"type":"summary_text","text":"同文🙂"},{"type":"summary_text","text":"摘要二"}],
             "content":[{"type":"reasoning_text","text":"同文🙂"},{"type":"reasoning_text","text":"公开二"}]},
            {"type":"message","id":"after","role":"assistant","phase":"final_answer",
             "content":[{"type":"output_text","text":"after","annotations":[]}]}
        ])
    }

    fn full_response() -> Value {
        json!({"id":"mixed-response","status":"completed","output":original_input()})
    }

    fn next_request(message: Message, stream: bool) -> Result<Value> {
        // 同一Message先经持久化格式往返，再走真实request_body；不调用HTTP。
        let restored = serde_json::from_value(serde_json::to_value(message)?)?;
        let client = ResponsesClient::new(LlmConfig {
            provider_name: Some("test-provider".into()),
            api_protocol: LlmApiProtocol::Responses,
            base_url: "https://api.example.test/v1/responses".into(),
            api_key: "fixture-key".into(),
            model: "gpt-test".into(),
            input_modalities: echo_core::llm::ModelInputModality::all_supported(),
            thinking_protocol: echo_core::llm::ThinkingProtocol::OpenaiReasoningEffort,
            timeouts: echo_core::llm::LlmTimeouts::default(),
        })?;
        client.request_body(
            &ChatRequest {
                messages: vec![restored, Message::user("继续".into())],
                ..Default::default()
            },
            stream,
        )
    }

    fn assert_next_request(message: Message) -> Result<()> {
        let mut expected = original_input();
        expected
            .as_array_mut()
            .ok_or_else(|| invalid_response("fixture input must be an array"))?
            .push(json!({"type":"message","role":"user","content":[{"type":"input_text","text":"继续"}]}));
        for stream in [false, true] {
            let body = next_request(message.clone(), stream)?;
            assert_eq!(body.get("input"), Some(&expected));
            assert_eq!(body.get("stream").and_then(Value::as_bool), Some(stream));
        }
        Ok(())
    }

    fn streamed_message(item_done: Option<Value>, prefix: bool) -> Result<Message> {
        let mut events =
            vec![json!({"type":"response.created","response":{"id":"mixed-response"}})];
        if prefix {
            events.extend([
                json!({"type":"response.reasoning_summary_text.delta","item_id":"mixed-reasoning","output_index":1,"summary_index":0,"delta":"同文"}),
                // 不同content索引可以交错到达；下一请求必须按原坐标排回0、1。
                json!({"type":"response.reasoning_text.delta","item_id":"mixed-reasoning","output_index":1,"content_index":1,"delta":"公开"}),
                json!({"type":"response.reasoning_text.delta","item_id":"mixed-reasoning","output_index":1,"content_index":0,"delta":"同文"}),
            ]);
        }
        if let Some(item) = item_done {
            events.push(json!({"type":"response.output_item.done","output_index":1,"item":item}));
        }
        events.push(json!({"type":"response.completed","response":full_response()}));
        let mut adapter = ResponsesStreamAdapter::default();
        let mut blocks = Vec::new();
        let mut terminals = 0usize;
        for event in events {
            if let Some(chunk) = adapter.map_event(event)? {
                terminals = terminals.saturating_add(usize::from(chunk.finish_reason.is_some()));
                blocks.extend(chunk.delta.assistant_blocks.unwrap_or_default());
            }
        }
        adapter.validate_transport_end("EOF")?;
        assert_eq!(terminals, 1);
        assert_eq!(
            blocks
                .iter()
                .filter(|block| matches!(block.reasoning, Some(ReasoningBlock::Opaque { .. })))
                .count(),
            1,
        );
        // 累积原canonical块，而非用公开正文重新合成provider原件。
        let mut message = Message::assistant("beforeafter".into());
        message.assistant_blocks = Some(blocks);
        Ok(message)
    }

    #[test]
    fn mixed_reasoning_nonstream_next_request_keeps_encrypted_summary_and_content_in_order()
    -> Result<()> {
        let message = response_to_chat(full_response())?.message;
        assert_next_request(message.clone())?;
        let mut reversed = message;
        reversed
            .assistant_blocks
            .as_mut()
            .ok_or_else(|| invalid_response("fixture native blocks missing"))?
            .reverse();
        // 同源Opaque可以先于或晚于明文入表；不能覆盖content，也不能误判重复item。
        assert_next_request(reversed)
    }

    #[test]
    fn mixed_reasoning_stream_next_request_deduplicates_only_carried_summary() -> Result<()> {
        let original = full_response();
        let item = original
            .pointer("/output/1")
            .cloned()
            .ok_or_else(|| invalid_response("fixture reasoning item missing"))?;
        for prefix in [false, true] {
            assert_next_request(streamed_message(Some(item.clone()), prefix)?)?;
            assert_next_request(streamed_message(None, prefix)?)?;
        }
        Ok(())
    }

    #[test]
    fn late_encrypted_after_plaintext_item_done_survives_persistence_and_next_request() -> Result<()>
    {
        let mut original = full_response();
        let item = original
            .pointer_mut("/output/1")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| invalid_response("fixture reasoning item missing"))?;
        item.remove("encrypted_content");
        let public_item = Value::Object(item.clone());
        for prefix in [false, true] {
            assert_next_request(streamed_message(Some(public_item.clone()), prefix)?)?;
        }
        Ok(())
    }

    #[test]
    fn summary_not_carried_by_opaque_remains_once_with_original_content() -> Result<()> {
        for carried in [0, 1] {
            let mut message = response_to_chat(full_response())?.message;
            let opaque = message
                .assistant_blocks
                .as_mut()
                .and_then(|blocks| {
                    blocks
                        .iter_mut()
                        .find_map(|block| match &mut block.reasoning {
                            Some(ReasoningBlock::Opaque { summary, .. }) => Some(summary),
                            _ => None,
                        })
                })
                .ok_or_else(|| invalid_response("fixture opaque block missing"))?;
            opaque.truncate(carried);
            assert_next_request(message)?;
        }
        Ok(())
    }

    #[test]
    fn mixed_reasoning_replay_rejects_item_owner_and_summary_conflicts() -> Result<()> {
        for conflict in ["item", "owner", "summary"] {
            let mut message = response_to_chat(full_response())?.message;
            let target = if conflict == "summary" {
                "mixed-reasoning:summary:0"
            } else {
                "mixed-reasoning:content:0"
            };
            let block = message
                .assistant_blocks
                .as_mut()
                .and_then(|blocks| blocks.iter_mut().find(|block| block.block_id == target))
                .ok_or_else(|| invalid_response("fixture plaintext block missing"))?;
            if conflict == "summary" {
                block.delta = "冲突摘要".into();
            } else {
                let source = block
                    .provider_source
                    .as_mut()
                    .ok_or_else(|| invalid_response("fixture provider source missing"))?;
                if conflict == "item" {
                    source.item_id = "different-item".into();
                } else {
                    source.request_id = "different-request".into();
                    block.request_id = "different-request".into();
                }
            }
            assert!(
                next_request(message, false).is_err(),
                "冲突原件不得被Opaque去重吞掉：{conflict}"
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod responses_compatibility_legacy_api_tests {
    use super::*;
    use echo_core::llm::types::ImageUrl;

    #[test]
    fn direct_tool_parts_survive_message_serde_without_text_flattening() -> Result<()> {
        let mut tool = Message::tool_result("call_1".into(), "inspect".into(), "".into());
        tool.content = MessageContent::Parts(vec![
            ContentPart::Text {
                text: "截图🙂".into(),
            },
            ContentPart::ImageUrl {
                image_url: ImageUrl {
                    url: "data:image/png;base64,AAAA".into(),
                    detail: Some("low".into()),
                },
            },
            ContentPart::File {
                name: "报告.pdf".into(),
                content: "JVBERi0=".into(),
            },
        ]);
        let restored: Message = serde_json::from_value(serde_json::to_value(tool)?)?;
        let input = messages_to_input(&[restored])?;
        assert_eq!(
            input.pointer("/0/call_id").and_then(Value::as_str),
            Some("call_1")
        );
        let output = input
            .pointer("/0/output")
            .and_then(Value::as_array)
            .ok_or_else(|| invalid_response("tool Parts must remain an array"))?;
        assert_eq!(
            output
                .iter()
                .filter_map(|part| part.get("type").and_then(Value::as_str))
                .collect::<Vec<_>>(),
            vec!["input_text", "input_image", "input_file"]
        );
        assert_eq!(
            input
                .pointer("/0/output/1/image_url")
                .and_then(Value::as_str),
            Some("data:image/png;base64,AAAA")
        );
        assert_eq!(
            input
                .pointer("/0/output/2/file_data")
                .and_then(Value::as_str),
            Some("data:application/pdf;base64,JVBERi0=")
        );
        Ok(())
    }

    #[test]
    fn orphan_tool_result_and_unknown_role_are_rejected_before_wire() {
        let orphan = Message {
            role: Role::Tool,
            content: MessageContent::Text("result".into()),
            ..Default::default()
        };
        assert!(messages_to_input(&[orphan]).is_err());
        let custom = Message {
            role: Role::Custom("unknown-role".into()),
            content: MessageContent::Text("text".into()),
            ..Default::default()
        };
        assert!(messages_to_input(&[custom]).is_err());
    }

    #[test]
    fn early_final_text_does_not_drop_later_tool_or_unknown_phase() -> Result<()> {
        let full = json!({"id":"interleaved","status":"completed","output":[
            {"type":"message","id":"early","phase":"final_answer","role":"assistant","content":[{"type":"output_text","text":"早候选","annotations":[]}]},
            {"type":"function_call","id":"fc_late","call_id":"late_call","name":"inspect","arguments":"{}","status":"completed"},
            {"type":"message","id":"last","phase":"future-phase","role":"assistant","content":[{"type":"refusal","refusal":"仍为原种类"}]}
        ]});
        let response = response_to_chat(full.clone())?;
        assert_eq!(response.finish_reason.as_deref(), Some("tool_calls"));
        let mut adapter = ResponsesStreamAdapter::default();
        let chunk = adapter
            .map_event(json!({"type":"response.completed","response":full}))?
            .ok_or_else(|| invalid_response("missing terminal"))?;
        assert_eq!(chunk.finish_reason.as_deref(), Some("tool_calls"));
        assert_eq!(
            messages_to_input(&[response.message])?
                .pointer("/2/phase")
                .and_then(Value::as_str),
            Some("future-phase")
        );
        Ok(())
    }
}

#[cfg(test)]
mod responses_compatibility_contract_tests {
    use super::*;
    use echo_core::llm::types::{FunctionSpec, ImageUrl};

    fn config(provider: &str, model: &str) -> LlmConfig {
        LlmConfig {
            provider_name: Some(provider.into()),
            model: model.into(),
            api_protocol: LlmApiProtocol::Responses,
            base_url: "https://api.example.test/v1/responses".into(),
            api_key: "fixture-key".into(),
            input_modalities: echo_core::llm::ModelInputModality::all_supported(),
            thinking_protocol: ThinkingProtocol::OpenaiReasoningEffort,
            timeouts: echo_core::llm::LlmTimeouts::default(),
        }
    }

    fn tool(strict: Option<bool>) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".into(),
            function: FunctionSpec {
                name: "inspect".into(),
                description: "Inspect".into(),
                parameters: json!({"type":"object","properties":{},"additionalProperties":false}),
                strict,
            },
        }
    }

    #[test]
    fn typed_choice_and_strict_are_separate_from_final_schema() -> Result<()> {
        let client = ResponsesClient::new(config("openai", "gpt-test"))?;
        for strict in [None, Some(false), Some(true)] {
            for choice in [
                ToolChoice::Auto,
                ToolChoice::None,
                ToolChoice::Required,
                ToolChoice::function("inspect"),
            ] {
                for stream in [false, true] {
                    let body = client.request_body(
                        &ChatRequest {
                            tools: Some(vec![tool(strict)]),
                            tool_choice: Some(choice.clone()),
                            response_format: Some(ResponseFormat::json_schema(
                                "answer",
                                json!({"type":"object"}),
                            )),
                            max_tokens: Some(512),
                            ..Default::default()
                        },
                        stream,
                    )?;
                    assert_eq!(
                        body.pointer("/tools/0/strict").and_then(Value::as_bool),
                        Some(strict.unwrap_or(false))
                    );
                    assert_eq!(
                        body.pointer("/text/format/strict").and_then(Value::as_bool),
                        Some(true)
                    );
                    let expected = match choice {
                        ToolChoice::Function { .. } => json!({"type":"function","name":"inspect"}),
                        _ => choice.to_openai_value(),
                    };
                    assert_eq!(body.get("tool_choice"), Some(&expected));
                    assert_eq!(body.get("max_output_tokens"), Some(&json!(512)));
                }
            }
        }
        Ok(())
    }

    #[test]
    fn explicit_invalid_tool_choices_fail_but_none_without_tools_is_valid() -> Result<()> {
        let client = ResponsesClient::new(config("openai", "gpt-test"))?;
        for choice in [
            ToolChoice::Required,
            ToolChoice::function(""),
            ToolChoice::function("missing"),
        ] {
            assert!(
                client
                    .request_body(
                        &ChatRequest {
                            tool_choice: Some(choice),
                            ..Default::default()
                        },
                        false
                    )
                    .is_err()
            );
        }
        assert_eq!(
            client
                .request_body(
                    &ChatRequest {
                        tool_choice: Some(ToolChoice::None),
                        ..Default::default()
                    },
                    false
                )?
                .get("tool_choice"),
            Some(&json!("none"))
        );
        Ok(())
    }

    #[test]
    fn deepseek_dialect_keeps_native_parts_and_rejects_unsupported_combinations() -> Result<()> {
        let flash = ResponsesClient::new(config("deepseek", "deepseek-flash"))?;
        let pro = ResponsesClient::new(config("deepseek", "deepseek-v4-pro"))?;
        let image = Message::user_with_image_url("图片", "https://example.test/a.png");
        let mut result = Message::tool_result("call".into(), "inspect".into(), "".into());
        result.content = image.content.clone();
        for message in [image.clone(), result.clone()] {
            let body = flash.request_body(&ChatRequest::new(vec![message.clone()]), false)?;
            assert!(body.get("include").is_none());
            assert!(
                pro.request_body(&ChatRequest::new(vec![message]), false)
                    .is_err()
            );
        }
        for role in [Role::User, Role::Tool] {
            let message = Message {
                role,
                tool_call_id: Some("call".into()),
                content: MessageContent::Parts(vec![ContentPart::File {
                    name: "report.pdf".into(),
                    content: "JVBERi0=".into(),
                }]),
                ..Default::default()
            };
            assert!(
                flash
                    .request_body(&ChatRequest::new(vec![message]), false)
                    .is_err()
            );
        }
        assert!(
            flash
                .request_body(
                    &ChatRequest {
                        tools: Some(vec![tool(Some(true))]),
                        ..Default::default()
                    },
                    false
                )
                .is_err()
        );
        let mut system_image = image;
        system_image.role = Role::System;
        assert!(
            flash
                .request_body(&ChatRequest::new(vec![system_image]), false)
                .is_err()
        );
        assert!(
            ResponsesClient::new(config("deepseek", "unknown"))?
                .request_body(&ChatRequest::default(), false)
                .is_err()
        );
        let body = flash.request_body(
            &ChatRequest {
                user_id: Some("original-user".into()),
                response_format: Some(ResponseFormat::JsonObject),
                ..Default::default()
            },
            false,
        )?;
        assert_eq!(body.get("user"), Some(&json!("original-user")));
        assert!(body.get("prompt_cache_key").is_none());
        assert_eq!(
            body.pointer("/text/format/type"),
            Some(&json!("json_object"))
        );
        Ok(())
    }

    #[test]
    fn explicit_thinking_temperature_and_budget_never_silently_drop() -> Result<()> {
        let flash = ResponsesClient::new(config("deepseek", "deepseek-flash"))?;
        for thinking in [
            ThinkingConfig::Level(ThinkingLevel::Xhigh),
            ThinkingConfig::BudgetTokens(1000),
        ] {
            assert!(
                flash
                    .request_body(
                        &ChatRequest {
                            thinking: Some(thinking),
                            ..Default::default()
                        },
                        false
                    )
                    .is_err()
            );
        }
        assert!(
            flash
                .request_body(
                    &ChatRequest {
                        temperature: Some(0.5),
                        ..Default::default()
                    },
                    false
                )
                .is_err()
        );
        let body = flash.request_body(
            &ChatRequest {
                thinking: Some(ThinkingConfig::Disabled),
                temperature: Some(0.5),
                ..Default::default()
            },
            false,
        )?;
        assert_eq!(body.pointer("/reasoning/effort"), Some(&json!("none")));
        assert_eq!(body.get("temperature"), Some(&json!(0.5)));
        assert!(body.pointer("/reasoning/summary").is_none());
        let body = flash.request_body(
            &ChatRequest {
                thinking: Some(ThinkingConfig::Level(ThinkingLevel::Medium)),
                ..Default::default()
            },
            false,
        )?;
        assert_eq!(
            body.pointer("/reasoning/effort"),
            Some(&json!("medium")),
            "不继承Chat clamp到high"
        );
        Ok(())
    }

    #[test]
    fn canonical_error_observation_survives_storage_without_vendor_pseudo_field() -> Result<()> {
        let message = Message::tool_result("call".into(), "inspect".into(), "error body".into())
            .with_tool_result_is_error(true);
        let restored: Message = serde_json::from_value(serde_json::to_value(message)?)?;
        assert_eq!(restored.tool_result_is_error, Some(true));
        let native = messages_to_input(&[restored])?;
        assert_eq!(native.pointer("/0/output"), Some(&json!("error body")));
        assert!(native.pointer("/0/tool_result_is_error").is_none());
        assert!(native.pointer("/0/is_error").is_none());
        Ok(())
    }

    #[test]
    fn malformed_media_and_default_text_are_explicit() -> Result<()> {
        let client = ResponsesClient::new(config("openai", "gpt-test"))?;
        let body = client.request_body(
            &ChatRequest {
                response_format: Some(ResponseFormat::Text),
                temperature: Some(0.25),
                ..Default::default()
            },
            false,
        )?;
        assert!(body.get("text").is_none());
        assert_eq!(body.get("temperature"), Some(&json!(0.25)));
        let message = Message::user_multimodal(vec![ContentPart::ImageUrl {
            image_url: ImageUrl {
                url: "https://example.test/a".into(),
                detail: Some("invented".into()),
            },
        }]);
        assert!(
            client
                .request_body(&ChatRequest::new(vec![message]), false)
                .is_err()
        );
        let message = Message::user_multimodal(vec![ContentPart::File {
            name: "a.pdf".into(),
            content: "%%%".into(),
        }]);
        assert!(
            client
                .request_body(&ChatRequest::new(vec![message]), false)
                .is_err()
        );
        Ok(())
    }
}

#[cfg(test)]
mod responses_compatibility_http_tests {
    use super::*;
    use echo_core::llm::types::ImageUrl;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn implicit_temperature_simple_chat_wire(provider: &str, model: &str) -> Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            let accepted = tokio::select! {
                accepted = listener.accept() => Some(accepted),
                _ = stopped => None,
            };
            let Some(accepted) = accepted else {
                return Ok::<_, echo_core::error::ReactError>(None);
            };
            let (mut socket, _) = accepted?;
            let body = tokio::time::timeout(Duration::from_secs(5), read_request(&mut socket))
                .await
                .map_err(|_| invalid_response("temperature fixture read timeout"))??;
            let reply = response(2).to_string();
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", reply.len()).as_bytes()).await?;
            socket.write_all(reply.as_bytes()).await?;
            socket.shutdown().await?;
            Ok(Some(body))
        });
        let client = ResponsesClient::new(LlmConfig {
            provider_name: Some(provider.into()),
            api_protocol: LlmApiProtocol::Responses,
            base_url: format!("http://{address}"),
            api_key: "owned-dummy-key".into(),
            model: model.into(),
            input_modalities: echo_core::llm::ModelInputModality::all_supported(),
            thinking_protocol: ThinkingProtocol::OpenaiReasoningEffort,
            timeouts: echo_core::llm::LlmTimeouts::default(),
        })?;
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            client.chat_simple(vec![Message::user("probe".into())]),
        )
        .await;
        let _ = stop.send(());
        // 两侧都收回后先保原请求首因，fixture读写错误不遮掉client错误。
        let captured = server.await;
        let text = result.map_err(|_| invalid_response("temperature request timeout"))??;
        let captured =
            captured.map_err(|_| invalid_response("temperature fixture join failed"))??;
        assert_eq!(text, "最终回答");
        let body = captured
            .ok_or_else(|| invalid_response("original chat_simple never sent a request"))?;
        assert!(
            body.get("temperature").is_none(),
            "default helper injected a wire sampling parameter"
        );
        assert_eq!(body.get("max_output_tokens"), Some(&json!(2048)));
        Ok(())
    }

    #[tokio::test]
    async fn implicit_temperature_default_simple_chat_deepseek_reasoning_wire() -> Result<()> {
        implicit_temperature_simple_chat_wire("deepseek", "deepseek-flash").await
    }

    #[tokio::test]
    async fn implicit_temperature_default_simple_chat_openai_wire() -> Result<()> {
        // 仅使用当前源码已识别的模型名；不声称这是用户现场的模型或外部API验收。
        implicit_temperature_simple_chat_wire("openai", "gpt-5.1").await
    }

    #[tokio::test]
    async fn implicit_temperature_explicit_simple_options_keep_provider_rejection() -> Result<()> {
        let client = ResponsesClient::new(LlmConfig {
            provider_name: Some("deepseek".into()),
            model: "deepseek-flash".into(),
            api_protocol: LlmApiProtocol::Responses,
            base_url: "http://127.0.0.1:1".into(),
            api_key: "owned-dummy-key".into(),
            input_modalities: echo_core::llm::ModelInputModality::all_supported(),
            thinking_protocol: ThinkingProtocol::OpenaiReasoningEffort,
            timeouts: echo_core::llm::LlmTimeouts::default(),
        })?;
        let error = client
            .chat_simple_with_options(
                vec![Message::user("probe".into())],
                echo_core::llm::SimpleChatOptions::default().with_temperature(0.25),
            )
            .await;
        assert!(
            matches!(error, Err(echo_core::error::ReactError::Llm(error)) if matches!(error.as_ref(), LlmError::SerializationError(message) if message == "Responses request: temperature cannot be honored with this reasoning configuration"))
        );
        Ok(())
    }

    fn response(round: usize) -> Value {
        let output = match round {
            0 => json!([
                {"id":"early","type":"message","role":"assistant","phase":"final_answer","content":[{"type":"output_text","text":"仍需工具","annotations":[]}]},
                {"id":"reason","type":"reasoning","encrypted_content":"sealed-original","summary":[{"type":"summary_text","text":"同文🙂"}],"content":[{"type":"reasoning_text","text":"同文🙂"}]},
                {"id":"fc","type":"function_call","call_id":"call","name":"inspect","arguments":"{}"}
            ]),
            1 => {
                json!([{"id":"progress","type":"message","role":"assistant","phase":"commentary","content":[{"type":"output_text","text":"已读取图片","annotations":[]}]}])
            }
            _ => {
                json!([{"id":"final","type":"message","role":"assistant","phase":"final_answer","content":[{"type":"output_text","text":"最终回答","annotations":[]}]}])
            }
        };
        json!({"id":format!("response-{round}"),"status":"completed","output":output,"usage":{"input_tokens":9,"output_tokens":4,"total_tokens":13}})
    }

    async fn read_request(socket: &mut tokio::net::TcpStream) -> Result<Value> {
        let mut bytes = Vec::new();
        loop {
            if socket.read_buf(&mut bytes).await? == 0 {
                return Err(invalid_response("HTTP fixture request truncated"));
            }
            let text = match std::str::from_utf8(&bytes) {
                Ok(text) => text,
                Err(error) if error.error_len().is_none() => continue,
                Err(error) => return Err(invalid_response(error.to_string())),
            };
            if let Some((headers, payload)) = text.split_once("\r\n\r\n") {
                let length = headers
                    .lines()
                    .find_map(|line| {
                        line.split_once(':')
                            .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                            .map(|(_, value)| value.trim().parse::<usize>())
                    })
                    .transpose()
                    .map_err(|error| invalid_response(error.to_string()))?
                    .ok_or_else(|| invalid_response("missing HTTP Content-Length"))?;
                // HTTP长度是字节数，不以此截断任何文本。
                if payload.len() >= length {
                    return serde_json::from_str(payload).map_err(Into::into);
                }
            }
        }
    }

    async fn exchange(
        client: &ResponsesClient,
        request: ChatRequest,
        stream: bool,
    ) -> Result<(Message, String)> {
        if !stream {
            let response = client.chat(request).await?;
            return Ok((response.message, response.finish_reason.unwrap_or_default()));
        }
        let mut stream = client.chat_stream(request).await?;
        let mut blocks = Vec::new();
        let mut calls = Vec::new();
        let mut text = String::new();
        let mut finish = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            if let Some(reason) = chunk.finish_reason {
                finish.push(reason);
            }
            text.push_str(chunk.delta.content.as_deref().unwrap_or_default());
            blocks.extend(chunk.delta.assistant_blocks.unwrap_or_default());
            // 本HTTP夹具仅在completed携带完整tool；断言完整性，不从预期JSON补原件。
            for call in chunk.delta.tool_calls.unwrap_or_default() {
                let function = call
                    .function
                    .ok_or_else(|| invalid_response("fixture tool function missing"))?;
                calls.push(ToolCall {
                    provider_source: call.provider_source,
                    id: call
                        .id
                        .ok_or_else(|| invalid_response("fixture tool call id missing"))?,
                    call_type: "function".into(),
                    function: FunctionCall {
                        name: function
                            .name
                            .ok_or_else(|| invalid_response("fixture tool name missing"))?,
                        arguments: function
                            .arguments
                            .ok_or_else(|| invalid_response("fixture arguments missing"))?,
                    },
                });
            }
        }
        assert_eq!(finish.len(), 1);
        let mut message = Message::assistant(text);
        message.assistant_blocks = Some(blocks);
        message.tool_calls = (!calls.is_empty()).then_some(calls);
        Ok((
            message,
            finish
                .into_iter()
                .next()
                .ok_or_else(|| invalid_response("fixture finish missing"))?,
        ))
    }

    async fn roundtrip(stream: bool) -> Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let server = tokio::spawn(async move {
            let mut requests = Vec::new();
            for round in 0..3 {
                let (mut socket, _) = listener.accept().await?;
                requests.push(read_request(&mut socket).await?);
                let raw = response(round);
                let body = if stream {
                    let mut events = vec![
                        json!({"type":"response.created","response":{"id":format!("response-{round}")}}),
                    ];
                    if round == 0 {
                        // plaintext先闭合，encrypted原件仅在completed晚到。
                        events.push(json!({"type":"response.output_item.done","output_index":1,"item":{"id":"reason","type":"reasoning","content":[{"type":"reasoning_text","text":"同文🙂"}]}}));
                    }
                    events.push(json!({"type":"response.completed","response":raw}));
                    events
                        .iter()
                        .map(|event| {
                            format!(
                                "event: {}\ndata: {event}\n\n",
                                event
                                    .get("type")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                            )
                        })
                        .collect::<String>()
                } else {
                    raw.to_string()
                };
                let content_type = if stream {
                    "text/event-stream"
                } else {
                    "application/json"
                };
                // Content-Length使用完整UTF-8 body字节数。
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).as_bytes()).await?;
                socket.write_all(body.as_bytes()).await?;
                socket.flush().await?;
            }
            Ok::<_, echo_core::error::ReactError>(requests)
        });
        let result = async {
            let client = ResponsesClient::new(LlmConfig {
                provider_name: Some("test-provider".into()),
                api_protocol: LlmApiProtocol::Responses,
                base_url: format!("http://{address}"),
                api_key: "fixture-key".into(),
                model: "gpt-test".into(),
                input_modalities: echo_core::llm::ModelInputModality::all_supported(),
                thinking_protocol: ThinkingProtocol::OpenaiReasoningEffort,
                timeouts: echo_core::llm::LlmTimeouts::default(),
            })?;
            let mut history = vec![Message::user("检查".into())];
            let (first, finish) =
                exchange(&client, ChatRequest::new(history.clone()), stream).await?;
            assert_eq!(finish, "tool_calls", "early final文字不能覆盖其后tool");
            let restored: Message = serde_json::from_value(serde_json::to_value(first)?)?;
            history.push(restored);
            let mut tool = Message::tool_result("call".into(), "inspect".into(), "".into())
                .with_tool_result_is_error(false);
            tool.content = MessageContent::Parts(vec![
                ContentPart::Text {
                    text: "截图🙂".into(),
                },
                ContentPart::ImageUrl {
                    image_url: ImageUrl {
                        url: "data:image/png;base64,AAAA".into(),
                        detail: Some("low".into()),
                    },
                },
            ]);
            history.push(serde_json::from_value(serde_json::to_value(tool)?)?);
            let (progress, finish) =
                exchange(&client, ChatRequest::new(history.clone()), stream).await?;
            assert_eq!(finish, "stop");
            assert!(
                progress
                    .assistant_blocks
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .any(|block| block.phase == AssistantPhase::Commentary)
            );
            history.push(progress);
            let (final_message, _) = exchange(&client, ChatRequest::new(history), stream).await?;
            assert_eq!(final_message.content.as_text().as_deref(), Some("最终回答"));
            Ok::<_, echo_core::error::ReactError>(())
        }
        .await;
        if let Err(error) = result {
            server.abort();
            return Err(error);
        }
        let requests = server
            .await
            .map_err(|error| invalid_response(error.to_string()))??;
        let second = requests
            .get(1)
            .ok_or_else(|| invalid_response("second request missing"))?;
        let replay = second
            .get("input")
            .and_then(Value::as_array)
            .ok_or_else(|| invalid_response("request input missing"))?;
        let original = response(0);
        let original = original
            .get("output")
            .and_then(Value::as_array)
            .ok_or_else(|| invalid_response("fixture output missing"))?;
        assert_eq!(
            replay.iter().skip(1).take(3).cloned().collect::<Vec<_>>(),
            *original
        );
        assert_eq!(
            replay
                .get(4)
                .and_then(|item| item.pointer("/output/1/type")),
            Some(&json!("input_image"))
        );
        assert!(
            replay
                .iter()
                .all(|item| item.get("tool_result_is_error").is_none()
                    && item.get("is_error").is_none()
                    && item.get("execution_status").is_none())
        );
        Ok(())
    }

    #[tokio::test]
    async fn http_dual_mode_late_opaque_tool_parts_and_progress_next_request() -> Result<()> {
        for stream in [false, true] {
            tokio::time::timeout(Duration::from_secs(10), roundtrip(stream))
                .await
                .map_err(|_| invalid_response("HTTP fixture deadline"))??;
        }
        Ok(())
    }
}

#[cfg(test)]
mod responses_metadata_legacy_api_tests {
    use super::*;

    fn full_response() -> Value {
        json!({"id":"meta-response-A","model":"model-native-未知🙂","status":"completed","incomplete_details":null,
            "output":[
                {"type":"reasoning","id":"reason-A","encrypted_content":"sealed-original","summary":[{"type":"summary_text","text":"同文🙂"}],"content":[{"type":"reasoning_text","text":"同文🙂"}]},
                {"type":"message","id":"message-A","role":"assistant","phase":"final_answer","content":[{"type":"output_text","text":"完成🙂","annotations":[]}]}
            ],
            "usage":{"input_tokens":5,"output_tokens":10,"total_tokens":15,"input_tokens_details":{"cached_tokens":2,"native_unknown":{"counter":9007199254740993_u64}},"output_tokens_details":{"reasoning_tokens":7,"native_array":[null,"未知",{"count":3}]}},
            "provider_response":{"status":"forged","request_id":"other"}})
    }

    #[test]
    fn nonstream_native_observation_reaches_message_serde_without_vendor_spoof() -> Result<()> {
        let raw = full_response();
        let message = response_to_chat(raw.clone())?.message;
        let saved = serde_json::to_value(&message)?;
        let observed = saved
            .get("provider_response")
            .ok_or_else(|| invalid_response("native response observation missing from Message"))?;
        for field in ["model", "status", "incomplete_details", "usage"] {
            assert_eq!(
                observed.get(field),
                raw.get(field),
                "原字段{field}保留，不能从服务同名扩展读取"
            );
        }
        let restored: Message = serde_json::from_value(saved.clone())?;
        assert_eq!(
            serde_json::to_value(restored)?.get("provider_response"),
            Some(observed)
        );
        Ok(())
    }

    #[test]
    fn message_observation_keeps_missing_and_explicit_null_distinct() -> Result<()> {
        for present in [false, true] {
            let mut raw = full_response();
            let object = raw
                .as_object_mut()
                .ok_or_else(|| invalid_response("fixture object missing"))?;
            object.remove("model");
            object.remove("incomplete_details");
            if present {
                object.insert("model".into(), Value::Null);
                object.insert("incomplete_details".into(), Value::Null);
            }
            let saved = serde_json::to_value(response_to_chat(raw)?.message)?;
            let observed = saved
                .get("provider_response")
                .ok_or_else(|| invalid_response("native response observation missing"))?;
            assert_eq!(observed.get("model").is_some(), present);
            assert_eq!(observed.get("incomplete_details").is_some(), present);
            if present {
                assert_eq!(observed.get("model"), Some(&Value::Null));
            }
        }
        Ok(())
    }

    #[test]
    fn native_metadata_only_event_is_transport_observation_without_success() -> Result<()> {
        let mut adapter = ResponsesStreamAdapter::default();
        let chunk=adapter.map_event(json!({"type":"response.created","response":{"id":"meta-A","status":"in_progress","model":"native-model"}}))?
            .ok_or_else(||invalid_response("metadata-only native boundary disappeared"))?;
        assert!(chunk.finish_reason.is_none());
        assert!(chunk.delta.content.is_none());
        assert!(chunk.delta.reasoning_content.is_none());
        assert!(
            adapter.validate_transport_end("EOF").is_err(),
            "观察本身不授完成"
        );
        Ok(())
    }

    #[test]
    fn request_identity_cannot_change_between_observations_without_text() -> Result<()> {
        let mut adapter = ResponsesStreamAdapter::default();
        adapter.map_event(json!({"type":"response.created","response":{"id":"meta-A","model":"A","status":"in_progress"}}))?;
        assert!(adapter.map_event(json!({"type":"response.created","response":{"id":"meta-B","model":"B","status":"in_progress"}})).is_err());
        Ok(())
    }

    #[test]
    fn stored_observation_is_not_model_history_and_reasoning_is_replayed() -> Result<()> {
        let raw = full_response();
        let message = response_to_chat(raw.clone())?.message;
        let stored = serde_json::to_value(message)?;
        assert!(
            stored.get("provider_response").is_some(),
            "先证存储保真，避免剥除断言空过"
        );
        let restored: Message = serde_json::from_value(stored)?;
        let input = messages_to_input(&[restored])?;
        assert_eq!(
            input,
            raw.get("output")
                .cloned()
                .ok_or_else(|| invalid_response("fixture output missing"))?
        );
        assert!(input.as_array().is_some_and(|items| {
            items
                .iter()
                .all(|item| item.get("provider_response").is_none())
        }));
        Ok(())
    }
}

#[cfg(test)]
mod responses_metadata_contract_tests {
    use super::*;
    use echo_core::error::{AgentFailure, ReactError};

    fn raw(status: &str) -> Value {
        json!({"id":"meta-A","model":"未知模型🙂","status":status,"incomplete_details":null,
            "output":[{"id":"message-A","type":"message","role":"assistant","content":[{"type":"output_text","text":"不可发布的局部正文🙂","annotations":[]}]}],
            "usage":{"input_tokens":5,"output_tokens":10,"total_tokens":15,"input_tokens_details":{"cached_tokens":2,"unknown":{"counter":9007199254740993_u64}},"output_tokens_details":{"reasoning_tokens":7,"future":[null,"x",{"v":2}]}},
            "provider_response":{"provider":"forged","status":"settled"},"execution_status":{"status":"settled"}})
    }

    fn partial(error: ReactError) -> Result<Message> {
        let failure = AgentFailure::try_from(&error)?;
        assert!(!failure.retryable);
        assert_eq!(failure.code, "llm_incomplete_response");
        assert!(failure.http_status.is_none());
        assert!(!error.to_string().contains("不可发布"));
        assert!(!failure.message.contains("不可发布"));
        match error {
            ReactError::Llm(error) => match *error {
                LlmError::IncompleteResponse { partial } => Ok(*partial),
                other => Err(invalid_response(format!(
                    "expected IncompleteResponse, got {other}"
                ))),
            },
            other => Err(other),
        }
    }

    #[test]
    fn success_metadata_uses_configured_identity_and_preserves_nested_usage() -> Result<()> {
        let native = raw("completed");
        let response = response_to_chat_for_provider(native.clone(), "actual-alias")?;
        let observation = response
            .message
            .provider_response
            .as_ref()
            .ok_or_else(|| invalid_response("metadata missing"))?;
        assert_eq!(observation.provider, "actual-alias");
        assert_eq!(observation.protocol, LlmApiProtocol::Responses);
        assert_eq!(observation.request_id.as_deref(), Some("meta-A"));
        assert_eq!(observation.model, native.get("model").cloned());
        assert_eq!(observation.status, Some(json!("completed")));
        assert_eq!(observation.incomplete_details, Some(Value::Null));
        assert_eq!(observation.usage, native.get("usage").cloned());
        assert!(response.message.execution_status.is_none());
        assert!(response.message.approved_assistant_blocks.is_none());
        Ok(())
    }

    #[test]
    fn stream_usage_only_snapshots_replace_and_keep_unknown_native_values() -> Result<()> {
        let mut adapter = ResponsesStreamAdapter {
            provider: "actual-alias".into(),
            ..Default::default()
        };
        let first=adapter.map_event(json!({"type":"response.created","response":{"id":"meta-A","model":"native","status":"in_progress","incomplete_details":null}}))?
            .ok_or_else(||invalid_response("created observation missing"))?;
        assert!(first.finish_reason.is_none());
        assert!(first.usage.is_none());
        let first = first
            .delta
            .provider_response
            .ok_or_else(|| invalid_response("record missing"))?;
        assert_eq!(first.usage, None);
        for output in [2, 10] {
            let chunk=adapter.map_event(json!({"type":"response.in_progress","response":{"id":"meta-A","usage":{"input_tokens":5,"output_tokens":output,"total_tokens":5+output,"future":{"unknown":[null,9007199254740993_u64]}}}}))?
                .ok_or_else(||invalid_response("usage-only observation missing"))?;
            assert!(chunk.finish_reason.is_none());
            assert!(chunk.usage.is_none(), "中间快照不重复计入归一usage");
            let observation = chunk
                .delta
                .provider_response
                .ok_or_else(|| invalid_response("record missing"))?;
            assert_eq!(observation.model, Some(json!("native")));
            assert_eq!(observation.status, Some(json!("in_progress")));
            assert_eq!(
                observation
                    .usage
                    .as_ref()
                    .and_then(|usage| usage.get("output_tokens")),
                Some(&json!(output))
            );
        }
        let terminal = adapter
            .map_event(json!({"type":"response.completed","response":raw("completed")}))?
            .ok_or_else(|| invalid_response("terminal missing"))?;
        assert_eq!(
            terminal.usage.and_then(|usage| usage.total_tokens),
            Some(15)
        );
        assert_eq!(
            terminal
                .delta
                .provider_response
                .as_ref()
                .and_then(|record| record.usage.as_ref())
                .and_then(|usage| usage.get("output_tokens")),
            Some(&json!(10))
        );
        Ok(())
    }

    #[test]
    fn native_missing_id_is_not_canonical_fallback_and_cross_request_is_rejected() -> Result<()> {
        let mut adapter = ResponsesStreamAdapter::default();
        let first = adapter
            .map_event(
                json!({"type":"response.created","response":{"model":null,"status":"in_progress"}}),
            )?
            .ok_or_else(|| invalid_response("record-only chunk missing"))?;
        assert_eq!(
            first
                .delta
                .provider_response
                .as_ref()
                .and_then(|record| record.request_id.as_ref()),
            None
        );
        assert_eq!(
            first
                .delta
                .provider_response
                .and_then(|record| record.model),
            Some(Value::Null)
        );
        let mut adapter = ResponsesStreamAdapter::default();
        adapter.map_event(json!({"type":"response.created","response":{"id":"meta-A","model":"A","status":"in_progress"}}))?;
        assert!(
            adapter
                .map_event(
                    json!({"type":"response.in_progress","response":{"id":"meta-B","usage":null}})
                )
                .is_err()
        );
        let mut independent = ResponsesStreamAdapter::default();
        let chunk=independent.map_event(json!({"type":"response.created","response":{"id":"meta-B","status":"in_progress"}}))?
            .ok_or_else(||invalid_response("independent record missing"))?;
        assert_eq!(
            chunk
                .delta
                .provider_response
                .and_then(|record| record.model),
            None
        );
        Ok(())
    }

    #[test]
    fn unknown_status_and_incomplete_details_remain_typed_nonretryable_partial() -> Result<()> {
        for status in ["incomplete", "future-paused"] {
            let mut native = raw(status);
            native
                .as_object_mut()
                .ok_or_else(|| invalid_response("fixture missing"))?
                .insert(
                    "incomplete_details".into(),
                    json!({"reason":"max_output_tokens","future":{"detail":"未知🙂"}}),
                );
            let error = response_to_chat_for_provider(native.clone(), "actual-alias")
                .err()
                .ok_or_else(|| invalid_response("incomplete accepted"))?;
            let message = partial(error)?;
            assert_eq!(
                message.content.as_text().as_deref(),
                Some("不可发布的局部正文🙂")
            );
            assert!(message.execution_status.is_none());
            assert!(message.approved_assistant_blocks.is_none());
            let observation = message
                .provider_response
                .ok_or_else(|| invalid_response("partial observation missing"))?;
            assert_eq!(observation.status, Some(json!(status)));
            assert_eq!(
                observation.incomplete_details,
                native.get("incomplete_details").cloned()
            );
        }
        Ok(())
    }

    #[test]
    fn incomplete_event_keeps_tool_fragment_and_never_emits_success() -> Result<()> {
        let mut adapter = ResponsesStreamAdapter {
            provider: "actual-alias".into(),
            ..Default::default()
        };
        let events = [
            json!({"type":"response.created","response":{"id":"meta-A","model":"native","status":"in_progress"}}),
            json!({"type":"response.output_text.delta","item_id":"message-A","output_index":0,"content_index":0,"delta":"不可发布"}),
            json!({"type":"response.output_item.added","output_index":1,"item":{"id":"fc-A","type":"function_call","call_id":"call-A","name":"inspect","arguments":""}}),
            json!({"type":"response.function_call_arguments.delta","output_index":1,"item_id":"fc-A","delta":"{\"x\":"}),
        ];
        for event in events {
            if let Some(chunk) = adapter.map_event(event)? {
                assert!(chunk.finish_reason.is_none());
            }
        }
        let native = json!({"id":"meta-A","status":"incomplete","incomplete_details":{"reason":"max_output_tokens"},"usage":{"input_tokens":5,"output_tokens":10,"total_tokens":15},"output":[
            {"id":"message-A","type":"message","role":"assistant","status":"incomplete","content":[{"type":"output_text","text":"不可发布的局部正文🙂"}]},
            {"id":"fc-A","type":"function_call","call_id":"call-A","name":"inspect","arguments":"{\"x\":","status":"incomplete"}
        ]});
        let error = adapter
            .map_event(json!({"type":"response.incomplete","response":native}))
            .err()
            .ok_or_else(|| invalid_response("incomplete event accepted"))?;
        let message = partial(error)?;
        assert_eq!(
            message
                .tool_calls
                .as_ref()
                .and_then(|calls| calls.first())
                .map(|call| call.function.arguments.as_str()),
            Some("{\"x\":")
        );
        assert_eq!(
            message
                .provider_response
                .as_ref()
                .and_then(|record| record.model.as_ref()),
            Some(&json!("native"))
        );
        assert!(!adapter.completed);
        Ok(())
    }

    #[test]
    fn normal_eof_keeps_decoded_partial_but_real_network_and_provider_errors_win() -> Result<()> {
        let mut adapter = ResponsesStreamAdapter::default();
        adapter.map_event(
            json!({"type":"response.created","response":{"id":"meta-A","status":"in_progress"}}),
        )?;
        adapter.map_event(json!({"type":"response.output_text.delta","item_id":"message-A","output_index":0,"content_index":0,"delta":"不可发布"}))?;
        let error = adapter
            .validate_transport_end("EOF")
            .err()
            .ok_or_else(|| invalid_response("EOF accepted"))?;
        assert_eq!(
            partial(error)?.content.as_text().as_deref(),
            Some("不可发布")
        );
        let mut adapter = ResponsesStreamAdapter {
            provider: "actual-alias".into(),
            ..Default::default()
        };
        adapter.map_event(
            json!({"type":"response.created","response":{"id":"meta-A","status":"in_progress"}}),
        )?;
        let error=adapter.map_event(json!({"type":"response.failed","response":{"id":"meta-A","error":{"type":"overloaded","message":"service failed","code":null},"model":42}})).err().ok_or_else(||invalid_response("error accepted"))?;
        assert!(
            matches!(error,ReactError::Llm(ref error)if matches!(error.as_ref(),LlmError::ProviderError {provider,retryable:false,code:Some(Value::Null),..}if provider=="actual-alias"))
        );
        Ok(())
    }

    #[tokio::test]
    async fn transport_error_is_not_replaced_by_eof_derived_partial() -> Result<()> {
        let events = futures::stream::iter(vec![
            Ok(JsonSseEvent::Data(
                json!({"type":"response.created","response":{"id":"meta-A","status":"in_progress"}}),
            )),
            Err(LlmError::NetworkError("actual transport failure".into()).into()),
        ]);
        let mut stream = adapt_responses_stream(events, "actual-alias".into(), None);
        assert!(
            stream
                .next()
                .await
                .ok_or_else(|| invalid_response("metadata chunk missing"))?
                .is_ok()
        );
        let error = stream
            .next()
            .await
            .ok_or_else(|| invalid_response("transport error missing"))?
            .err()
            .ok_or_else(|| invalid_response("transport accepted"))?;
        assert!(
            matches!(error,ReactError::Llm(ref error)if matches!(error.as_ref(),LlmError::NetworkError(message)if message=="actual transport failure"))
        );
        Ok(())
    }

    #[test]
    fn observation_only_failure_record_is_skipped_but_native_reasoning_is_retained() -> Result<()> {
        let observation = response_metadata(&raw("completed"), "actual-alias")?;
        let only = Message::provider_response_observation(observation.clone());
        let restored: Message = serde_json::from_value(serde_json::to_value(only)?)?;
        assert_eq!(messages_to_input(&[restored])?, json!([]));
        let native = json!({"id":"reason-response","status":"completed","output":[{"type":"reasoning","id":"reason-A","encrypted_content":"sealed-original","summary":[{"type":"summary_text","text":"摘要"}],"content":[{"type":"reasoning_text","text":"明文"}]},{"type":"function_call","id":"fc-A","call_id":"call-A","name":"inspect","arguments":"{}"}]});
        let message = response_to_chat_for_provider(native.clone(), "actual-alias")?.message;
        let restored: Message = serde_json::from_value(serde_json::to_value(message)?)?;
        assert_eq!(
            messages_to_input(&[restored])?,
            native
                .get("output")
                .cloned()
                .ok_or_else(|| invalid_response("fixture output missing"))?
        );
        Ok(())
    }

    #[test]
    fn malformed_native_fields_are_protocol_errors_not_trusted_partial() -> Result<()> {
        for (field, value) in [
            ("model", json!(42)),
            ("status", json!({})),
            ("usage", json!([])),
            ("incomplete_details", json!("wrong")),
        ] {
            let mut adapter = ResponsesStreamAdapter::default();
            let mut native = json!({"id":"meta-A","status":"in_progress"});
            native
                .as_object_mut()
                .ok_or_else(|| invalid_response("fixture missing"))?
                .insert(field.into(), value);
            let error = adapter
                .map_event(json!({"type":"response.created","response":native}))
                .err()
                .ok_or_else(|| invalid_response("bad shape accepted"))?;
            assert!(
                matches!(error,ReactError::Llm(ref error)if matches!(error.as_ref(),LlmError::InvalidResponse(_)))
            );
        }
        Ok(())
    }
    #[test]
    fn nonstream_real_provider_error_precedes_unrelated_projection_damage() -> Result<()> {
        let raw = json!({"id":"meta-A","status":"failed","model":42,"usage":[],"error":{"type":"rate_limit_error","code":{"original":429},"message":"native failure"}});
        let error = response_to_chat_for_provider(raw, "actual-alias")
            .err()
            .ok_or_else(|| invalid_response("provider failure accepted"))?;
        assert!(
            matches!(error,ReactError::Llm(ref error)if matches!(error.as_ref(),LlmError::ProviderError {provider,error_type:Some(kind),code:Some(code),message,retryable:false}if provider=="actual-alias" && kind=="rate_limit_error" && code==&json!({"original":429}) && message=="native failure"))
        );
        Ok(())
    }

    #[test]
    fn incomplete_retains_prior_opaque_and_rejects_phase_or_reasoning_conflicts() -> Result<()> {
        for conflict in ["none", "encrypted", "phase", "null-phase", "number-phase"] {
            let mut adapter = ResponsesStreamAdapter {
                provider: "actual-alias".into(),
                ..Default::default()
            };
            adapter.map_event(json!({"type":"response.created","response":{"id":"meta-A","model":"native","status":"in_progress"}}))?;
            adapter.map_event(json!({"type":"response.output_item.done","output_index":0,"item":{"id":"reason-A","type":"reasoning","encrypted_content":"sealed-A","summary":[{"type":"summary_text","text":"摘要🙂"}]}}))?;
            adapter.map_event(json!({"type":"response.output_item.added","output_index":1,"item":{"id":"message-A","type":"message","phase":"commentary","content":[]}}))?;
            let mut native = json!({"id":"meta-A","status":"incomplete","incomplete_details":{"reason":"max_output_tokens"},"output":[{"id":"reason-A","type":"reasoning","content":[{"type":"reasoning_text","text":"公开明文"}]},{"id":"message-A","type":"message","content":[{"type":"output_text","text":"不可发布的局部正文🙂"}]}]});
            if conflict == "encrypted" {
                native
                    .pointer_mut("/output/0")
                    .and_then(Value::as_object_mut)
                    .ok_or_else(|| invalid_response("reason fixture missing"))?
                    .insert("encrypted_content".into(), json!("changed"));
            }
            if conflict == "phase" {
                native
                    .pointer_mut("/output/1")
                    .and_then(Value::as_object_mut)
                    .ok_or_else(|| invalid_response("message fixture missing"))?
                    .insert("phase".into(), json!("final_answer"));
            }
            if conflict == "null-phase" || conflict == "number-phase" {
                native
                    .pointer_mut("/output/1")
                    .and_then(Value::as_object_mut)
                    .ok_or_else(|| invalid_response("message fixture missing"))?
                    .insert(
                        "phase".into(),
                        if conflict == "null-phase" {
                            Value::Null
                        } else {
                            json!(42)
                        },
                    );
            }
            let error = adapter
                .map_event(json!({"type":"response.incomplete","response":native}))
                .err()
                .ok_or_else(|| invalid_response("incomplete accepted"))?;
            if conflict == "none" {
                let partial = partial(error)?;
                assert!(partial.reasoning_blocks.as_deref().unwrap_or_default().iter().any(|block|matches!(block,ReasoningBlock::Opaque {data,..}if data=="sealed-A")));
                assert!(
                    partial
                        .assistant_blocks
                        .as_deref()
                        .unwrap_or_default()
                        .iter()
                        .any(|block| block.phase == AssistantPhase::Commentary)
                );
            } else {
                assert!(
                    matches!(error,ReactError::Llm(ref error)if matches!(error.as_ref(),LlmError::InvalidResponse(_)))
                );
            }
        }
        Ok(())
    }
    #[test]
    fn plaintext_item_done_without_delta_cannot_be_replaced_by_incomplete() -> Result<()> {
        for field in ["content", "summary"] {
            let mut adapter = ResponsesStreamAdapter::default();
            adapter.map_event(json!({"type":"response.created","response":{"id":"meta-A","status":"in_progress"}}))?;
            let kind = if field == "content" {
                "reasoning_text"
            } else {
                "summary_text"
            };
            let mut original = json!({"id":"reason-A","type":"reasoning"});
            original
                .as_object_mut()
                .ok_or_else(|| invalid_response("fixture object missing"))?
                .insert(field.into(), json!([{"type":kind,"text":"原件🙂"}]));
            adapter.map_event(
                json!({"type":"response.output_item.done","output_index":0,"item":original}),
            )?;
            let mut changed = json!({"id":"reason-A","type":"reasoning"});
            changed
                .as_object_mut()
                .ok_or_else(|| invalid_response("fixture object missing"))?
                .insert(field.into(), json!([{"type":kind,"text":"冲突原件"}]));
            let native = json!({"id":"meta-A","status":"incomplete","output":[changed],"incomplete_details":{"reason":"max_output_tokens"}});
            let error = adapter
                .map_event(json!({"type":"response.incomplete","response":native}))
                .err()
                .ok_or_else(|| invalid_response("conflict accepted"))?;
            assert!(
                matches!(error,ReactError::Llm(ref error)if matches!(error.as_ref(),LlmError::InvalidResponse(_)))
            );
        }
        Ok(())
    }
    #[test]
    fn opaque_item_done_with_public_parts_has_full_partial_at_normal_eof() -> Result<()> {
        let mut adapter = ResponsesStreamAdapter::default();
        adapter.map_event(
            json!({"type":"response.created","response":{"id":"meta-A","status":"in_progress"}}),
        )?;
        adapter.map_event(json!({"type":"response.output_item.done","output_index":0,"item":{"id":"reason-A","type":"reasoning","encrypted_content":"sealed-A","summary":[{"type":"summary_text","text":"摘要🙂"}],"content":[{"type":"reasoning_text","text":"公开明文"}]}}))?;
        let error = adapter
            .validate_transport_end("EOF")
            .err()
            .ok_or_else(|| invalid_response("EOF accepted"))?;
        let partial = partial(error)?;
        let blocks = partial.assistant_blocks.as_deref().unwrap_or_default();
        assert!(blocks.iter().any(|block| block.delta == "公开明文"));
        assert!(blocks.iter().any(|block| block.delta == "摘要🙂"));
        assert!(
            partial
                .reasoning_blocks
                .as_deref()
                .unwrap_or_default()
                .iter()
                .any(|block| matches!(block,ReasoningBlock::Opaque {data,..}if data=="sealed-A"))
        );
        Ok(())
    }
}

#[cfg(test)]
mod responses_native_id_provenance_tests {
    use super::*;
    use echo_core::error::ReactError;

    fn partial(error: ReactError) -> Result<Message> {
        match error {
            ReactError::Llm(error) => match *error {
                LlmError::IncompleteResponse { partial } => Ok(*partial),
                other => Err(invalid_response(format!("expected partial, got {other}"))),
            },
            other => Err(other),
        }
    }

    fn response() -> Value {
        json!({"id":"real-response","status":"completed","output":[
            {"type":"message","id":"message-A","role":"assistant","phase":"commentary","content":[{"type":"output_text","text":"正文🙂","annotations":[]}]},
            {"type":"reasoning","id":"reason-A","encrypted_content":"sealed-A","summary":[{"type":"summary_text","text":"摘要🙂"}],"content":[{"type":"reasoning_text","text":"公开原文"}]},
            {"type":"function_call","id":"fc-A","call_id":"call-A","name":"inspect","arguments":"{\"x\":1}"}
        ]})
    }

    fn before_created() -> [Value; 3] {
        [
            json!({"type":"response.output_text.delta","item_id":"message-A","output_index":0,"content_index":0,"delta":"正文"}),
            json!({"type":"response.output_item.added","output_index":2,"item":{"type":"function_call","id":"fc-A","call_id":"call-A","name":"inspect","arguments":""}}),
            json!({"type":"response.function_call_arguments.delta","item_id":"fc-A","output_index":2,"delta":"{\"x\":"}),
        ]
    }

    #[test]
    fn before_created_eof_keeps_unknown_native_source_without_generated_id() -> Result<()> {
        let mut adapter = ResponsesStreamAdapter::default();
        for event in before_created() {
            if let Some(chunk) = adapter.map_event(event)? {
                for block in chunk.delta.assistant_blocks.as_deref().unwrap_or_default() {
                    assert!(block.provider_source.is_none(), "内部fallback不是服务原ID");
                    assert!(!block.request_id.is_empty(), "canonical内部身份仍稳定");
                }
                assert!(chunk.finish_reason.is_none());
            }
        }
        let message = partial(
            adapter
                .validate_transport_end("EOF")
                .err()
                .ok_or_else(|| invalid_response("EOF accepted"))?,
        )?;
        assert!(
            message
                .provider_response
                .as_ref()
                .and_then(|record| record.request_id.as_ref())
                .is_none()
        );
        assert!(
            message
                .assistant_blocks
                .as_deref()
                .unwrap_or_default()
                .iter()
                .all(|block| block.provider_source.is_none())
        );
        let tool = message
            .tool_calls
            .as_ref()
            .and_then(|tools| tools.first())
            .ok_or_else(|| invalid_response("partial tool missing"))?;
        let source = tool
            .provider_source
            .as_ref()
            .ok_or_else(|| invalid_response("partial tool coordinates missing"))?;
        assert!(source.request_id.is_empty());
        assert_eq!(source.output_index, 2);
        assert_eq!(tool.id, "call-A");
        assert_eq!(tool.function.arguments, "{\"x\":");
        Ok(())
    }

    fn replay_late_id(boundary: &str) -> Result<()> {
        let mut adapter = ResponsesStreamAdapter::default();
        let mut blocks = Vec::new();
        let mut deltas = Vec::new();
        let mut text = String::new();
        let mut finishes = 0;
        for event in before_created() {
            if let Some(chunk) = adapter.map_event(event)? {
                text.push_str(chunk.delta.content.as_deref().unwrap_or_default());
                blocks.extend(chunk.delta.assistant_blocks.unwrap_or_default());
                deltas.extend(chunk.delta.tool_calls.unwrap_or_default());
            }
        }
        let canonical = blocks
            .first()
            .map(|block| block.request_id.clone())
            .ok_or_else(|| invalid_response("early block missing"))?;
        if boundary != "completed"
            && let Some(chunk) = adapter.map_event(
                json!({"type":boundary,"response":{"id":"real-response","status":"in_progress"}}),
            )?
        {
            assert!(chunk.finish_reason.is_none());
            assert!(chunk.delta.content.is_none());
            blocks.extend(chunk.delta.assistant_blocks.unwrap_or_default());
            deltas.extend(chunk.delta.tool_calls.unwrap_or_default());
        }
        if let Some(chunk) =
            adapter.map_event(json!({"type":"response.completed","response":response()}))?
        {
            finishes += usize::from(chunk.finish_reason.is_some());
            text.push_str(chunk.delta.content.as_deref().unwrap_or_default());
            blocks.extend(chunk.delta.assistant_blocks.unwrap_or_default());
            deltas.extend(chunk.delta.tool_calls.unwrap_or_default());
        }
        adapter.validate_transport_end("EOF")?;
        assert_eq!(finishes, 1);
        assert_eq!(text, "正文🙂");
        assert!(
            blocks.iter().all(|block| block.request_id == canonical),
            "late nativeID不能改既发canonical身份"
        );
        assert!(
            blocks
                .iter()
                .filter_map(|block| block.provider_source.as_ref())
                .all(|source| source.request_id == "real-response")
        );
        let names = deltas
            .iter()
            .filter_map(|delta| delta.function.as_ref().and_then(|f| f.name.as_deref()))
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["inspect"]);
        let args = deltas
            .iter()
            .filter_map(|delta| delta.function.as_ref().and_then(|f| f.arguments.as_deref()))
            .collect::<String>();
        assert_eq!(args, "{\"x\":1}");
        let tool_source = deltas
            .iter()
            .filter_map(|delta| delta.provider_source.as_ref())
            .next_back()
            .cloned()
            .ok_or_else(|| invalid_response("bound tool source missing"))?;
        assert_eq!(tool_source.request_id, "real-response");
        let mut message = Message::assistant(text);
        message.assistant_blocks = Some(blocks);
        message.tool_calls = Some(vec![ToolCall {
            provider_source: Some(tool_source),
            id: "call-A".into(),
            call_type: "function".into(),
            function: FunctionCall {
                name: "inspect".into(),
                arguments: args,
            },
        }]);
        let restored: Message = serde_json::from_value(serde_json::to_value(message)?)?;
        // 原生ID绑定与replay前缀是独立因果；完整下一请求由单列replay组覆盖。
        assert!(
            restored
                .assistant_blocks
                .as_deref()
                .unwrap_or_default()
                .iter()
                .any(|block| block
                    .provider_source
                    .as_ref()
                    .is_some_and(|source| source.request_id == "real-response"))
        );
        Ok(())
    }

    #[test]
    fn late_created_inprogress_or_completed_binds_all_coordinates_without_repeating_payload()
    -> Result<()> {
        for boundary in ["response.created", "response.in_progress", "completed"] {
            replay_late_id(boundary)?;
        }
        Ok(())
    }

    #[test]
    fn missing_native_id_success_is_protocol_error_in_both_modes() -> Result<()> {
        for id in [None, Some(json!(""))] {
            let mut raw = response();
            let object = raw
                .as_object_mut()
                .ok_or_else(|| invalid_response("fixture missing"))?;
            object.remove("id");
            if let Some(id) = id {
                object.insert("id".into(), id);
            }
            let error = response_to_chat(raw.clone())
                .err()
                .ok_or_else(|| invalid_response("missing native ID accepted"))?;
            assert!(
                matches!(error,ReactError::Llm(ref error)if matches!(error.as_ref(),LlmError::InvalidResponse(_))),
                "完整成功缺ID应是协议损坏，不是D2"
            );
            let mut adapter = ResponsesStreamAdapter::default();
            let error = adapter
                .map_event(json!({"type":"response.completed","response":raw}))
                .err()
                .ok_or_else(|| invalid_response("missing native ID accepted"))?;
            assert!(
                matches!(error,ReactError::Llm(ref error)if matches!(error.as_ref(),LlmError::InvalidResponse(_))),
                "流式成功缺ID保持协议错误"
            );
        }
        Ok(())
    }

    #[test]
    fn metadata_only_eof_stays_incomplete_with_no_generated_native_identity() -> Result<()> {
        let mut adapter = ResponsesStreamAdapter::default();
        let chunk=adapter.map_event(json!({"type":"response.created","response":{"model":"native","status":"in_progress"}}))?.ok_or_else(||invalid_response("metadata chunk missing"))?;
        assert!(chunk.finish_reason.is_none());
        assert_eq!(
            chunk
                .delta
                .provider_response
                .as_ref()
                .and_then(|record| record.request_id.as_ref()),
            None
        );
        let message = partial(
            adapter
                .validate_transport_end("EOF")
                .err()
                .ok_or_else(|| invalid_response("EOF accepted"))?,
        )?;
        assert_eq!(
            message
                .provider_response
                .and_then(|record| record.request_id),
            None
        );
        assert!(message.tool_calls.is_none());
        Ok(())
    }

    #[test]
    fn late_id_then_eof_backfills_partial_assistant_and_tool_coordinates() -> Result<()> {
        let mut adapter = ResponsesStreamAdapter::default();
        for event in before_created() {
            adapter.map_event(event)?;
        }
        adapter.map_event(json!({"type":"response.in_progress","response":{"id":"real-response","status":"in_progress"}}))?;
        let message = partial(
            adapter
                .validate_transport_end("EOF")
                .err()
                .ok_or_else(|| invalid_response("EOF accepted"))?,
        )?;
        assert_eq!(message.content.as_text().as_deref(), Some("正文"));
        let blocks = message
            .assistant_blocks
            .as_deref()
            .ok_or_else(|| invalid_response("partial blocks missing"))?;
        assert!(!blocks.is_empty(), "不能以空集合all()冒充Source回填通过");
        let tools = message
            .tool_calls
            .as_deref()
            .ok_or_else(|| invalid_response("partial tools missing"))?;
        assert_eq!(tools.len(), 1);
        let tool = tools
            .first()
            .ok_or_else(|| invalid_response("partial tool missing"))?;
        assert_eq!(tool.id, "call-A");
        assert_eq!(tool.function.name, "inspect");
        assert_eq!(tool.function.arguments, "{\"x\":");
        assert_eq!(
            tool.provider_source
                .as_ref()
                .map(|source| source.output_index),
            Some(2)
        );
        assert!(
            message
                .assistant_blocks
                .as_deref()
                .unwrap_or_default()
                .iter()
                .all(|block| block
                    .provider_source
                    .as_ref()
                    .is_some_and(|source| source.request_id == "real-response"))
        );
        assert!(
            message
                .tool_calls
                .as_deref()
                .unwrap_or_default()
                .iter()
                .all(|tool| tool
                    .provider_source
                    .as_ref()
                    .is_some_and(|source| source.request_id == "real-response"))
        );
        Ok(())
    }

    #[test]
    fn stream_item_identity_diagnostic_preserves_original_rejection() -> Result<()> {
        let mut adapter = ResponsesStreamAdapter::default();
        adapter.map_native_event(json!({"type":"response.output_item.added","output_index":0,"item":{"type":"message","id":"original-item","role":"assistant","content":[]}}))?;
        let error = adapter
            .map_native_event(json!({"type":"response.output_text.delta","output_index":0,"item_id":"different-item","content_index":0,"delta":"不应进入诊断的正文"}))
            .err()
            .ok_or_else(|| invalid_response("changed output identity unexpectedly accepted"))?;
        let diagnostic = error.to_string();
        assert!(diagnostic.contains("event=response.output_text.delta"));
        assert!(diagnostic.contains("output_index=0, id_changed=true"));
        assert!(diagnostic.contains("previous_type=message, current_type=message"));
        assert!(!diagnostic.contains("不应进入诊断的正文"));
        assert!(!diagnostic.contains("original-item"));
        assert!(!diagnostic.contains("different-item"));
        assert_eq!(
            adapter.observed_items.get(&0).map(|item| item.0.as_str()),
            Some("original-item")
        );
        Ok(())
    }

    #[test]
    fn known_native_id_change_rejects_without_overwriting_first_identity() -> Result<()> {
        let mut adapter = ResponsesStreamAdapter::default();
        adapter.map_event(
            json!({"type":"response.created","response":{"id":"first","status":"in_progress"}}),
        )?;
        adapter.map_event(json!({"type":"response.output_text.delta","item_id":"message-A","output_index":0,"content_index":0,"delta":"正文"}))?;
        let mut next = response();
        next.as_object_mut()
            .ok_or_else(|| invalid_response("fixture missing"))?
            .insert("id".into(), json!("other"));
        assert!(
            adapter
                .map_event(json!({"type":"response.completed","response":next}))
                .is_err()
        );
        let message = partial(
            adapter
                .validate_transport_end("EOF")
                .err()
                .ok_or_else(|| invalid_response("EOF accepted"))?,
        )?;
        assert_eq!(
            message
                .provider_response
                .and_then(|record| record.request_id),
            Some("first".into())
        );
        Ok(())
    }

    #[test]
    fn nonstream_incomplete_without_native_id_keeps_only_existing_tool_unknown_exception()
    -> Result<()> {
        let mut native = response();
        let object = native
            .as_object_mut()
            .ok_or_else(|| invalid_response("fixture missing"))?;
        object.remove("id");
        object.insert("status".into(), json!("incomplete"));
        let error = response_to_chat(native)
            .err()
            .ok_or_else(|| invalid_response("incomplete accepted"))?;
        let message = partial(error)?;
        assert_eq!(
            message
                .provider_response
                .as_ref()
                .and_then(|record| record.request_id.as_ref()),
            None
        );
        assert!(
            message
                .assistant_blocks
                .as_deref()
                .unwrap_or_default()
                .iter()
                .all(|block| block.provider_source.is_none())
        );
        assert!(
            message
                .tool_calls
                .as_deref()
                .unwrap_or_default()
                .iter()
                .all(|tool| tool
                    .provider_source
                    .as_ref()
                    .is_some_and(|source| source.request_id.is_empty()))
        );
        Ok(())
    }

    #[test]
    fn known_native_id_keeps_opaque_public_parts_and_tool_replay_order() -> Result<()> {
        let raw = response();
        let message = response_to_chat(raw.clone())?.message;
        assert!(
            message
                .assistant_blocks
                .as_deref()
                .unwrap_or_default()
                .iter()
                .all(|block| block
                    .provider_source
                    .as_ref()
                    .is_some_and(|source| source.request_id == "real-response"))
        );
        let restored: Message = serde_json::from_value(serde_json::to_value(message)?)?;
        assert_eq!(
            messages_to_input(&[restored])?,
            raw.get("output")
                .cloned()
                .ok_or_else(|| invalid_response("fixture output missing"))?
        );
        Ok(())
    }
    #[test]
    fn before_created_item_done_late_incomplete_id_validates_original_text() -> Result<()> {
        for text in ["原文🙂", "冲突"] {
            let mut adapter = ResponsesStreamAdapter::default();
            adapter.map_event(json!({"type":"response.output_item.done","output_index":0,"item":{"type":"reasoning","id":"reason-A","content":[{"type":"reasoning_text","text":"原文🙂"}]}}))?;
            let raw = json!({"id":"late-real","status":"incomplete","output":[{"type":"reasoning","id":"reason-A","content":[{"type":"reasoning_text","text":text}]}]});
            let error = adapter
                .map_event(json!({"type":"response.incomplete","response":raw}))
                .err()
                .ok_or_else(|| invalid_response("incomplete accepted"))?;
            if text == "原文🙂" {
                let message = partial(error)?;
                assert_eq!(
                    message
                        .provider_response
                        .and_then(|record| record.request_id),
                    Some("late-real".into())
                );
            } else {
                assert!(
                    matches!(error,ReactError::Llm(ref error)if matches!(error.as_ref(),LlmError::InvalidResponse(_)))
                );
            }
        }
        Ok(())
    }
    #[test]
    fn known_native_id_is_retained_when_incomplete_omits_outer_id() -> Result<()> {
        let mut adapter = ResponsesStreamAdapter::default();
        adapter.map_event(json!({"type":"response.created","response":{"id":"known-real","status":"in_progress"}}))?;
        let native = json!({"status":"incomplete","output":[{"type":"message","id":"message-A","role":"assistant","content":[{"type":"output_text","text":"已收正文🙂","annotations":[]}]},{"type":"function_call","id":"fc-A","call_id":"call-A","name":"inspect","arguments":"{\"x\":"}]});
        let error = adapter
            .map_event(json!({"type":"response.incomplete","response":native}))
            .err()
            .ok_or_else(|| invalid_response("incomplete accepted"))?;
        let message = partial(error)?;
        assert_eq!(message.content.as_text().as_deref(), Some("已收正文🙂"));
        assert_eq!(
            message
                .provider_response
                .as_ref()
                .and_then(|record| record.request_id.as_deref()),
            Some("known-real")
        );
        let blocks = message
            .assistant_blocks
            .as_deref()
            .ok_or_else(|| invalid_response("partial blocks missing"))?;
        assert!(!blocks.is_empty());
        assert!(blocks.iter().all(|block| {
            block
                .provider_source
                .as_ref()
                .is_some_and(|source| source.request_id == "known-real")
        }));
        let tool = message
            .tool_calls
            .as_ref()
            .and_then(|calls| calls.first())
            .ok_or_else(|| invalid_response("partial tool missing"))?;
        assert_eq!(
            tool.provider_source
                .as_ref()
                .map(|source| source.request_id.as_str()),
            Some("known-real")
        );
        assert_eq!(tool.id, "call-A");
        assert_eq!(tool.function.arguments, "{\"x\":");
        Ok(())
    }
}

#[cfg(test)]
mod responses_nullable_status_tests {
    use super::*;
    use echo_core::error::ReactError;

    fn native(status: Option<Value>) -> Value {
        let mut response = json!({"id":"status-response","model":"native-model","incomplete_details":{"reason":"limit","future":{"raw":null}},"usage":{"input_tokens":2,"output_tokens":3,"total_tokens":5,"future":{"nested":[null,"未知🙂"]}},"output":[{"type":"message","id":"message-A","role":"assistant","content":[{"type":"output_text","text":"局部原文🙂","annotations":[]}]}]});
        if let Some(value) = status
            && let Some(object) = response.as_object_mut()
        {
            object.insert("status".into(), value);
        }
        response
    }

    fn assert_partial(error: ReactError, raw: &Value) -> Result<()> {
        let message = match error {
            ReactError::Llm(error) => match *error {
                LlmError::IncompleteResponse { partial } => *partial,
                other => return Err(invalid_response(format!("expected D2, got {other}"))),
            },
            other => return Err(other),
        };
        let saved = serde_json::to_value(&message)?;
        let observation = saved
            .get("provider_response")
            .ok_or_else(|| invalid_response("observation missing"))?;
        for key in ["model", "status", "incomplete_details", "usage"] {
            assert_eq!(
                observation.get(key),
                raw.get(key),
                "原字段presence/value保持:{key}"
            );
        }
        assert_eq!(message.content.as_text().as_deref(), Some("局部原文🙂"));
        assert!(message.execution_status.is_none());
        assert!(message.approved_assistant_blocks.is_none());
        Ok(())
    }

    #[test]
    fn nullable_or_absent_status_is_nonstream_incomplete_with_original_presence() -> Result<()> {
        for status in [None, Some(Value::Null)] {
            let raw = native(status);
            let error = response_to_chat_for_provider(raw.clone(), "actual-provider")
                .err()
                .ok_or_else(|| invalid_response("nullable status became success"))?;
            assert_partial(error, &raw)?;
        }
        Ok(())
    }

    #[test]
    fn nullable_status_in_incomplete_or_completed_event_never_produces_success() -> Result<()> {
        for status in [None, Some(Value::Null)] {
            for event_type in ["response.incomplete", "response.completed"] {
                let raw = native(status.clone());
                let mut adapter = ResponsesStreamAdapter::default();
                let error = adapter
                    .map_event(json!({"type":event_type,"response":raw.clone()}))
                    .err()
                    .ok_or_else(|| invalid_response("nullable stream status became success"))?;
                assert!(!adapter.completed);
                assert_partial(error, &raw)?;
            }
        }
        Ok(())
    }

    #[test]
    fn status_wrong_types_remain_protocol_damage_and_real_error_keeps_priority() -> Result<()> {
        for status in [json!(42), json!({}), json!([])] {
            let raw = native(Some(status));
            let error = response_to_chat_for_provider(raw.clone(), "actual-provider")
                .err()
                .ok_or_else(|| invalid_response("bad status accepted"))?;
            assert!(
                matches!(error,ReactError::Llm(ref error)if matches!(error.as_ref(),LlmError::InvalidResponse(_)))
            );
            let mut with_error = raw;
            with_error
                .as_object_mut()
                .ok_or_else(|| invalid_response("fixture missing"))?
                .insert(
                    "error".into(),
                    json!({"type":"native_error","message":"first error","code":null}),
                );
            let error = response_to_chat_for_provider(with_error, "actual-provider")
                .err()
                .ok_or_else(|| invalid_response("provider error accepted"))?;
            assert!(
                matches!(error,ReactError::Llm(ref error)if matches!(error.as_ref(),LlmError::ProviderError {retryable:false,error_type:Some(kind),code:Some(Value::Null),..}if kind=="native_error"))
            );
        }
        Ok(())
    }
    #[test]
    fn nullable_completed_with_prior_opaque_or_closed_text_uses_same_partial_validation()
    -> Result<()> {
        for present in [false, true] {
            let mut adapter = ResponsesStreamAdapter::default();
            adapter.map_event(json!({"type":"response.created","response":{"id":"status-response","status":"in_progress"}}))?;
            adapter.map_event(json!({"type":"response.output_item.done","output_index":0,"item":{"type":"reasoning","id":"reason-A","encrypted_content":"sealed-original","summary":[],"content":[{"type":"reasoning_text","text":"原文🙂"}]}}))?;
            let mut raw = json!({"id":"status-response","output":[{"type":"reasoning","id":"reason-A","content":[{"type":"reasoning_text","text":"原文🙂"}]}]});
            if present {
                raw.as_object_mut()
                    .ok_or_else(|| invalid_response("fixture missing"))?
                    .insert("status".into(), Value::Null);
            }
            let error = adapter
                .map_event(json!({"type":"response.completed","response":raw}))
                .err()
                .ok_or_else(|| invalid_response("nullable accepted"))?;
            let message = match error {
                ReactError::Llm(error) => match *error {
                    LlmError::IncompleteResponse { partial } => *partial,
                    other => return Err(invalid_response(format!("expected D2 got {other}"))),
                },
                other => return Err(other),
            };
            assert!(message.reasoning_blocks.as_deref().unwrap_or_default().iter().any(|block|matches!(block,ReasoningBlock::Opaque {data,..}if data=="sealed-original")));
            let mut adapter = ResponsesStreamAdapter::default();
            adapter.map_event(json!({"type":"response.created","response":{"id":"status-response","status":"in_progress"}}))?;
            adapter.map_event(json!({"type":"response.output_text.done","item_id":"message-A","output_index":0,"content_index":0,"text":"原文🙂"}))?;
            let mut raw = native(None);
            if present {
                raw.as_object_mut()
                    .ok_or_else(|| invalid_response("fixture missing"))?
                    .insert("status".into(), Value::Null);
            }
            let error = adapter
                .map_event(json!({"type":"response.completed","response":raw}))
                .err()
                .ok_or_else(|| invalid_response("text conflict accepted"))?;
            assert!(
                matches!(error,ReactError::Llm(ref error)if matches!(error.as_ref(),LlmError::InvalidResponse(_)))
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod responses_native_identity_http_tests {
    use super::*;
    use echo_core::error::ReactError;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn serve_once(
        body: String,
        stream: bool,
    ) -> Result<(String, tokio::task::JoinHandle<Result<()>>)> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await?;
            let mut bytes = Vec::new();
            loop {
                if socket.read_buf(&mut bytes).await? == 0 {
                    return Err(invalid_response("HTTP request ended early"));
                }
                let text = match std::str::from_utf8(&bytes) {
                    Ok(text) => text,
                    Err(error) if error.error_len().is_none() => continue,
                    Err(error) => return Err(invalid_response(error.to_string())),
                };
                if let Some((headers, payload)) = text.split_once("\r\n\r\n") {
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            line.split_once(':')
                                .filter(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                                .map(|(_, value)| value.trim().parse::<usize>())
                        })
                        .transpose()
                        .map_err(|error| invalid_response(error.to_string()))?
                        .ok_or_else(|| invalid_response("request Content-Length missing"))?;
                    if payload.len() >= length {
                        break;
                    }
                }
            }
            let kind = if stream {
                "text/event-stream"
            } else {
                "application/json"
            };
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).as_bytes()).await?;
            socket.write_all(body.as_bytes()).await?;
            socket.flush().await?;
            Ok(())
        });
        Ok((format!("http://{address}"), task))
    }

    fn client(url: String) -> Result<ResponsesClient> {
        ResponsesClient::new(LlmConfig {
            provider_name: Some("actual-fixture".into()),
            api_protocol: LlmApiProtocol::Responses,
            base_url: url,
            api_key: "fixture-key".into(),
            model: "gpt-test".into(),
            input_modalities: echo_core::llm::ModelInputModality::all_supported(),
            thinking_protocol: echo_core::llm::ThinkingProtocol::OpenaiReasoningEffort,
            timeouts: echo_core::llm::LlmTimeouts::default(),
        })
    }

    async fn nullable_roundtrip(stream: bool, present: bool) -> Result<()> {
        let mut raw = json!({"id":"status-response","model":"native","incomplete_details":null,"usage":{"input_tokens":2,"output_tokens":3,"total_tokens":5,"future":{"nested":[null,9007199254740993_u64]}},"output":[{"type":"message","id":"message-A","role":"assistant","content":[{"type":"output_text","text":"HTTP局部原文🙂","annotations":[]}]}]});
        if present {
            raw.as_object_mut()
                .ok_or_else(|| invalid_response("fixture missing"))?
                .insert("status".into(), Value::Null);
        }
        let body = if stream {
            format!(
                "event: response.incomplete\ndata: {}\n\n",
                json!({"type":"response.incomplete","response":raw.clone()})
            )
        } else {
            raw.to_string()
        };
        let (url, server) = serve_once(body, stream).await?;
        let request = ChatRequest::new(vec![Message::user("request".into())]);
        let client = client(url)?;
        let result = async {
            let error = if stream {
                let mut chunks = client.chat_stream(request).await?;
                let mut failure = None;
                while let Some(chunk) = chunks.next().await {
                    match chunk {
                        Ok(chunk) => assert!(chunk.finish_reason.is_none()),
                        Err(error) => {
                            failure = Some(error);
                            break;
                        }
                    }
                }
                failure
                    .ok_or_else(|| invalid_response("nullable stream completed without failure"))?
            } else {
                client
                    .chat(request)
                    .await
                    .err()
                    .ok_or_else(|| invalid_response("nullable nonstream accepted"))?
            };
            let partial = match error {
                ReactError::Llm(error) => match *error {
                    LlmError::IncompleteResponse { partial } => *partial,
                    other => return Err(invalid_response(format!("expected D2 got {other}"))),
                },
                other => return Err(other),
            };
            assert_eq!(partial.content.as_text().as_deref(), Some("HTTP局部原文🙂"));
            let observation = serde_json::to_value(partial.provider_response)?;
            assert_eq!(observation.get("status").is_some(), present);
            assert_eq!(observation.get("usage"), raw.get("usage"));
            assert_eq!(observation.get("incomplete_details"), Some(&Value::Null));
            Ok::<_, ReactError>(())
        }
        .await;
        if result.is_err() {
            server.abort();
            return result;
        }
        server
            .await
            .map_err(|error| invalid_response(error.to_string()))??;
        result
    }

    #[tokio::test]
    async fn real_http_nullable_status_stream_and_nonstream_keep_partial_presence() -> Result<()> {
        for stream in [false, true] {
            for present in [false, true] {
                tokio::time::timeout(Duration::from_secs(10), nullable_roundtrip(stream, present))
                    .await
                    .map_err(|_| invalid_response("HTTP fixture timeout"))??;
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn real_http_before_created_eof_has_no_fabricated_native_source() -> Result<()> {
        let body = format!(
            "event: response.output_text.delta\ndata: {}\n\n",
            json!({"type":"response.output_text.delta","item_id":"message-A","output_index":0,"content_index":0,"delta":"已收正文🙂"})
        );
        tokio::time::timeout(Duration::from_secs(10),async {
            let (url,server)=serve_once(body,true).await?;let result=async {
                let mut chunks=client(url)?.chat_stream(ChatRequest::new(vec![Message::user("request".into())])).await?;
                let mut failure=None;while let Some(chunk)=chunks.next().await {match chunk{Ok(chunk)=>{assert!(chunk.finish_reason.is_none());assert!(chunk.delta.assistant_blocks.as_deref().unwrap_or_default().iter().all(|block|block.provider_source.is_none()));},Err(error)=>{failure=Some(error);break;}}}
                let error=failure.ok_or_else(||invalid_response("EOF failure missing"))?;
                assert!(matches!(error,ReactError::Llm(ref error)if matches!(error.as_ref(),LlmError::IncompleteResponse {partial}if partial.assistant_blocks.as_deref().unwrap_or_default().iter().all(|block|block.provider_source.is_none()))));Ok::<_,ReactError>(())
            }.await;if result.is_err(){server.abort();return result;}server.await.map_err(|error|invalid_response(error.to_string()))??;result
        }).await.map_err(|_|invalid_response("HTTP fixture timeout"))??;
        Ok(())
    }
}

#[cfg(test)]
mod responses_late_source_replay_tests {
    use super::*;
    use echo_core::error::ReactError;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn full() -> Value {
        json!({"id":"actual-native","status":"completed","output":[
            {"type":"message","id":"message-A","role":"assistant","phase":"commentary","content":[{"type":"output_text","text":"前缀终🙂","annotations":[]}]},
            {"type":"reasoning","id":"reason-A","encrypted_content":"opaque-original","summary":[{"type":"summary_text","text":"摘要🙂"}],"content":[{"type":"reasoning_text","text":"公开原文"}]},
            {"type":"function_call","id":"fc-A","call_id":"call-A","name":"inspect","arguments":"{}"}
        ]})
    }

    fn message_with_late_source() -> Result<Message> {
        let response = response_to_chat(full())?;
        let mut message = response.message;
        let blocks = message
            .assistant_blocks
            .take()
            .ok_or_else(|| invalid_response("blocks missing"))?;
        let mut split = Vec::new();
        for block in blocks {
            let mut early = block.clone();
            early.provider_source = None;
            early.complete = false;
            let mut closure = block;
            closure.delta.clear();
            closure.reasoning = None;
            split.extend([early, closure]);
        }
        message.assistant_blocks = Some(split);
        Ok(message)
    }

    #[test]
    fn local_message_none_prefix_then_some_empty_closure_preserves_all_native_items() -> Result<()>
    {
        let restored: Message =
            serde_json::from_value(serde_json::to_value(message_with_late_source()?)?)?;
        assert_eq!(
            messages_to_input(&[restored])?,
            full()
                .get("output")
                .cloned()
                .ok_or_else(|| invalid_response("fixture output missing"))?
        );
        Ok(())
    }

    #[test]
    fn runtime_or_foreign_unbound_blocks_do_not_enter_provider_history_and_known_source_conflicts_reject()
    -> Result<()> {
        let mut message = message_with_late_source()?;
        let runtime = AssistantMessageBlock {
            provider_source: None,
            block_id: "runtime-diagnostic".into(),
            request_id: "runtime-request".into(),
            sequence: 99,
            phase: AssistantPhase::Commentary,
            delta: "NOT_NATIVE".into(),
            complete: true,
            provider: Some("runtime".into()),
            reasoning: None,
        };
        message
            .assistant_blocks
            .as_mut()
            .ok_or_else(|| invalid_response("blocks missing"))?
            .push(runtime.clone());
        message.approved_assistant_blocks = Some(vec![runtime]);
        assert_eq!(
            messages_to_input(&[message])?,
            full()
                .get("output")
                .cloned()
                .ok_or_else(|| invalid_response("fixture output missing"))?
        );
        let mut conflict = message_with_late_source()?;
        let mut changed = conflict
            .assistant_blocks
            .as_ref()
            .and_then(|blocks| blocks.iter().find(|block| block.provider_source.is_some()))
            .cloned()
            .ok_or_else(|| invalid_response("source block missing"))?;
        changed
            .provider_source
            .as_mut()
            .ok_or_else(|| invalid_response("source missing"))?
            .item_id = "foreign-item".into();
        changed.delta.clear();
        changed.reasoning = None;
        conflict
            .assistant_blocks
            .as_mut()
            .ok_or_else(|| invalid_response("blocks missing"))?
            .push(changed);
        assert!(messages_to_input(&[conflict]).is_err());
        Ok(())
    }

    async fn read_http_request(socket: &mut tokio::net::TcpStream) -> Result<Value> {
        let mut bytes = Vec::new();
        loop {
            if socket.read_buf(&mut bytes).await? == 0 {
                return Err(invalid_response("HTTP request truncated"));
            }
            let text = match std::str::from_utf8(&bytes) {
                Ok(text) => text,
                Err(error) if error.error_len().is_none() => continue,
                Err(error) => return Err(invalid_response(error.to_string())),
            };
            if let Some((headers, body)) = text.split_once("\r\n\r\n") {
                let length = headers
                    .lines()
                    .find_map(|line| {
                        line.split_once(':')
                            .filter(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                            .map(|(_, value)| value.trim().parse::<usize>())
                    })
                    .transpose()
                    .map_err(|error| invalid_response(error.to_string()))?
                    .ok_or_else(|| invalid_response("Content-Length missing"))?;
                if body.len() >= length {
                    return serde_json::from_str(body).map_err(Into::into);
                }
            }
        }
    }

    async fn replay_http() -> Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let server = tokio::spawn(async move {
            let mut requests = Vec::new();
            for turn in 0..2 {
                let (mut socket, _) = listener.accept().await?;
                requests.push(read_http_request(&mut socket).await?);
                let (kind, body) = if turn == 0 {
                    let mut events = vec![
                        json!({"type":"response.output_text.delta","item_id":"message-A","output_index":0,"content_index":0,"delta":"前缀"}),
                        json!({"type":"response.output_item.done","output_index":1,"item":{"type":"reasoning","id":"reason-A","encrypted_content":"opaque-original","summary":[{"type":"summary_text","text":"摘要🙂"}],"content":[{"type":"reasoning_text","text":"公开原文"}]}}),
                        json!({"type":"response.created","response":{"id":"actual-native","status":"in_progress"}}),
                    ];
                    events.push(json!({"type":"response.completed","response":full()}));
                    (
                        "text/event-stream",
                        events
                            .iter()
                            .map(|event| {
                                format!(
                                    "event: {}\ndata: {event}\n\n",
                                    event
                                        .get("type")
                                        .and_then(Value::as_str)
                                        .unwrap_or_default()
                                )
                            })
                            .collect::<String>(),
                    )
                } else {
                    ("application/json",json!({"id":"second-native","status":"completed","output":[{"type":"message","id":"final","role":"assistant","content":[{"type":"output_text","text":"second","annotations":[]}]}]}).to_string())
                };
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).as_bytes()).await?;
                socket.write_all(body.as_bytes()).await?;
                socket.flush().await?;
            }
            Ok::<_, ReactError>(requests)
        });
        let client = ResponsesClient::new(LlmConfig {
            provider_name: Some("actual-provider".into()),
            api_protocol: LlmApiProtocol::Responses,
            base_url: format!("http://{address}"),
            api_key: "fixture-key".into(),
            model: "gpt-test".into(),
            input_modalities: echo_core::llm::ModelInputModality::all_supported(),
            thinking_protocol: echo_core::llm::ThinkingProtocol::OpenaiReasoningEffort,
            timeouts: echo_core::llm::LlmTimeouts::default(),
        })?;
        let result = async {
            let mut stream = client
                .chat_stream(ChatRequest::new(vec![Message::user("first".into())]))
                .await?;
            let mut blocks = Vec::new();
            let mut content = String::new();
            let mut tools = Vec::new();
            let mut finishes = 0;
            while let Some(chunk) = stream.next().await {
                let chunk = chunk?;
                finishes += usize::from(chunk.finish_reason.is_some());
                content.push_str(chunk.delta.content.as_deref().unwrap_or_default());
                blocks.extend(chunk.delta.assistant_blocks.unwrap_or_default());
                for tool in chunk.delta.tool_calls.unwrap_or_default() {
                    let function = tool
                        .function
                        .ok_or_else(|| invalid_response("tool function missing"))?;
                    tools.push(ToolCall {
                        provider_source: tool.provider_source,
                        id: tool.id.ok_or_else(|| invalid_response("tool id missing"))?,
                        call_type: "function".into(),
                        function: FunctionCall {
                            name: function
                                .name
                                .ok_or_else(|| invalid_response("tool name missing"))?,
                            arguments: function
                                .arguments
                                .ok_or_else(|| invalid_response("tool args missing"))?,
                        },
                    });
                }
            }
            assert_eq!(finishes, 1);
            assert_eq!(content, "前缀终🙂");
            let mut message = Message::assistant(content);
            message.assistant_blocks = Some(blocks);
            message.tool_calls = Some(tools);
            let restored: Message = serde_json::from_value(serde_json::to_value(message)?)?;
            client
                .chat(ChatRequest::new(vec![
                    restored,
                    Message::tool_result("call-A".into(), "inspect".into(), "result".into()),
                ]))
                .await?;
            Ok::<_, ReactError>(())
        }
        .await;
        if result.is_err() {
            server.abort();
            return result;
        }
        let requests = server
            .await
            .map_err(|error| invalid_response(error.to_string()))??;
        let input = requests
            .get(1)
            .and_then(|request| request.get("input"))
            .and_then(Value::as_array)
            .ok_or_else(|| invalid_response("next HTTP input missing"))?;
        assert_eq!(
            input.iter().take(3).cloned().collect::<Vec<_>>(),
            full()
                .get("output")
                .and_then(Value::as_array)
                .cloned()
                .ok_or_else(|| invalid_response("fixture output missing"))?
        );
        Ok(())
    }

    #[tokio::test]
    async fn real_http_late_actual_id_empty_source_closure_next_request_keeps_full_opaque_and_text()
    -> Result<()> {
        tokio::time::timeout(Duration::from_secs(10), replay_http())
            .await
            .map_err(|_| invalid_response("HTTP fixture timeout"))??;
        Ok(())
    }
}

#[cfg(test)]
mod responses_replay_owner_r03_tests {
    use super::*;
    use echo_core::error::ReactError;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn full() -> Value {
        json!({"id":"history-native","status":"completed","model":"native-history-model","output":[
            {"type":"message","id":"history-message","role":"assistant","phase":"commentary","content":[{"type":"output_text","text":"原文🙂","annotations":[]}]},
            {"type":"reasoning","id":"history-reason","encrypted_content":"opaque-history","summary":[{"type":"summary_text","text":"摘要🙂"}],"content":[{"type":"reasoning_text","text":"公开原文"}]},
            {"type":"function_call","id":"history-fc","call_id":"history-call","name":"inspect","arguments":"{}"}
        ]})
    }

    fn split_message() -> Result<Message> {
        let mut message = response_to_chat_for_provider(full(), "historical-vendor")?.message;
        let blocks = message
            .assistant_blocks
            .take()
            .ok_or_else(|| invalid_response("blocks missing"))?;
        let mut observed = Vec::new();
        for block in blocks {
            let mut prefix = block.clone();
            prefix.provider_source = None;
            prefix.complete = false;
            let mut closure = block;
            closure.delta.clear();
            closure.reasoning = None;
            observed.extend([prefix, closure]);
        }
        message.assistant_blocks = Some(observed);
        Ok(message)
    }

    fn altered(case: &str) -> Result<Message> {
        let mut message = split_message()?;
        if case == "foreign-middle-source" {
            let blocks = message
                .assistant_blocks
                .as_mut()
                .ok_or_else(|| invalid_response("blocks missing"))?;
            let mut middle = blocks
                .get(1)
                .cloned()
                .ok_or_else(|| invalid_response("closure missing"))?;
            middle
                .provider_source
                .as_mut()
                .ok_or_else(|| invalid_response("source missing"))?
                .protocol = LlmApiProtocol::Anthropic;
            blocks.insert(1, middle);
        } else if case == "foreign-provider-then-cleared" {
            let blocks = message
                .assistant_blocks
                .as_mut()
                .ok_or_else(|| invalid_response("blocks missing"))?;
            blocks
                .get_mut(0)
                .ok_or_else(|| invalid_response("prefix missing"))?
                .provider = Some("foreign-provider".into());
            blocks
                .get_mut(1)
                .ok_or_else(|| invalid_response("closure missing"))?
                .provider = None;
        } else if case == "whole-group-owner-swapped" {
            for block in message.assistant_blocks.iter_mut().flatten() {
                if let Some(source) = block.provider_source.as_mut() {
                    source.request_id = "foreign-native-owner".into();
                }
            }
            for tool in message.tool_calls.iter_mut().flatten() {
                if let Some(source) = tool.provider_source.as_mut() {
                    source.request_id = "foreign-native-owner".into();
                }
            }
        } else if case == "observation-protocol-conflict" {
            message
                .provider_response
                .as_mut()
                .ok_or_else(|| invalid_response("observation missing"))?
                .protocol = LlmApiProtocol::Anthropic;
        } else if case == "whole-group-provider-swapped" {
            for block in message.assistant_blocks.iter_mut().flatten() {
                if block.block_id == "history-message:content:0" {
                    block.provider = Some("foreign-provider".into());
                }
            }
        } else {
            return Err(invalid_response("unknown case"));
        }
        serde_json::from_value(serde_json::to_value(message)?).map_err(Into::into)
    }

    #[test]
    fn canonical_group_collects_foreign_middle_and_provider_rollback_before_filtering() -> Result<()>
    {
        for case in ["foreign-middle-source", "foreign-provider-then-cleared"] {
            assert!(
                messages_to_input(&[altered(case)?]).is_err(),
                "中间观察不能被protocol/provider过滤吞掉:{case}"
            );
        }
        Ok(())
    }

    #[test]
    fn replay_requires_same_d1_native_owner_protocol_and_recorded_vendor() -> Result<()> {
        for case in [
            "whole-group-owner-swapped",
            "observation-protocol-conflict",
            "whole-group-provider-swapped",
        ] {
            assert!(
                messages_to_input(&[altered(case)?]).is_err(),
                "DTO观察与source冲突不得进入历史:{case}"
            );
        }
        Ok(())
    }

    #[test]
    fn runtime_never_upgraded_groups_are_excluded_and_none_to_some_keeps_original_payload()
    -> Result<()> {
        let mut message = split_message()?;
        let diagnostic = AssistantMessageBlock {
            provider_source: None,
            block_id: "runtime-diagnostic".into(),
            request_id: "runtime-owner".into(),
            sequence: 99,
            phase: AssistantPhase::Commentary,
            delta: "MUST_NOT_ENTER_WIRE".into(),
            complete: true,
            provider: Some("runtime".into()),
            reasoning: None,
        };
        message
            .assistant_blocks
            .as_mut()
            .ok_or_else(|| invalid_response("blocks missing"))?
            .push(diagnostic.clone());
        message.approved_assistant_blocks = Some(vec![diagnostic]);
        let restored: Message = serde_json::from_value(serde_json::to_value(message)?)?;
        assert_eq!(
            messages_to_input(&[restored])?,
            full()
                .get("output")
                .cloned()
                .ok_or_else(|| invalid_response("fixture output missing"))?
        );
        Ok(())
    }

    pub(super) fn client(url: String) -> Result<ResponsesClient> {
        ResponsesClient::new(LlmConfig {
            provider_name: Some("current-client-alias".into()),
            api_protocol: LlmApiProtocol::Responses,
            base_url: url,
            api_key: "fixture-key".into(),
            model: "gpt-test".into(),
            input_modalities: echo_core::llm::ModelInputModality::all_supported(),
            thinking_protocol: echo_core::llm::ThinkingProtocol::OpenaiReasoningEffort,
            timeouts: echo_core::llm::LlmTimeouts::default(),
        })
    }

    pub(super) async fn read_request(socket: &mut tokio::net::TcpStream) -> Result<Value> {
        let mut bytes = Vec::new();
        loop {
            if socket.read_buf(&mut bytes).await? == 0 {
                return Err(invalid_response("HTTP request truncated"));
            }
            let text = match std::str::from_utf8(&bytes) {
                Ok(text) => text,
                Err(error) if error.error_len().is_none() => continue,
                Err(error) => return Err(invalid_response(error.to_string())),
            };
            if let Some((headers, body)) = text.split_once("\r\n\r\n") {
                let length = headers
                    .lines()
                    .find_map(|line| {
                        line.split_once(':')
                            .filter(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                            .map(|(_, value)| value.trim().parse::<usize>())
                    })
                    .transpose()
                    .map_err(|error| invalid_response(error.to_string()))?
                    .ok_or_else(|| invalid_response("Content-Length missing"))?;
                if body.len() >= length {
                    return serde_json::from_str(body).map_err(Into::into);
                }
            }
        }
    }

    #[tokio::test]
    async fn cold_dto_foreign_source_and_d1_conflicts_never_send_real_http() -> Result<()> {
        for case in [
            "foreign-middle-source",
            "foreign-provider-then-cleared",
            "whole-group-owner-swapped",
            "observation-protocol-conflict",
            "whole-group-provider-swapped",
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").await?;
            let address = listener.local_addr()?;
            let server = tokio::spawn(async move {
                match tokio::time::timeout(Duration::from_millis(150), listener.accept()).await {
                    Err(_) => Ok::<_, ReactError>(false),
                    Ok(accept) => {
                        let (mut socket, _) = accept?;
                        read_request(&mut socket).await?;
                        let body=json!({"id":"negative-control-response","status":"completed","output":[{"type":"message","id":"negative-control-message","role":"assistant","content":[{"type":"output_text","text":"done","annotations":[]}]}]}).to_string();
                        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).as_bytes()).await?;
                        socket.write_all(body.as_bytes()).await?;
                        socket.flush().await?;
                        Ok(true)
                    }
                }
            });
            let result = client(format!("http://{address}"))?
                .chat(ChatRequest::new(vec![altered(case)?]))
                .await;
            let connected = server
                .await
                .map_err(|error| invalid_response(error.to_string()))??;
            assert!(
                matches!(result,Err(ReactError::Llm(ref error))if matches!(error.as_ref(),LlmError::InvalidResponse(_))),
                "cold DTO冲突必须在请求构造时拒绝:{case}"
            );
            assert!(!connected, "冲突不得实际连HTTP:{case}");
        }
        Ok(())
    }

    #[tokio::test]
    async fn cold_dto_real_http_keeps_history_vendor_without_binding_current_client_alias()
    -> Result<()> {
        tokio::time::timeout(Duration::from_secs(10),async {
            let listener=TcpListener::bind("127.0.0.1:0").await?;let address=listener.local_addr()?;
            let server=tokio::spawn(async move {
                let (mut socket,_)=listener.accept().await?;let request=read_request(&mut socket).await?;
                let body=json!({"id":"new-native","status":"completed","output":[{"type":"message","id":"new-message","role":"assistant","content":[{"type":"output_text","text":"done","annotations":[]}]}]}).to_string();
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).as_bytes()).await?;socket.write_all(body.as_bytes()).await?;socket.flush().await?;Ok::<_,ReactError>(request)
            });
            let restored:Message=serde_json::from_value(serde_json::to_value(split_message()?)?)?;
            let result=client(format!("http://{address}"))?.chat(ChatRequest::new(vec![restored])).await;
            if let Err(error)=result{server.abort();return Err(error);}
            let request=server.await.map_err(|error|invalid_response(error.to_string()))??;
            assert_eq!(request.get("input"),full().get("output"));
            assert!(request.get("input").and_then(Value::as_array).is_some_and(|items|items.iter().all(|item|item.get("provider_response").is_none())));Ok(())
        }).await.map_err(|_|invalid_response("HTTP fixture timeout"))?
    }
}

#[cfg(test)]
mod responses_late_source_projection_r03_tests {
    use super::responses_replay_owner_r03_tests::{client, read_request};
    use super::*;
    use echo_core::agent::{
        AgentOutputProjection, AgentPhase, ExecutionStatus, ExecutionStatusKind,
    };
    use echo_core::error::ReactError;
    use std::time::Duration;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

    async fn provider_to_core(known_first: bool) -> Result<()> {
        let mut events = Vec::new();
        if known_first {
            events.push(json!({"type":"response.created","response":{"id":"actual-r03","status":"in_progress"}}));
        }
        events.extend([
            json!({"type":"response.reasoning_summary_text.delta","item_id":"reason-r03","output_index":0,"summary_index":0,"delta":"摘要"}),
            json!({"type":"response.output_item.done","output_index":0,"item":{"type":"reasoning","id":"reason-r03","summary":[{"type":"summary_text","text":"摘要🙂"}]}}),
            json!({"type":"response.output_item.added","output_index":2,"item":{"type":"function_call","id":"fc-r03","call_id":"call-r03","name":"inspect","arguments":"{}"}}),
            json!({"type":"response.completed","response":{"id":"actual-r03","status":"completed","output":[{"type":"reasoning","id":"reason-r03","encrypted_content":"opaque-r03","summary":[{"type":"summary_text","text":"摘要🙂"}]},{"type":"message","id":"message-r03","role":"assistant","content":[{"type":"output_text","text":"正文","annotations":[]}]},{"type":"function_call","id":"fc-r03","call_id":"call-r03","name":"inspect","arguments":"{}"}]}}),
        ]);
        let payload = events
            .iter()
            .map(|event| {
                format!(
                    "event: {}\ndata: {event}\n\n",
                    event
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                )
            })
            .collect::<String>();
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await?;
            read_request(&mut socket).await?;
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",payload.len()).as_bytes()).await?;
            socket.write_all(payload.as_bytes()).await?;
            socket.flush().await?;
            Ok::<_, ReactError>(())
        });
        let result = async {
            let mut projection = AgentOutputProjection::default();
            projection.observe_status(&ExecutionStatus {
                adapter_generated: false,
                execution_id: "actual-test-execution".into(),
                request_id: None,
                sequence: 0,
                phase: AgentPhase::Thinking,
                status: ExecutionStatusKind::Started,
                reason: None,
            })?;
            let mut stream = client(format!("http://{address}"))?
                .chat_stream(ChatRequest::new(vec![Message::user("request".into())]))
                .await?;
            let mut displayed = String::new();
            let mut body = String::new();
            let mut sequences = Vec::new();
            let mut names = Vec::new();
            let mut arguments = String::new();
            let mut terminals = 0;
            while let Some(chunk) = stream.next().await {
                let chunk = chunk?;
                terminals += usize::from(chunk.finish_reason.is_some());
                body.push_str(chunk.delta.content.as_deref().unwrap_or_default());
                for block in chunk.delta.assistant_blocks.as_deref().unwrap_or_default() {
                    sequences.push(block.sequence);
                    if let Some(delta) = projection.observe_block(block)?
                        && block.phase == AssistantPhase::Reasoning
                    {
                        displayed.push_str(&delta.text);
                    }
                }
                for call in chunk.delta.tool_calls.as_deref().unwrap_or_default() {
                    if let Some(function) = call.function.as_ref() {
                        if let Some(name) = function.name.as_deref() {
                            names.push(name.to_string());
                        }
                        arguments.push_str(function.arguments.as_deref().unwrap_or_default());
                    }
                }
            }
            assert_eq!(terminals, 1);
            assert_eq!(
                displayed, "摘要🙂",
                "真实provider→当前Core7be投影不能parent先显示全摘要后才搬childprefix"
            );
            assert_eq!(body, "正文");
            assert_eq!(names, vec!["inspect"]);
            assert_eq!(arguments, "{}");
            assert!(
                sequences
                    .windows(2)
                    .all(|pair| pair.first().zip(pair.get(1)).is_none_or(|(a, b)| a < b)),
                "重排后global sequence仍严格递增"
            );
            Ok::<_, ReactError>(())
        }
        .await;
        if result.is_err() {
            server.abort();
            return result;
        }
        server
            .await
            .map_err(|error| invalid_response(error.to_string()))??;
        result
    }

    #[tokio::test]
    async fn real_provider_late_id_child_source_update_precedes_dependent_opaque_parent_in_public_core_projection()
    -> Result<()> {
        tokio::time::timeout(Duration::from_secs(10), provider_to_core(false))
            .await
            .map_err(|_| invalid_response("HTTP fixture timeout"))??;
        Ok(())
    }

    #[tokio::test]
    async fn known_id_provider_sequence_keeps_public_core_reasoning_and_tool_output_once()
    -> Result<()> {
        tokio::time::timeout(Duration::from_secs(10), provider_to_core(true))
            .await
            .map_err(|_| invalid_response("HTTP fixture timeout"))??;
        Ok(())
    }
}

#[cfg(test)]
mod cache_prefix_wire_contract {
    use super::*;
    #[test]
    fn three_growing_prompts_keep_wire_prefix() -> Result<()> {
        let client = ResponsesClient::new(LlmConfig::for_provider(
            "openai",
            "https://api.openai.com/v1",
            "synthetic",
            "gpt-4.1",
            echo_core::llm::LlmApiProtocol::Responses,
        )?)?;
        super::super::assert_cache_wire_growth(|request| client.request_body(request, false))
    }
}
