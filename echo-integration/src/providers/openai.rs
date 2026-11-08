use echo_core::error::{LlmError, ReactError, Result};
use echo_core::llm::capabilities::{
    ModelFactInputs, ModelProfileResolution, ModelProfileResolver, ProviderCapabilityOverride,
};
use echo_core::llm::types::{ChatCompletionRequest, ContentPart, Message, MessageContent};
use echo_core::llm::{ChatChunk, ChatRequest, ChatResponse, LlmClient};
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use reqwest::Client;
use reqwest::header::HeaderMap;
use std::sync::Arc;
use tracing::{Instrument, info_span};

use super::client::{ensure_request_not_cancelled, post_json, stream_post_json};
use super::client::{
    incomplete_chat_response, observe_chat_response, reported_provider_error,
    strip_chat_observation_fields,
};
use super::config::LlmConfig;
use super::native_message::{blocks_from_message, output_source};
use super::thinking_translate::translate_chat_thinking;

fn token_limits(model: &str, provider: &str, limit: Option<u32>) -> (Option<u32>, Option<u32>) {
    let lower = model.to_ascii_lowercase();
    if matches!(provider, "openai" | "azure-openai")
        && (lower.starts_with("gpt-5")
            || lower.starts_with("o1")
            || lower.starts_with("o3")
            || lower.starts_with("o4"))
    {
        (None, limit)
    } else {
        (limit, None)
    }
}

// ── Convenience Functions ─────────────────────────────────────────────────────

/// 保留历史中的原协议 reasoning_content，移除仅属 Framework 的观察字段。
/// Chat 的 tool/system/assistant content 只允许文本；富工具结果由既有
/// runtime 另发 user multimodal 消息，直接调用 LlmClient 时不能静默丢媒体。
fn normalize_messages(messages: Vec<Message>) -> Result<Vec<Message>> {
    normalize_messages_for_vendor(messages, false)
}

fn normalize_messages_for_vendor(
    messages: Vec<Message>,
    deepseek_rich: bool,
) -> Result<Vec<Message>> {
    messages
        .into_iter()
        .filter(|message| !message.is_provider_response_observation_only())
        .map(|mut msg| {
            // 空原生Source ID只属于D2 partial；剥字段前拒绝，防止partial
            // 被转换成可回放的完整工具。外部完整调用的provider_source=None仍合法。
            if msg.tool_calls.as_deref().unwrap_or_default().iter().any(|call|
                call.provider_source.as_ref().is_some_and(|source| source.request_id.is_empty())) {
                return Err(request_error("D2 partial tool source cannot be replayed in Chat history"));
            }
            msg.provider_response = None;
            msg.reasoning_blocks = None;
            msg.assistant_blocks = None;
            msg.execution_status = None;
            msg.approved_assistant_blocks = None;
            msg.tool_result_is_error = None;
            for tool in msg.tool_calls.as_mut().into_iter().flatten() {
                tool.provider_source = None;
            }
            let role = msg.role.as_str();
            if !matches!(role, "user" | "assistant" | "tool" | "system" | "developer") {
                return Err(request_error(format!("unsupported message role '{role}'")));
            }
            if deepseek_rich && role == "developer" {
                return Err(request_error("DeepSeek Chat has no developer role; use system explicitly"));
            }
            if role == "tool" && msg.tool_call_id.as_deref().is_none_or(str::is_empty) {
                return Err(request_error("tool message requires tool_call_id"));
            }
            if let MessageContent::Parts(parts) = &mut msg.content {
                let mut rewritten = Vec::with_capacity(parts.len());
                for part in parts.drain(..) {
                    rewritten.push(match part {
                        ContentPart::ResourceLink { resource } => ContentPart::Text {
                            text: resource.model_text(),
                        },
                        ContentPart::ImageUrl { .. } | ContentPart::File { .. }
                            if role != "user" && !(role == "tool" && deepseek_rich) => {
                                return Err(request_error(format!(
                                    "{role} content supports text parts only; image/file requires a separate user multimodal message"
                                )));
                            }
                        other => other,
                    });
                }
                msg.content = MessageContent::Parts(rewritten);
            }
            if deepseek_rich && matches!(role, "assistant" | "system")
                && let MessageContent::Parts(parts) = &msg.content
            {
                let text = parts.iter().filter_map(|part| match part {
                    ContentPart::Text { text } => Some(text.as_str()), _ => None,
                }).collect::<String>();
                msg.content = MessageContent::Text(text);
            }
            Ok(msg)
        })
        .collect()
}

fn request_error(message: impl Into<String>) -> ReactError {
    LlmError::SerializationError(format!("Chat Completions request: {}", message.into())).into()
}

/// Core ContentPart 保持 provider-neutral；只在 Chat 请求的 wire 边界转换
/// file 的 filename/file_data 嵌套，不改变文件持久化格式或原始 base64。
#[derive(Clone, Copy)]
enum ChatFileDialect {
    TextOnly,
    OpenAiPdf,
    DeepseekImage,
}

fn normalize_wire_parts(body: &mut serde_json::Value, dialect: ChatFileDialect) -> Result<()> {
    let messages = body
        .get_mut("messages")
        .and_then(serde_json::Value::as_array_mut)
        .ok_or_else(|| request_error("messages must be an array"))?;
    for message in messages {
        if let Some(parts) = message
            .get_mut("content")
            .and_then(serde_json::Value::as_array_mut)
        {
            for part in parts {
                if part.get("type").and_then(serde_json::Value::as_str) != Some("file") {
                    continue;
                }
                let name = part
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| request_error("file requires a filename"))?;
                let content = part
                    .get("content")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| request_error("file requires base64 content"))?;
                let bytes =
                    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, content)
                        .map_err(|_| request_error("file content is invalid base64"))?;
                if matches!(dialect, ChatFileDialect::DeepseekImage) {
                    let ext = name
                        .rsplit('.')
                        .next()
                        .unwrap_or_default()
                        .to_ascii_lowercase();
                    let mime = match ext.as_str() {
                        "png" => "image/png",
                        "jpg" | "jpeg" => "image/jpeg",
                        "gif" => "image/gif",
                        "webp" => "image/webp",
                        _ => {
                            return Err(request_error(
                                "DeepSeek Chat native file input requires a JPEG/PNG/GIF/WebP image",
                            ));
                        }
                    };
                    *part = serde_json::json!({"type":"file","filename":name,"file_data":format!("data:{mime};base64,{content}")});
                } else if is_text_class_filename(name) {
                    let decoded = String::from_utf8(bytes)
                        .map_err(|_| request_error("text attachment is not valid UTF-8"))?;
                    *part = serde_json::json!({"type":"text","text":format!("[Attachment: {name}]\n{decoded}")});
                } else if matches!(dialect, ChatFileDialect::OpenAiPdf)
                    && name.to_ascii_lowercase().ends_with(".pdf")
                {
                    *part = serde_json::json!({"type":"file","file":{
                        "filename":name,"file_data":format!("data:application/pdf;base64,{content}")
                    }});
                } else {
                    return Err(request_error(
                        "binary file input is not supported by this exact Chat provider/model; original file was not replaced by a placeholder",
                    ));
                }
            }
        }
    }
    Ok(())
}

fn validate_tool_choice(request: &ChatRequest) -> Result<()> {
    use echo_core::llm::ToolChoice;
    let tools = request.tools.as_deref().unwrap_or_default();
    match request.tool_choice.as_ref() {
        Some(ToolChoice::Required | ToolChoice::Function { .. }) if tools.is_empty() => {
            return Err(request_error(
                "required/function tool_choice requires exposed tools",
            ));
        }
        _ => {}
    }
    if let Some(ToolChoice::Function { name }) = &request.tool_choice
        && (name.trim().is_empty() || !tools.iter().any(|tool| tool.function.name == *name))
    {
        return Err(request_error(
            "selected function must be nonempty and present in exposed tools",
        ));
    }
    Ok(())
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
    model_profile_resolver: ModelProfileResolver,
    header_map: HeaderMap,
}

impl OpenAiClient {
    /// Create a client with a custom configuration
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

    /// Create a client with a shared HTTP client
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

    pub(crate) fn with_model_profile_resolver(mut self, resolver: ModelProfileResolver) -> Self {
        self.model_profile_resolver = resolver;
        self
    }

    /// 流/非流共用一份请求构造与验证，防止模式切换丢失工具或媒体合同。
    fn request_body(&self, request: &ChatRequest, streaming: bool) -> Result<serde_json::Value> {
        ensure_request_not_cancelled(request.cancel_token.as_ref())?;
        self.config.validate_input_modalities(&request.messages)?;
        validate_tool_choice(request)?;
        let provider = self
            .config
            .provider_name
            .as_deref()
            .unwrap_or("")
            .to_ascii_lowercase();
        let model = self.config.model.to_ascii_lowercase();
        if !streaming
            && matches!(
                provider.as_str(),
                "dashscope" | "qwen" | "aliyun" | "alibaba" | "modelstudio" | "bailian"
            )
            && matches!(model.as_str(), "qwen3.5-122b-a10b" | "qwen3.5-27b")
        {
            return Err(request_error(
                "this exact DashScope model supports streaming only; use chat_stream",
            ));
        }

        let openai_model = matches!(provider.as_str(), "openai" | "azure-openai")
            && (model.starts_with("gpt-4o")
                || model.starts_with("gpt-4.1")
                || model.starts_with("gpt-5")
                || model.starts_with("o3")
                || model.starts_with("o4"));
        let deepseek_rich = provider == "deepseek"
            && matches!(model.as_str(), "deepseek-flash" | "deepseek-v4-pro");
        if provider == "deepseek" {
            use echo_core::llm::{ThinkingConfig, ThinkingLevel, ToolChoice};
            let disabled = matches!(
                request.thinking,
                Some(ThinkingConfig::Disabled | ThinkingConfig::Level(ThinkingLevel::None))
            );
            if matches!(
                request.tool_choice,
                Some(ToolChoice::Required | ToolChoice::Function { .. })
            ) && !disabled
            {
                return Err(request_error(
                    "DeepSeek Chat required/function tool_choice needs explicitly disabled thinking (default is enabled)",
                ));
            }
            if matches!(
                request.response_format,
                Some(echo_core::llm::types::ResponseFormat::JsonSchema { .. })
            ) {
                return Err(request_error(
                    "DeepSeek Chat response_format supports text/json_object, not json_schema",
                ));
            }
        }
        let tools = request.tools.as_deref().unwrap_or_default();
        let strict_requested = tools.iter().any(|tool| tool.function.strict == Some(true));
        let deepseek_beta = provider == "deepseek"
            && reqwest::Url::parse(&self.config.base_url)
                .ok()
                .is_some_and(|url| url.path().split('/').any(|segment| segment == "beta"));
        // 函数strict与最终JSON schema是不同能力。OpenAI工具协议依据既有
        // resolved tool_support；DeepSeek严格工具另需官方beta入口且全部严格。
        let strict_tools = matches!(provider.as_str(), "openai" | "azure-openai")
            && self
                .model_profile_resolution()
                .profile
                .capabilities
                .tool_support;
        if strict_requested && !(strict_tools || deepseek_beta) {
            return Err(request_error(
                "strict function arguments are unsupported for the resolved Chat provider/model/endpoint",
            ));
        }
        if strict_requested
            && deepseek_beta
            && tools.iter().any(|tool| tool.function.strict != Some(true))
        {
            return Err(request_error(
                "DeepSeek beta strict mode requires strict:true on every exposed function",
            ));
        }
        let t = translate_chat_thinking(
            &self.config.model,
            &provider,
            self.model_profile_resolution().profile.thinking_protocol,
            &request.thinking,
        )?;
        if t.drop_temperature && request.temperature.is_some() {
            return Err(request_error(
                "temperature is not supported with this reasoning model/mode; omit it explicitly",
            ));
        }
        let (max_tokens, max_completion_tokens) =
            token_limits(&self.config.model, &provider, request.max_tokens);
        let req = ChatCompletionRequest {
            model: self.config.model.clone(),
            messages: if deepseek_rich {
                normalize_messages_for_vendor(request.messages.clone(), true)?
            } else {
                normalize_messages(request.messages.clone())?
            },
            temperature: if t.drop_temperature {
                None
            } else {
                request.temperature
            },
            max_tokens,
            max_completion_tokens,
            stream: streaming.then_some(true),
            stream_options: streaming.then(|| serde_json::json!({"include_usage":true})),
            tools: request.tools.clone(),
            tool_choice: request.tool_choice.clone(),
            response_format: request.response_format.clone(),
            reasoning_effort: t.reasoning_effort,
            enable_thinking: t.enable_thinking,
            thinking_budget: t.thinking_budget,
            thinking_type: t.thinking_type,
            think: t.ollama_think,
            user_id: request.user_id.clone(),
        };
        let mut body = serde_json::to_value(req)
            .map_err(|error| LlmError::SerializationError(error.to_string()))?;
        let dialect = if deepseek_rich {
            ChatFileDialect::DeepseekImage
        } else if openai_model {
            ChatFileDialect::OpenAiPdf
        } else {
            ChatFileDialect::TextOnly
        };
        normalize_wire_parts(&mut body, dialect)?;
        echo_core::llm::cache::wire::log_wire_prompt(&body, "chat");
        Ok(body)
    }

    fn build_http_client() -> Client {
        Client::new()
    }
}

// 同一生产map，无新取消/终态权威；只提供provider模块内部的确定性组件接缝。
pub(super) fn adapt_openai_stream<S>(
    stream: S,
    provider: String,
) -> BoxStream<'static, Result<ChatChunk>>
where
    S: futures::Stream<Item = Result<echo_core::llm::types::ChatCompletionChunk>> + Send + 'static,
{
    let mut request_id = String::new();
    let mut sequence = 0_u64;
    let mut observed_blocks = std::collections::BTreeMap::new();
    let mut tool_ids = std::collections::BTreeMap::new();
    Box::pin(futures::StreamExt::map(stream, move |result| {
        result.map(|chunk| {
            if request_id.is_empty() {
                request_id = if chunk.id.is_empty() {
                    super::native_message::fallback_request_id("chat")
                } else {
                    chunk.id.clone()
                };
            }
            let choice = chunk.choices.first();
            let mut delta = choice.map(|c| c.delta.clone()).unwrap_or_default();
            delta.assistant_blocks = None;
            delta.reasoning_blocks = None;
            let native_request_id = delta
                .provider_response
                .as_ref()
                .and_then(|record| record.request_id.clone());
            for tool in delta.tool_calls.as_mut().into_iter().flatten() {
                tool.provider_source = None;
                if let Some(id) = &tool.id {
                    tool_ids.insert(tool.index, id.clone());
                }
                if let (Some(id), Some(native_id)) =
                    (tool_ids.get(&tool.index), native_request_id.as_deref())
                {
                    tool.provider_source = Some(output_source(
                        echo_core::llm::LlmApiProtocol::ChatCompletions,
                        native_id,
                        id,
                        tool.index,
                        None,
                        "tool_calls",
                        None,
                    ));
                }
            }
            let finish_reason = choice.and_then(|c| c.finish_reason.clone());
            let message = Message {
                content: delta
                    .content
                    .clone()
                    .map(MessageContent::Text)
                    .unwrap_or_default(),
                reasoning_content: delta.reasoning_content.clone(),
                reasoning_blocks: delta.reasoning_blocks.clone(),
                ..Default::default()
            };
            let mut blocks = blocks_from_message(&message, &request_id, &provider, false);
            for block in &mut blocks {
                // 原有观察map保留已解码坐标（不保假原生ID），供晚到真实ID
                // 在闭合增量中补全；不从正文或phase重新猜来源。
                let source_coordinates = block.provider_source.as_ref().map(|source| {
                    (
                        source.item_id.clone(),
                        source.output_index,
                        source.content_index,
                        source.content_kind.clone(),
                        source.phase.clone(),
                    )
                });
                // canonical block.request_id仍可使用本地稳定scope；原生Source
                // 只放真实服务ID，未观察到时None，不把fallback伪装为原件。
                if let Some(native_id) = native_request_id.as_ref() {
                    if let Some(source) = block.provider_source.as_mut() {
                        source.request_id = native_id.clone();
                    }
                } else {
                    block.provider_source = None;
                }
                block.sequence = sequence;
                sequence = sequence.saturating_add(1);
                observed_blocks.insert(block.block_id.clone(), (block.clone(), source_coordinates));
            }
            if finish_reason.is_some() {
                for (block, coordinates) in observed_blocks.values() {
                    let mut closed = block.clone();
                    if let (
                        Some(native_id),
                        Some((item_id, output_index, content_index, content_kind, phase)),
                    ) = (native_request_id.as_ref(), coordinates)
                    {
                        closed.provider_source =
                            Some(echo_core::llm::types::ProviderOutputSource {
                                protocol: echo_core::llm::LlmApiProtocol::ChatCompletions,
                                request_id: native_id.clone(),
                                item_id: item_id.clone(),
                                output_index: *output_index,
                                content_index: *content_index,
                                content_kind: content_kind.clone(),
                                phase: phase.clone(),
                            });
                    }
                    closed.delta.clear();
                    closed.reasoning = None;
                    closed.complete = true;
                    closed.sequence = sequence;
                    sequence = sequence.saturating_add(1);
                    blocks.push(closed);
                }
            }
            if !blocks.is_empty() {
                delta.assistant_blocks = Some(blocks);
            }
            ChatChunk {
                delta,
                finish_reason,
                usage: chunk.usage.clone(),
            }
        })
    }))
}

impl LlmClient for OpenAiClient {
    fn chat(&self, request: ChatRequest) -> BoxFuture<'_, Result<ChatResponse>> {
        let model = self.config.model.clone();
        Box::pin(
            async move {
                let timeouts = request.timeouts.unwrap_or(self.config.timeouts);
                let body = self.request_body(&request, false)?;

                let mut native: serde_json::Value = post_json(
                    self.client.clone(),
                    body,
                    self.header_map.clone(),
                    &self.config.base_url,
                    timeouts,
                    request.cancel_token.clone(),
                )
                .await?;

                if let Some(error) = native.get("error") {
                    return Err(reported_provider_error(
                        self.config
                            .provider_name
                            .as_deref()
                            .unwrap_or("openai_compatible"),
                        error,
                    )
                    .into());
                }
                let observation = observe_chat_response(
                    self.config
                        .provider_name
                        .as_deref()
                        .unwrap_or("openai_compatible"),
                    &native,
                    None,
                )?;
                strip_chat_observation_fields(&mut native);
                let mut raw: echo_core::llm::types::ChatCompletionResponse =
                    serde_json::from_value(native)
                        .map_err(|error| LlmError::InvalidResponse(error.to_string()))?;
                let request_id = raw.id.clone();
                // Provider JSON can contain similarly named extension fields;
                // only the runtime may supply execution/approved observations.
                for choice in &mut raw.choices {
                    choice.message.execution_status = None;
                    choice.message.approved_assistant_blocks = None;
                    choice.message.assistant_blocks = None;
                    choice.message.reasoning_blocks = None;
                    choice.message.tool_result_is_error = None;
                    for (index, tool) in choice
                        .message
                        .tool_calls
                        .as_mut()
                        .into_iter()
                        .flatten()
                        .enumerate()
                    {
                        let index = u32::try_from(index).map_err(|_| {
                            LlmError::InvalidResponse("Chat tool index exceeds u32".into())
                        })?;
                        tool.provider_source = Some(output_source(
                            echo_core::llm::LlmApiProtocol::ChatCompletions,
                            &request_id,
                            &tool.id,
                            index,
                            None,
                            "tool_calls",
                            None,
                        ));
                    }
                }
                let choice = raw.choices.first().ok_or(LlmError::EmptyResponse)?;
                let mut message = choice.message.clone();
                message.provider_response = Some(observation);
                if !matches!(
                    choice.finish_reason.as_deref(),
                    Some("stop" | "tool_calls" | "function_call")
                ) {
                    return Err(incomplete_chat_response(message).into());
                }
                super::native_message::validate_completion(
                    &message,
                    choice.finish_reason.as_deref(),
                )?;
                if message
                    .provider_response
                    .as_ref()
                    .and_then(|record| record.request_id.as_deref())
                    .is_none_or(str::is_empty)
                {
                    return Err(incomplete_chat_response(message).into());
                }

                let assistant_blocks = blocks_from_message(
                    &message,
                    &request_id,
                    self.config
                        .provider_name
                        .as_deref()
                        .unwrap_or("openai_compatible"),
                    true,
                );
                if !assistant_blocks.is_empty() {
                    message.assistant_blocks = Some(assistant_blocks);
                }
                let response = ChatResponse {
                    message,
                    finish_reason: choice.finish_reason.clone(),
                    usage: raw.usage.clone(),
                    raw,
                };
                ensure_request_not_cancelled(request.cancel_token.as_ref())?;
                Ok(response)
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
                let timeouts = request.timeouts.unwrap_or(self.config.timeouts);
                let body = self.request_body(&request, true)?;
                let adapted_request = self
                    .client
                    .post(self.config.base_url.clone())
                    .headers(self.header_map.clone())
                    .json(&body);

                let stream = stream_post_json(
                    self.config
                        .provider_name
                        .clone()
                        .unwrap_or_else(|| "openai_compatible".into()),
                    adapted_request,
                    self.config.model.clone(),
                    timeouts,
                    request.cancel_token,
                )
                .await?;

                let provider = self
                    .config
                    .provider_name
                    .clone()
                    .unwrap_or_else(|| "openai_compatible".into());
                Ok(adapt_openai_stream(stream, provider))
            }
            .instrument(info_span!("openai_chat_stream", model = %model)),
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
        observation: &echo_core::llm::types::ProviderResponseMetadata,
        _previous: Option<&echo_core::llm::types::Usage>,
    ) -> Result<Option<echo_core::llm::types::Usage>> {
        if observation.protocol != echo_core::llm::LlmApiProtocol::ChatCompletions
            || observation.provider
                != self
                    .config
                    .provider_name
                    .as_deref()
                    .unwrap_or("openai_compatible")
        {
            return Ok(None);
        }
        let Some(raw) = observation.usage.as_ref().filter(|raw| raw.is_object()) else {
            return Ok(None);
        };
        let usage =
            serde_json::from_value::<echo_core::llm::types::Usage>(raw.clone()).map_err(|_| {
                LlmError::InvalidResponse("reported Chat usage is not representable".into())
            })?;
        Ok((usage.prompt_tokens.is_some()
            || usage.completion_tokens.is_some()
            || usage.total_tokens.is_some())
        .then_some(usage))
    }

    fn protocol_capabilities(&self) -> ProviderCapabilityOverride {
        ProviderCapabilityOverride::openai_chat_protocol()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reported_usage_observation_keeps_source_absence_and_reported_zero() -> Result<()> {
        let client = OpenAiClient::new(LlmConfig::for_provider(
            "usage-fixture",
            "https://example.invalid/v1/chat/completions",
            "fixture-key",
            "fixture-model",
            echo_core::llm::LlmApiProtocol::ChatCompletions,
        )?)?;
        let mut observation: echo_core::llm::types::ProviderResponseMetadata =
            serde_json::from_value(serde_json::json!({
                "protocol":"chat_completions", "provider":"usage-fixture", "request_id":"native-original",
                "usage":{"prompt_tokens":0,"completion_tokens":0,"total_tokens":0}
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

    use base64::Engine as _;
    use echo_core::llm::types::ChatCompletionChunk;

    fn multimodal_msg(parts: Vec<ContentPart>) -> Message {
        let mut msg = Message::user(String::new());
        msg.content = MessageContent::Parts(parts);
        msg
    }

    #[test]
    fn client_protocol_facts_do_not_follow_the_configured_provider_label()
    -> std::result::Result<(), String> {
        let openai = OpenAiClient::new(
            LlmConfig::for_provider(
                "openai",
                "https://api.openai.com/v1",
                "test-key",
                "gpt-test",
                echo_core::llm::LlmApiProtocol::ChatCompletions,
            )
            .map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        assert_eq!(
            openai.protocol_capabilities(),
            ProviderCapabilityOverride::openai_chat_protocol()
        );
        assert!(openai.capabilities().structured_output);
        assert!(openai.capabilities().tool_support);

        let custom = OpenAiClient::new(
            LlmConfig::for_provider(
                "custom",
                "https://gateway.example/v1",
                "test-key",
                "future-model",
                echo_core::llm::LlmApiProtocol::ChatCompletions,
            )
            .map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        assert_eq!(
            custom.protocol_capabilities(),
            openai.protocol_capabilities()
        );
        assert!(!custom.capabilities().structured_output);
        assert!(!custom.capabilities().tool_support);
        Ok(())
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
        let out = normalize_messages(vec![msg]).map_err(|error| error.to_string())?;
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
    fn files_are_lossless_or_rejected_at_the_wire_boundary() -> Result<()> {
        let b64 = base64::engine::general_purpose::STANDARD.encode("中文 notes🙂");
        let mut body = serde_json::json!({"messages":[{"role":"user","content":[
            {"type":"file","name":"notes.txt","content":b64}
        ]}]});
        normalize_wire_parts(&mut body, ChatFileDialect::TextOnly)?;
        assert_eq!(
            body.pointer("/messages/0/content/0/text")
                .and_then(serde_json::Value::as_str),
            Some("[Attachment: notes.txt]\n中文 notes🙂")
        );
        let b64 = base64::engine::general_purpose::STANDARD.encode(b"%PDF-1.7");
        let original = serde_json::json!({"messages":[{"role":"user","content":[{"type":"file","name":"paper.pdf","content":b64}]}]});
        assert!(normalize_wire_parts(&mut original.clone(), ChatFileDialect::TextOnly).is_err());
        let mut native = original;
        normalize_wire_parts(&mut native, ChatFileDialect::OpenAiPdf)?;
        assert_eq!(
            native
                .pointer("/messages/0/content/0/file/file_data")
                .and_then(serde_json::Value::as_str),
            Some(format!("data:application/pdf;base64,{b64}").as_str())
        );
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
        let out = normalize_messages(vec![msg]).map_err(|error| error.to_string())?;
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
        let out = normalize_messages(vec![msg]).map_err(|error| error.to_string())?;
        assert_eq!(
            out.first().and_then(|message| message.content.as_text()),
            Some("hello".to_string())
        );
        Ok(())
    }

    #[test]
    fn anthropic_reasoning_blocks_are_not_forwarded() -> std::result::Result<(), String> {
        use echo_core::llm::types::ReasoningBlock;

        let mut msg = Message::assistant("answer".to_string());
        msg.reasoning_blocks = Some(vec![ReasoningBlock::Signed {
            thinking: "private reasoning".to_string(),
            signature: "signature".to_string(),
        }]);

        let out = normalize_messages(vec![msg]).map_err(|error| error.to_string())?;
        assert!(
            out.first()
                .is_some_and(|message| message.reasoning_blocks.is_none())
        );
        Ok(())
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
    include!("openai_compat_tests.rs");
    include!("openai_resume_tests.rs");
    include!("openai_metadata_tests.rs");
    include!("openai_metadata_stream_tests.rs");
    include!("openai_metadata_d2_tests.rs");

    mod legacy_api_tests {
        include!("openai_legacy_api_tests.rs");
    }
}

#[cfg(test)]
mod cache_prefix_wire_contract {
    use super::*;
    #[test]
    fn three_growing_prompts_keep_wire_prefix() -> Result<()> {
        let client = OpenAiClient::new(LlmConfig::for_provider(
            "openai",
            "https://api.openai.com/v1",
            "synthetic",
            "gpt-4.1",
            echo_core::llm::LlmApiProtocol::ChatCompletions,
        )?)?;
        super::super::assert_cache_wire_growth(|request| client.request_body(request, false))
    }
}
