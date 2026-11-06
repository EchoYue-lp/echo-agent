//! Anthropic Messages API provider
//!
//! Implements [`LlmClient`] for Anthropic's `/v1/messages` endpoint.
//! System messages are sent as a top-level `system` field (not in the messages array).

use echo_core::agent::{AssistantMessageBlock, AssistantPhase};
use echo_core::error::{LlmError, Result};
use echo_core::llm::capabilities::{
    ModelFactSource, ModelProfileResolution, ModelProfileResolver, ProviderCapabilityOverride,
};
use echo_core::llm::types::{
    ChatCompletionResponse, ContentPart, DeltaFunctionCall, DeltaMessage, DeltaToolCall,
    FunctionCall, Message, MessageContent, ProviderResponseMetadata, ReasoningBlock,
    ResponseFormat, Role, ToolCall, Usage,
};
use echo_core::llm::{
    ChatChunk, ChatRequest, ChatResponse, LlmApiProtocol, LlmClient, LlmTimeouts,
    ModelInputModality, ThinkingProtocol, ToolChoice,
};
use futures::StreamExt;
use futures::future::BoxFuture;

use super::anthropic_cache::AnthropicCachePlan;
use super::client::{
    JsonSseEvent, ensure_request_not_cancelled, post_json_request, stream_json_sse,
};
use super::config::{protocol_fact_set, validate_model_input_modalities};
use super::native_message::output_source;
use futures::stream::BoxStream;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{Instrument, info_span};

pub struct AnthropicClient {
    client: Arc<Client>,
    api_key: String,
    model: String,
    base_url: String,
    provider_name: String,
    model_profile_resolver: ModelProfileResolver,
    input_modalities: Vec<ModelInputModality>,
    timeouts: LlmTimeouts,
}

impl AnthropicClient {
    /// 只转换协议事实；模型支持沿现有 sourced resolver，不在适配器建立第二份配置。
    fn validate_request_features(&self, request: &ChatRequest) -> Result<AnthropicControls> {
        validate_model_input_modalities(&self.model, &self.input_modalities, &request.messages)?;
        let invalid = |feature: &str, reason: &str| {
            LlmError::InvalidResponse(format!(
                "Anthropic {feature} is unsupported for model '{}': {reason}",
                self.model
            ))
        };
        let resolution = self.model_profile_resolution();
        let tools = request.tools.as_deref().unwrap_or_default();
        if !tools.is_empty() && !resolution.profile.supports_tools {
            return Err(invalid("tools", "resolved exact model disables tool support").into());
        }
        let tool_choice = request.tool_choice.as_ref().map(|choice| match choice {
            ToolChoice::Auto => AnthropicToolChoice::Auto,
            ToolChoice::None => AnthropicToolChoice::None,
            ToolChoice::Required => AnthropicToolChoice::Any,
            ToolChoice::Function { name } => AnthropicToolChoice::Tool { name: name.clone() },
        });
        if matches!(
            request.tool_choice,
            Some(ToolChoice::Required | ToolChoice::Function { .. })
        ) && tools.is_empty()
        {
            return Err(invalid("tool_choice", "required/function requires exposed tools").into());
        }
        if let Some(ToolChoice::Function { name }) = &request.tool_choice
            && (name.trim().is_empty() || !tools.iter().any(|tool| tool.function.name == *name))
        {
            return Err(invalid(
                "tool_choice",
                "selected function must be nonempty and exposed",
            )
            .into());
        }
        if let Some(choice) = request.tool_choice.as_ref() {
            self.require_tool_choice_support(&resolution, choice)?;
        }
        for tool in tools {
            if tool.tool_type != "function" || tool.function.name.trim().is_empty() {
                return Err(
                    invalid("tools", "only named client function tools are supported").into(),
                );
            }
            if tool.function.strict.is_some() {
                // final JSON Schema flag 不证明工具 strict；v1 无独立 strict fact，未知模型必须拒绝。
                self.require_strict_tool_support()?;
            }
        }
        let (thinking, effort) = if self.is_deepseek_anthropic() {
            build_deepseek_anthropic_thinking(&request.thinking)?
        } else {
            build_anthropic_thinking(
                &self.model,
                resolution.profile.thinking_protocol,
                &request.thinking,
                request.max_tokens.unwrap_or(4096),
            )?
        };
        let active_thinking = thinking
            .as_ref()
            .is_some_and(|value| value.block_type != "disabled")
            || (!self.is_deepseek_anthropic()
                && resolution.profile.thinking_protocol == ThinkingProtocol::AnthropicAdaptive)
            || (self.is_deepseek_anthropic() && request.thinking.is_none());
        if active_thinking
            && matches!(
                request.tool_choice,
                Some(ToolChoice::Required | ToolChoice::Function { .. })
            )
        {
            return Err(invalid(
                "thinking + tool_choice",
                "forced tool use cannot be combined with thinking",
            )
            .into());
        }
        if let Some(temperature) = request.temperature {
            let upper = if self.is_deepseek_anthropic() {
                2.0
            } else {
                1.0
            };
            if !temperature.is_finite() || !(0.0..=upper).contains(&temperature) {
                return Err(
                    invalid("temperature", "value is outside the native provider range").into(),
                );
            }
            // 不能删显式采样参数来制造请求成功；不同 thinking 方言没有统一可兑现的采样控制。
            if active_thinking && !self.is_deepseek_anthropic() {
                return Err(invalid("thinking + temperature", "explicit sampling with thinking is not verified; omit temperature or disable thinking").into());
            }
        }
        let format = match &request.response_format {
            None | Some(ResponseFormat::Text) => None,
            Some(ResponseFormat::JsonObject) => {
                return Err(invalid(
                    "response_format JsonObject",
                    "Messages has no proven native JSON-object-only mode; use JsonSchema",
                )
                .into());
            }
            Some(ResponseFormat::JsonSchema { json_schema }) => {
                self.require_schema_support(&resolution, "structured final output")?;
                if !json_schema.strict || !json_schema.schema.is_object() {
                    return Err(invalid(
                        "response_format JsonSchema",
                        "native json_schema requires a schema object and strict=true",
                    )
                    .into());
                }
                Some(AnthropicJsonFormat {
                    format_type: "json_schema",
                    schema: json_schema.schema.clone(),
                })
            }
        };
        if !self.is_deepseek_anthropic()
            && effort.is_some()
            && matches!(
                resolution.profile.thinking_protocol,
                ThinkingProtocol::AnthropicAdaptive
            )
        {
            return Err(invalid(
                "effort",
                "adaptive-only model effort controls need exact primary support evidence",
            )
            .into());
        }
        let output_config = (format.is_some() || effort.is_some())
            .then_some(AnthropicOutputConfig { format, effort });
        Ok(AnthropicControls {
            tool_choice,
            thinking,
            output_config,
        })
    }

    fn require_strict_tool_support(&self) -> Result<()> {
        if self.has_official_ga_schema_model() {
            return Ok(());
        }
        Err(LlmError::InvalidResponse("Anthropic strict tool input is unsupported/unverified for this exact endpoint/model; final structured_output capability is not evidence for tool strict".into()).into())
    }

    fn require_tool_choice_support(
        &self,
        resolution: &ModelProfileResolution,
        choice: &ToolChoice,
    ) -> Result<()> {
        let native = self.has_official_ga_schema_model()
            || (self.is_deepseek_anthropic()
                && (self.model.starts_with("claude")
                    || matches!(self.model.as_str(), "deepseek-flash" | "deepseek-v4-pro")));
        let exact = resolution.registered_facts.iter().any(|facts| {
            matches!(
                facts.metadata.source,
                ModelFactSource::ExactModel | ModelFactSource::CallerOverride
            ) && resolution.applied_facts.contains(&facts.metadata)
                && match choice {
                    ToolChoice::Auto => {
                        facts.capabilities.tool_support == Some(true)
                            || facts.profile.supports_tools == Some(true)
                    }
                    ToolChoice::None => {
                        facts.capabilities.supports_tool_choice_none == Some(true)
                            || facts.profile.supports_tool_choice_none == Some(true)
                    }
                    // 统一 v1 没有 forced-choice 单独能力事实；普通 tool_support 不授予强制选择。
                    ToolChoice::Required | ToolChoice::Function { .. } => false,
                }
        });
        let supported = match choice {
            ToolChoice::None => resolution.profile.supports_tool_choice_none,
            _ => resolution.profile.supports_tools,
        };
        if supported && (native || exact) {
            return Ok(());
        }
        Err(LlmError::InvalidResponse("Anthropic tool_choice unsupported/unverified for exact endpoint/model and selected choice".into()).into())
    }

    fn require_schema_support(
        &self,
        resolution: &ModelProfileResolution,
        feature: &str,
    ) -> Result<()> {
        if self.is_deepseek_anthropic() {
            return Err(LlmError::InvalidResponse(format!("DeepSeek Anthropic compatibility does not support {feature}; output_config supports effort only")).into());
        }
        let exact = resolution.registered_facts.iter().any(|facts| {
            matches!(
                facts.metadata.source,
                ModelFactSource::ExactModel | ModelFactSource::CallerOverride
            ) && resolution.applied_facts.contains(&facts.metadata)
                && (facts.capabilities.structured_output == Some(true)
                    || facts.profile.supports_structured_output == Some(true))
        });
        if resolution.profile.capabilities.structured_output
            && (self.has_official_ga_schema_model() || exact)
        {
            return Ok(());
        }
        Err(LlmError::InvalidResponse(format!(
            "Anthropic {feature} unsupported/unverified for exact provider/model '{}'/ '{}'; configure a sourced exact model fact", self.provider_name, self.model
        )).into())
    }

    fn is_deepseek_anthropic(&self) -> bool {
        reqwest::Url::parse(&self.base_url).ok().is_some_and(|url| {
            url.host_str() == Some("api.deepseek.com")
                && (url.path() == "/anthropic" || url.path().starts_with("/anthropic/"))
        })
    }

    fn has_official_ga_schema_model(&self) -> bool {
        // 官方 GA 正文只证明这三个具名 4.5 家族；不推断其它 Claude/兼容服务。
        // https://claude.com/resources/articles/structured-outputs-on-the-claude-developer-platform
        self.provider_name == "anthropic"
            && reqwest::Url::parse(&self.base_url)
                .ok()
                .is_some_and(|url| url.host_str() == Some("api.anthropic.com"))
            && matches!(
                self.model.as_str(),
                "claude-sonnet-4-5"
                    | "claude-sonnet-4-5-20250929"
                    | "claude-opus-4-5"
                    | "claude-opus-4-5-20251101"
                    | "claude-haiku-4-5"
                    | "claude-haiku-4-5-20251001"
            )
    }

    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        let model = model.into();
        let base_url = "https://api.anthropic.com/v1/messages".to_string();
        let provider_name = "anthropic".to_string();
        let model_profile_resolver = ModelProfileResolver::new().register_protocol_facts(
            &provider_name,
            protocol_fact_set(LlmApiProtocol::Anthropic, std::time::SystemTime::now()),
        );
        Self {
            client: Arc::new(Self::build_http_client()),
            api_key: api_key.into(),
            model,
            base_url,
            provider_name,
            model_profile_resolver,
            input_modalities: ModelInputModality::all_supported(),
            timeouts: LlmTimeouts::default(),
        }
    }

    pub fn with_base_url(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        let base_url = base_url.into();
        let model = model.into();
        let provider_name = "anthropic".to_string();
        let model_profile_resolver = ModelProfileResolver::new().register_protocol_facts(
            &provider_name,
            protocol_fact_set(LlmApiProtocol::Anthropic, std::time::SystemTime::now()),
        );
        Self {
            client: Arc::new(Self::build_http_client()),
            api_key: api_key.into(),
            model,
            base_url,
            provider_name,
            model_profile_resolver,
            input_modalities: ModelInputModality::all_supported(),
            timeouts: LlmTimeouts::default(),
        }
    }

    pub(crate) fn with_http_client(mut self, client: Arc<Client>) -> Self {
        self.client = client;
        self
    }

    pub(crate) fn with_model_profile_resolver(
        mut self,
        provider_name: impl Into<String>,
        resolver: ModelProfileResolver,
    ) -> Self {
        self.provider_name = provider_name.into();
        self.model_profile_resolver = resolver;
        self
    }

    pub fn with_input_modalities(mut self, input_modalities: Vec<ModelInputModality>) -> Self {
        self.input_modalities = if input_modalities.is_empty() {
            ModelInputModality::text_only()
        } else {
            input_modalities
        };
        self
    }

    /// Set the request and streaming timeout policy.
    pub fn with_timeouts(mut self, timeouts: LlmTimeouts) -> Self {
        self.timeouts = timeouts;
        self
    }

    fn build_http_client() -> Client {
        Client::new()
    }

    fn convert_request(&self, request: &ChatRequest) -> Result<AnthropicRequest> {
        let controls = self.validate_request_features(request)?;
        let mut system_parts: Vec<String> = Vec::new();
        let mut messages: Vec<AnthropicMessage> = Vec::new();
        let mut pending_tools = std::collections::HashSet::new();

        for msg in &request.messages {
            if msg.is_provider_response_observation_only() {
                continue;
            }
            if msg.role == Role::System {
                if let MessageContent::Parts(parts) = &msg.content
                    && parts.iter().any(|part| {
                        !matches!(
                            part,
                            ContentPart::Text { .. } | ContentPart::ResourceLink { .. }
                        )
                    })
                {
                    return Err(LlmError::InvalidResponse(
                        "Anthropic system content supports text only".into(),
                    )
                    .into());
                }
                if let Some(text) = msg.content.as_text() {
                    system_parts.push(text);
                }
                continue;
            }

            if msg.role == Role::Tool {
                let tool_use_id = msg
                    .tool_call_id
                    .clone()
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| {
                        LlmError::InvalidResponse(
                            "Anthropic tool_result omitted tool_call_id".into(),
                        )
                    })?;
                if !pending_tools.remove(&tool_use_id) {
                    return Err(LlmError::InvalidResponse(
                        "Anthropic tool_result is duplicate or has no preceding tool_use".into(),
                    )
                    .into());
                }
                let result = ContentBlock::ToolResult {
                    tool_use_id,
                    content: tool_result_content(&msg.content)?,
                    // DeepSeek忽略is_error；Some(false)等价其普通非错误tool_result默认语义。
                    // 仅wire省略不支持的false标记，canonical Message/State仍保Some(false)。
                    // Some(true)无法兑现原生错误语义，交方言检查明确拒绝；None不推断成功。
                    is_error: if self.is_deepseek_anthropic()
                        && msg.tool_result_is_error == Some(false)
                    {
                        None
                    } else {
                        msg.tool_result_is_error
                    },
                    cache_control: None,
                };
                // 并行工具回填必须处于同一个紧随 assistant 的 user 消息，顺序沿输入保留。
                if let Some(last) = messages.last_mut()
                    && last.role == "user"
                    && let AnthropicContent::Blocks(blocks) = &mut last.content
                    && blocks
                        .iter()
                        .all(|block| matches!(block, ContentBlock::ToolResult { .. }))
                {
                    blocks.push(result);
                } else {
                    messages.push(AnthropicMessage {
                        role: "user".into(),
                        content: AnthropicContent::Blocks(vec![result]),
                    });
                }
                continue;
            }
            if !pending_tools.is_empty() {
                return Err(LlmError::InvalidResponse(
                    "Anthropic tool results must precede the next conversation message".into(),
                )
                .into());
            }
            if !matches!(msg.role, Role::User | Role::Assistant) {
                return Err(LlmError::InvalidResponse(
                    "Anthropic messages support user/assistant roles only".into(),
                )
                .into());
            }
            if msg.role == Role::Assistant {
                if let MessageContent::Parts(parts) = &msg.content
                    && parts.iter().any(|part| {
                        !matches!(
                            part,
                            ContentPart::Text { .. } | ContentPart::ResourceLink { .. }
                        )
                    })
                {
                    return Err(LlmError::InvalidResponse(
                        "Anthropic assistant input cannot represent Image/File parts".into(),
                    )
                    .into());
                }
                for call in msg.tool_calls.as_deref().unwrap_or_default() {
                    if call.id.is_empty()
                        || call.function.name.is_empty()
                        || !pending_tools.insert(call.id.clone())
                    {
                        return Err(LlmError::InvalidResponse(
                            "Anthropic tool_use requires unique nonempty id/name".into(),
                        )
                        .into());
                    }
                }
            }

            if msg.role == Role::Assistant
                && let Some(blocks) = native_anthropic_history(
                    msg,
                    self.is_deepseek_anthropic(),
                    &self.provider_name,
                )?
            {
                messages.push(AnthropicMessage {
                    role: "assistant".into(),
                    content: AnthropicContent::Blocks(blocks),
                });
                continue;
            }

            if msg.role == Role::Assistant
                && let Some(ref tool_calls) = msg.tool_calls
            {
                let mut blocks: Vec<ContentBlock> = Vec::new();
                append_reasoning_blocks(
                    &mut blocks,
                    msg.reasoning_blocks.as_deref(),
                    self.is_deepseek_anthropic(),
                    &self.provider_name,
                )?;
                if let Some(ref text) = msg.content.as_text()
                    && !text.is_empty()
                {
                    blocks.push(ContentBlock::Text {
                        text: text.clone(),
                        cache_control: None,
                    });
                }
                for tc in tool_calls {
                    let input = parse_tool_input(&tc.function.arguments)?;
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
                    let mut blocks: Vec<ContentBlock> = Vec::new();
                    if msg.role == Role::Assistant {
                        append_reasoning_blocks(
                            &mut blocks,
                            msg.reasoning_blocks.as_deref(),
                            self.is_deepseek_anthropic(),
                            &self.provider_name,
                        )?;
                    }
                    blocks.extend(
                        parts
                            .iter()
                            .map(part_to_content_block)
                            .collect::<Result<Vec<_>>>()?,
                    );
                    if blocks.is_empty() {
                        AnthropicContent::Text(String::new())
                    } else {
                        AnthropicContent::Blocks(blocks)
                    }
                }
                _ if msg.role == Role::Assistant && msg.reasoning_blocks.is_some() => {
                    let mut blocks = Vec::new();
                    append_reasoning_blocks(
                        &mut blocks,
                        msg.reasoning_blocks.as_deref(),
                        self.is_deepseek_anthropic(),
                        &self.provider_name,
                    )?;
                    if let Some(text) = msg.content.as_text()
                        && !text.is_empty()
                    {
                        blocks.push(ContentBlock::Text {
                            text,
                            cache_control: None,
                        });
                    }
                    AnthropicContent::Blocks(blocks)
                }
                _ => AnthropicContent::Text(msg.content.as_text().unwrap_or_default()),
            };

            messages.push(AnthropicMessage {
                role: msg.role.as_str().to_string(),
                content,
            });
        }

        // ── Cache breakpoint strategy (delegated to AnthropicCachePlan) ──
        //
        // Build a read-only layout view from the request messages + tools, then
        // let `AnthropicCachePlan` decide which breakpoints to place. This
        // consolidates the strategy in one place and makes it unit-testable.
        //
        // When `cache_hints` is present on the request, the agent layer has
        // pre-computed the layout; we build a plan from those hints.
        // Otherwise we build the layout here and apply the default plan.

        let tools_ref: &[echo_core::llm::types::ToolDefinition] =
            request.tools.as_deref().unwrap_or(&[]);
        let cache_plan = if let Some(ref hints) = request.cache_hints
            && !hints.breakpoints.is_empty()
        {
            // Agent layer provided explicit breakpoints via CacheHints.
            use echo_core::llm::cache::BreakpointTarget as BT;
            AnthropicCachePlan {
                breakpoints: hints.breakpoints.clone(),
                has_system_breakpoint: hints
                    .breakpoints
                    .iter()
                    .any(|b| matches!(b, BT::SystemLastBlock)),
                has_tool_breakpoint: hints
                    .breakpoints
                    .iter()
                    .any(|b| matches!(b, BT::ToolsLastTool)),
            }
        } else {
            // No explicit breakpoints: either no `cache_hints` at all, or the
            // main think path which sends `Some(CacheHints { breakpoints: vec![], .. })`
            // — the agent layer computes the layout for the stable-prefix hash
            // but leaves breakpoint derivation to the provider. Deriving here
            // (rather than at the agent layer) keeps the layering clean: echo-agent
            // must not depend on the Anthropic-specific `AnthropicCachePlan`.
            // WITHOUT this fallback the main path would place ZERO cache_control
            // (has_system/tool=false, history_breakpoint_count=0) and Anthropic
            // would never cache the stable prefix on the highest-volume path.
            let layout = echo_core::llm::cache::PromptCacheLayout::from_messages(
                &request.messages,
                tools_ref,
            );
            AnthropicCachePlan::from_layout(&layout)
        };

        // Build tools with cache_control on the last tool.
        let tools: Option<Vec<AnthropicToolDef>> = request.tools.as_ref().map(|tools| {
            let count = tools.len();
            tools
                .iter()
                .enumerate()
                .map(|(i, t)| AnthropicToolDef {
                    name: t.function.name.clone(),
                    description: Some(t.function.description.clone()),
                    input_schema: t.function.parameters.clone(),
                    strict: t.function.strict,
                    cache_control: if cache_plan.has_tool_breakpoint && i == count - 1 {
                        Some(CacheControl::ephemeral())
                    } else {
                        None
                    },
                })
                .collect()
        });

        // Convert system prompt to blocks format with cache_control.
        let system = (!system_parts.is_empty()).then(|| {
            let text = system_parts.join("\n\n");
            vec![SystemBlock {
                block_type: "text".to_string(),
                text,
                cache_control: if cache_plan.has_system_breakpoint {
                    Some(CacheControl::ephemeral())
                } else {
                    None
                },
            }]
        });

        // Place cache breakpoints on conversation messages.
        if cache_plan.history_breakpoint_count() > 0 {
            let used_breakpoints = usize::from(cache_plan.has_system_breakpoint)
                + usize::from(cache_plan.has_tool_breakpoint && tools.is_some());
            let remaining_breakpoints = 4usize.saturating_sub(used_breakpoints);

            if remaining_breakpoints > 0 {
                if cache_plan.breakpoints.is_empty() {
                    // Default fallback (no explicit history indices).
                    apply_conversation_cache_breakpoints(&mut messages, remaining_breakpoints);
                } else {
                    // Map BreakpointTarget to concrete message indices and apply.
                    let mut msg_indices: Vec<usize> = cache_plan
                        .breakpoints
                        .iter()
                        .filter_map(|bp| match bp {
                            echo_core::llm::cache::BreakpointTarget::HistoryIndex(i) => Some(*i),
                            echo_core::llm::cache::BreakpointTarget::HistoryLastStable => {
                                messages.iter().rposition(|msg| !msg.is_runtime_context())
                            }
                            _ => None,
                        })
                        .collect();
                    msg_indices.sort_unstable();
                    msg_indices.dedup();
                    for &idx in msg_indices.iter().take(remaining_breakpoints) {
                        if let Some(msg) = messages.get_mut(idx) {
                            msg.add_cache_control_ephemeral();
                        }
                    }
                }
            }
        }

        // thinking、format、effort 一次校验，同源用于流式和非流式请求。
        let max_tokens = request.max_tokens.unwrap_or(4096);
        if self.is_deepseek_anthropic() {
            for message in &messages {
                if let AnthropicContent::Blocks(blocks) = &message.content {
                    validate_deepseek_blocks(blocks)?;
                }
            }
        }
        Ok(AnthropicRequest {
            model: self.model.clone(),
            max_tokens,
            system,
            messages,
            temperature: request.temperature,
            tools,
            stream: None,
            thinking: controls.thinking,
            output_config: controls.output_config,
            tool_choice: controls.tool_choice,
            metadata: request.user_id.as_ref().map(|uid| AnthropicMetadata {
                user_id: uid.clone(),
            }),
        })
    }

    fn convert_response(&self, resp: AnthropicResponse) -> Result<ChatResponse> {
        let mut raw = resp.0;
        strip_anthropic_framework_extensions(&mut raw);
        if !raw.is_object() {
            return Err(
                LlmError::InvalidResponse("Anthropic response must be an object".into()).into(),
            );
        }
        let provider_response = anthropic_response_metadata(&raw, &self.provider_name)?;
        let finish = anthropic_finish_reason(
            provider_response
                .stop_reason
                .as_ref()
                .and_then(serde_json::Value::as_str),
        );
        let content: Vec<ContentBlock> =
            serde_json::from_value(raw.get("content").cloned().ok_or_else(|| {
                LlmError::InvalidResponse("Anthropic response omitted content".into())
            })?)
            .map_err(|error| LlmError::InvalidResponse(error.to_string()))?;
        let native_request_id = provider_response
            .request_id
            .as_deref()
            .filter(|id| !id.is_empty());
        // 本地fallback只拥有规范块身份，绝不写成服务原生Source坐标。
        let request_id = native_request_id
            .map(str::to_string)
            .unwrap_or_else(|| super::native_message::fallback_request_id("anthropic"));
        let native_usage = raw
            .get("usage")
            .filter(|value| !value.is_null())
            .cloned()
            .map(serde_json::from_value::<AnthropicUsage>)
            .transpose()
            .map_err(|error| LlmError::InvalidResponse(error.to_string()))?;
        let mut assistant_blocks = Vec::new();
        let mut content_parts: Vec<String> = Vec::new();
        let mut reasoning_parts: Vec<String> = Vec::new();
        let mut reasoning_blocks: Vec<ReasoningBlock> = Vec::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut tool_ids = std::collections::HashSet::new();

        for (index, block) in content.iter().enumerate() {
            let sequence = u64::try_from(index).map_err(|_| {
                LlmError::InvalidResponse("Anthropic content index exceeds u64".into())
            })?;
            let (phase, delta, reasoning) = match block {
                ContentBlock::Text { text, .. } => (AssistantPhase::Unknown, text.clone(), None),
                ContentBlock::Thinking {
                    thinking,
                    signature,
                } => (
                    AssistantPhase::Reasoning,
                    thinking.clone(),
                    Some(thinking_reasoning(
                        thinking,
                        signature.as_deref(),
                        &self.provider_name,
                        &format!("{request_id}:content:{index}"),
                        self.is_deepseek_anthropic(),
                    )?),
                ),
                ContentBlock::RedactedThinking { data } => {
                    if self.is_deepseek_anthropic() {
                        return Err(LlmError::InvalidResponse(
                            "DeepSeek Anthropic redacted_thinking is unsupported".into(),
                        )
                        .into());
                    }
                    if data.is_empty() {
                        return Err(LlmError::InvalidResponse(
                            "Anthropic redacted_thinking omitted data".into(),
                        )
                        .into());
                    }
                    (
                        AssistantPhase::Reasoning,
                        String::new(),
                        Some(ReasoningBlock::Redacted { data: data.clone() }),
                    )
                }
                _ => (AssistantPhase::Unknown, String::new(), None),
            };
            let output_index = u32::try_from(index).map_err(|_| {
                LlmError::InvalidResponse("Anthropic content index exceeds u32".into())
            })?;
            let content_kind = match block {
                ContentBlock::Text { .. } => "text",
                ContentBlock::Thinking { .. } => "thinking",
                ContentBlock::RedactedThinking { .. } => "redacted_thinking",
                ContentBlock::ToolUse { .. } => "tool_use",
                _ => {
                    return Err(LlmError::InvalidResponse(
                        "unsupported Anthropic output block".into(),
                    )
                    .into());
                }
            };
            if !matches!(block, ContentBlock::ToolUse { .. }) {
                assistant_blocks.push(AssistantMessageBlock {
                    provider_source: native_request_id.map(|native_id| {
                        output_source(
                            LlmApiProtocol::Anthropic,
                            native_id,
                            &format!("{request_id}:content:{index}"),
                            output_index,
                            None,
                            content_kind,
                            None,
                        )
                    }),
                    block_id: format!("{request_id}:content:{index}"),
                    request_id: request_id.clone(),
                    sequence,
                    phase,
                    delta,
                    complete: finish.is_ok(),
                    provider: Some(self.provider_name.clone()),
                    reasoning,
                });
            }
            match block {
                ContentBlock::Text { text, .. } => content_parts.push(text.clone()),
                ContentBlock::ToolUse { id, name, input } => {
                    if id.is_empty()
                        || name.is_empty()
                        || !input.is_object()
                        || !tool_ids.insert(id.clone())
                    {
                        return Err(LlmError::InvalidResponse(
                            "Anthropic tool_use has malformed identity or input".into(),
                        )
                        .into());
                    }
                    tool_calls.push(ToolCall {
                        provider_source: Some(output_source(
                            LlmApiProtocol::Anthropic,
                            native_request_id.unwrap_or_default(),
                            id,
                            output_index,
                            None,
                            "tool_use",
                            None,
                        )),
                        id: id.clone(),
                        call_type: "function".to_string(),
                        function: FunctionCall {
                            name: name.clone(),
                            arguments: serde_json::to_string(input)
                                .map_err(|error| LlmError::SerializationError(error.to_string()))?,
                        },
                    });
                }
                ContentBlock::Thinking {
                    thinking,
                    signature,
                } => {
                    reasoning_parts.push(thinking.clone());
                    reasoning_blocks.push(thinking_reasoning(
                        thinking,
                        signature.as_deref(),
                        &self.provider_name,
                        &format!("{request_id}:content:{index}"),
                        self.is_deepseek_anthropic(),
                    )?);
                }
                ContentBlock::RedactedThinking { data } => {
                    reasoning_blocks.push(ReasoningBlock::Redacted { data: data.clone() });
                }
                _ => {}
            }
        }

        let message = Message {
            role: Role::Assistant,
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
            reasoning_content: (!reasoning_parts.is_empty()).then(|| reasoning_parts.join("")),
            reasoning_blocks: (!reasoning_blocks.is_empty()).then_some(reasoning_blocks),
            assistant_blocks: (!assistant_blocks.is_empty()).then_some(assistant_blocks),
            execution_status: None,
            approved_assistant_blocks: None,
            tool_result_is_error: None,
            provider_response: Some(provider_response),
        };
        let finish_reason = match finish {
            Ok(reason) if !content.is_empty() => reason,
            Ok(_) => return Err(LlmError::EmptyResponse.into()),
            Err(_) => {
                return Err(LlmError::IncompleteResponse {
                    partial: Box::new(message),
                }
                .into());
            }
        };

        let validation = if finish_reason.as_deref() != Some("continue") {
            super::native_message::validate_completion(&message, finish_reason.as_deref())
        } else if message
            .tool_calls
            .as_ref()
            .is_some_and(|tools| !tools.is_empty())
            || message
                .content
                .as_text()
                .is_some_and(|text| !text.trim().is_empty())
        {
            super::native_message::validate_completion(&message, Some("stop"))
        } else {
            Ok(())
        };
        if let Err(error) = validation {
            if matches!(&error, echo_core::error::ReactError::Llm(inner) if matches!(inner.as_ref(), LlmError::EmptyResponse))
            {
                return Err(LlmError::IncompleteResponse {
                    partial: Box::new(message),
                }
                .into());
            }
            return Err(error);
        }

        if message
            .provider_response
            .as_ref()
            .and_then(|metadata| metadata.request_id.as_deref())
            .filter(|id| !id.is_empty())
            .is_none()
        {
            return Err(LlmError::InvalidResponse(
                "Anthropic completed response omitted native id".into(),
            )
            .into());
        }

        // Extract token usage from Anthropic response
        let usage = native_usage.as_ref().map(|u| {
            let prompt = u.input_tokens;
            let completion = u.output_tokens;
            Usage {
                prompt_tokens: Some(prompt),
                completion_tokens: Some(completion),
                total_tokens: Some(prompt.saturating_add(completion)),
                cache_creation_input_tokens: u.cache_creation_input_tokens,
                cache_read_input_tokens: u.cache_read_input_tokens,
                ..Default::default()
            }
        });

        Ok(ChatResponse {
            message,
            finish_reason,
            usage: usage.clone(),
            raw: ChatCompletionResponse {
                id: request_id.clone(),
                choices: Vec::new(),
                created: None,
                model: raw
                    .get("model")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                usage,
                extra: Some(raw),
            },
        })
    }
}

impl LlmClient for AnthropicClient {
    fn chat(&self, request: ChatRequest) -> BoxFuture<'_, Result<ChatResponse>> {
        let model = self.model.clone();
        Box::pin(
            async move {
                let timeouts = request.timeouts.unwrap_or(self.timeouts);
                let body = self.convert_request(&request)?;
                let request_builder = self
                    .client
                    .post(&self.base_url)
                    .header("x-api-key", &self.api_key)
                    .header("anthropic-version", "2023-06-01")
                    .header("anthropic-beta", "prompt-caching-2024-07-31")
                    .header("content-type", "application/json")
                    .json(&body);
                let request_builder = match timeouts.request_timeout() {
                    Some(timeout) => request_builder.timeout(timeout),
                    None => request_builder,
                };
                let mut raw: serde_json::Value =
                    post_json_request(request_builder, request.cancel_token.clone()).await?;
                strip_anthropic_framework_extensions(&mut raw);
                let anthropic_resp: AnthropicResponse = serde_json::from_value(raw.clone())
                    .map_err(|error| LlmError::InvalidResponse(error.to_string()))?;
                let mut response = self.convert_response(anthropic_resp)?;
                // 原响应的缺省/null与供应商扩展只在现有 raw 通道保留；不能经 typed 重序列化补造字段。
                response.raw.extra = Some(raw);
                ensure_request_not_cancelled(request.cancel_token.as_ref())?;
                Ok(response)
            }
            .instrument(info_span!("anthropic_chat", model = %model)),
        )
    }

    fn chat_stream(
        &self,
        request: ChatRequest,
    ) -> BoxFuture<'_, Result<BoxStream<'static, Result<ChatChunk>>>> {
        let model = self.model.clone();
        Box::pin(
            async move {
                let timeouts = request.timeouts.unwrap_or(self.timeouts);
                let mut body = self.convert_request(&request)?;
                body.stream = Some(true);

                let cancel_token = request.cancel_token.clone();
                let stream_request = self
                    .client
                    .post(&self.base_url)
                    .header("x-api-key", &self.api_key)
                    .header("anthropic-version", "2023-06-01")
                    .header("anthropic-beta", "prompt-caching-2024-07-31")
                    .header("content-type", "application/json")
                    .json(&body);
                let raw_stream =
                    stream_json_sse(stream_request, self.model.clone(), timeouts, cancel_token)
                        .await?;
                Ok(adapt_anthropic_stream(
                    raw_stream,
                    self.provider_name.clone(),
                    self.is_deepseek_anthropic(),
                ))
            }
            .instrument(info_span!("anthropic_chat_stream", model = %model)),
        )
    }

    fn model_name(&self) -> &str {
        &self.model
    }

    fn provider_name(&self) -> Option<&str> {
        Some(&self.provider_name)
    }

    fn protocol_capabilities(&self) -> ProviderCapabilityOverride {
        let mut capabilities = ProviderCapabilityOverride::anthropic_protocol();
        capabilities.supports_tool_choice_none = Some(true);
        // 表示适配器可编码；实际模型资格由 sourced resolution 决定。
        capabilities.structured_output = Some(true);
        capabilities
    }

    fn model_profile_resolution(&self) -> ModelProfileResolution {
        let mut facts = protocol_fact_set(LlmApiProtocol::Anthropic, std::time::SystemTime::now());
        facts.capabilities.supports_tool_choice_none =
            Some(self.has_official_ga_schema_model() || self.is_deepseek_anthropic());
        // schema 编码已实现，但未知模型不可从 protocol 直接继承支持。
        facts.capabilities.structured_output = Some(self.has_official_ga_schema_model());
        if self.has_official_ga_schema_model() {
            facts.metadata.provenance = "Anthropic structured outputs GA 2026-02-04; https://claude.com/resources/articles/structured-outputs-on-the-claude-developer-platform".into();
        }
        self.model_profile_resolver
            .clone()
            .register_protocol_facts(&self.provider_name, facts)
            .resolve_for_protocol_at(
                &self.provider_name,
                &self.model,
                LlmApiProtocol::Anthropic,
                Some(&self.base_url),
                std::time::SystemTime::now(),
            )
    }

    fn capabilities(&self) -> echo_core::llm::capabilities::ProviderCapabilities {
        self.model_profile_resolution().profile.capabilities
    }
}

/// 只清协议wrapper/内容块的框架保留扩展；不递归改写工具input或原生usage中的用户数据。
fn strip_anthropic_framework_extensions(raw: &mut serde_json::Value) {
    let Some(fields) = raw.as_object_mut() else {
        return;
    };
    for name in [
        "provider_response",
        "execution_status",
        "approved_assistant_blocks",
        "assistant_blocks",
        "provider_source",
    ] {
        fields.remove(name);
    }
    for wrapper in ["message", "content_block", "extra", "extensions"] {
        if let Some(value) = fields.get_mut(wrapper) {
            strip_anthropic_framework_extensions(value);
        }
    }
    if let Some(content) = fields
        .get_mut("content")
        .and_then(serde_json::Value::as_array_mut)
    {
        for block in content {
            strip_anthropic_framework_extensions(block);
        }
    }
}

/// 只从本协议外层原件提取已批准字段；通过Core公开serde校验固定记录，不依赖私有helper。
fn anthropic_response_metadata(
    raw: &serde_json::Value,
    provider: &str,
) -> Result<ProviderResponseMetadata> {
    if !raw.is_object() {
        return Err(LlmError::InvalidResponse(
            "Anthropic native response wrapper must be an object".into(),
        )
        .into());
    }
    let request_id = match raw.get("id") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(id)) => Some(id.clone()),
        Some(_) => {
            return Err(LlmError::InvalidResponse(
                "Anthropic response id must be string or null".into(),
            )
            .into());
        }
    };
    let mut fields = serde_json::Map::new();
    fields.insert(
        "protocol".into(),
        serde_json::to_value(LlmApiProtocol::Anthropic)
            .map_err(|error| LlmError::SerializationError(error.to_string()))?,
    );
    fields.insert(
        "provider".into(),
        serde_json::Value::String(provider.into()),
    );
    if let Some(id) = request_id {
        fields.insert("request_id".into(), serde_json::Value::String(id));
    }
    for name in ["model", "stop_reason", "stop_sequence", "usage"] {
        if let Some(value) = raw.get(name) {
            fields.insert(name.into(), value.clone());
        }
    }
    // 原生Anthropic没有Responses status/incomplete_details；任意上游同名扩展不进入公共观察。
    serde_json::from_value(serde_json::Value::Object(fields)).map_err(|error| {
        LlmError::InvalidResponse(format!("Anthropic response observation invalid: {error}")).into()
    })
}

/// 同一响应的原生usage更新是累计计数/键值更新；绝不把output_tokens相加。
fn merge_anthropic_usage_snapshot(saved: &mut serde_json::Value, update: &serde_json::Value) {
    match (saved.as_object_mut(), update.as_object()) {
        (Some(saved), Some(update)) => {
            for (key, value) in update {
                if let Some(previous) = saved.get_mut(key) {
                    merge_anthropic_usage_snapshot(previous, value);
                } else {
                    saved.insert(key.clone(), value.clone());
                }
            }
        }
        _ => *saved = update.clone(),
    }
}

/// Streaming and non-streaming responses use the same stop classification.
/// pause_turn is a normal continuation boundary, never a final answer.
fn anthropic_finish_reason(reason: Option<&str>) -> Result<Option<String>> {
    match reason {
        Some("end_turn" | "stop_sequence" | "refusal") => Ok(Some("stop".to_string())),
        Some("tool_use") => Ok(Some("tool_calls".to_string())),
        Some("pause_turn") => Ok(Some("continue".to_string())),
        other => Err(LlmError::InvalidResponse(format!(
            "Anthropic response has incomplete or unknown stop reason: {other:?}"
        ))
        .into()),
    }
}

// ── Anthropic API types ──────────────────────────────────────────────────────

/// Cache control marker for Anthropic prompt caching
#[derive(Serialize, Deserialize, Debug, Clone)]
struct CacheControl {
    #[serde(rename = "type")]
    cache_type: String,
}

impl CacheControl {
    fn ephemeral() -> Self {
        Self {
            cache_type: "ephemeral".to_string(),
        }
    }
}

/// System prompt block that can carry cache_control
#[derive(Serialize)]
struct SystemBlock {
    #[serde(rename = "type")]
    block_type: String,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<CacheControl>,
}

#[derive(Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<Vec<SystemBlock>>,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<AnthropicToolDef>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
    /// 原生 thinking 控制；不可表达的显式配置在编码前报错。
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<AnthropicThinking>,
    /// 官方 schema：format 与 effort 均位于 output_config，禁止 top-level effort。
    #[serde(skip_serializing_if = "Option::is_none")]
    output_config: Option<AnthropicOutputConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<AnthropicToolChoice>,
    /// `metadata:{user_id}` for prompt-cache (KVCache) isolation on the
    /// DeepSeek/Anthropic endpoint. Filled from `ChatRequest.user_id` when set;
    /// omitted entirely when `None` (stage4 P4.1: cache_user_id single-source).
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<AnthropicMetadata>,
}

/// Anthropic endpoint metadata carrying `user_id` for prompt-cache isolation
/// (DeepSeek requires this on its Anthropic-compatible endpoint).
#[derive(Serialize)]
struct AnthropicMetadata {
    user_id: String,
}

/// Anthropic `thinking` block. For 3.7–4.5 this is
/// `{type:"enabled", budget_tokens}`; for 4.6 it's `{type:"adaptive"}`.
#[derive(Serialize)]
struct AnthropicThinking {
    #[serde(rename = "type")]
    block_type: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    budget_tokens: Option<u32>,
}

/// 显式请求不可被 clamp/drop 成另一个承诺；native budget 至少 1024 且小于输出上限。
fn build_anthropic_thinking(
    model: &str,
    thinking_protocol: ThinkingProtocol,
    thinking: &Option<echo_core::llm::ThinkingConfig>,
    max_tokens: u32,
) -> Result<(Option<AnthropicThinking>, Option<String>)> {
    use echo_core::llm::{ThinkingConfig as C, ThinkingLevel as L, ThinkingProtocol as T};
    let Some(cfg) = thinking.as_ref() else {
        return Ok((None, None));
    };
    let invalid = |reason: &str| {
        LlmError::InvalidResponse(format!(
            "Anthropic thinking unsupported for model '{model}': {reason}"
        ))
    };
    let disabled = matches!(cfg, C::Disabled | C::Level(L::None | L::Minimal));
    match thinking_protocol {
        T::AnthropicThinkingBudget => {
            if disabled {
                return Ok((Some(AnthropicThinking { block_type: "disabled", budget_tokens: None }), None));
            }
            let budget = match cfg {
                C::BudgetTokens(tokens) => *tokens,
                _ => cfg.to_anthropic_budget(max_tokens).ok_or_else(|| invalid("cannot resolve budget"))?,
            };
            if budget < 1024 || budget >= max_tokens {
                return Err(invalid("budget_tokens must be >=1024 and strictly less than max_tokens").into());
            }
            Ok((Some(AnthropicThinking { block_type: "enabled", budget_tokens: Some(budget) }), None))
        }
        T::AnthropicEffort => {
            if matches!(cfg, C::BudgetTokens(_)) {
                return Err(invalid("an exact token budget is not representable by adaptive effort").into());
            }
            if disabled {
                return Ok((Some(AnthropicThinking { block_type: "disabled", budget_tokens: None }), None));
            }
            Ok((Some(AnthropicThinking { block_type: "adaptive", budget_tokens: None }), cfg.to_anthropic_effort().map(str::to_string)))
        }
        T::AnthropicAdaptive => Err(invalid("explicit controls on adaptive-only models need exact primary evidence; no setting was silently dropped").into()),
        _ => Err(invalid("resolved model has no Anthropic thinking dialect").into()),
    }
}

/// DeepSeek 官方 Anthropic 兼容合同：budget 不生效，effort 与温度走该服务方言。
fn build_deepseek_anthropic_thinking(
    thinking: &Option<echo_core::llm::ThinkingConfig>,
) -> Result<(Option<AnthropicThinking>, Option<String>)> {
    use echo_core::llm::{ThinkingConfig as C, ThinkingLevel as L};
    let Some(cfg) = thinking else {
        return Ok((None, None));
    };
    match cfg {
        C::BudgetTokens(_) => Err(LlmError::InvalidResponse(
            "DeepSeek Anthropic ignores budget_tokens; an exact budget cannot be honored".into(),
        )
        .into()),
        C::Disabled | C::Level(L::None | L::Minimal) => Ok((
            Some(AnthropicThinking {
                block_type: "disabled",
                budget_tokens: None,
            }),
            None,
        )),
        _ => Ok((
            Some(AnthropicThinking {
                block_type: "enabled",
                budget_tokens: None,
            }),
            cfg.to_anthropic_effort().map(str::to_string),
        )),
    }
}

fn thinking_reasoning(
    thinking: &str,
    signature: Option<&str>,
    provider: &str,
    id: &str,
    allow_unsigned: bool,
) -> Result<ReasoningBlock> {
    if let Some(signature) = signature.filter(|signature| !signature.is_empty()) {
        return Ok(ReasoningBlock::Signed {
            thinking: thinking.into(),
            signature: signature.into(),
        });
    }
    if !allow_unsigned {
        return Err(LlmError::InvalidResponse(
            "Anthropic native thinking requires its signature".into(),
        )
        .into());
    }
    let mut raw = serde_json::json!({"type":"thinking","thinking":thinking});
    if let Some(signature) = signature
        && let Some(fields) = raw.as_object_mut()
    {
        fields.insert(
            "signature".into(),
            serde_json::Value::String(signature.into()),
        );
    }
    Ok(ReasoningBlock::Opaque {
        provider: provider.into(),
        id: id.into(),
        data: raw.to_string(),
        summary: Vec::new(),
    })
}

fn unsigned_thinking_replay(raw: &str) -> Result<ContentBlock> {
    let value: ContentBlock = serde_json::from_str(raw).map_err(|_| {
        LlmError::InvalidResponse("DeepSeek opaque thinking is not a native block".into())
    })?;
    if !matches!(value, ContentBlock::Thinking { .. }) {
        return Err(
            LlmError::InvalidResponse("DeepSeek opaque state is not thinking".into()).into(),
        );
    }
    Ok(value)
}

fn validate_deepseek_blocks(blocks: &[ContentBlock]) -> Result<()> {
    for block in blocks {
        match block {
            ContentBlock::Document { .. } | ContentBlock::RedactedThinking { .. } => {
                return Err(LlmError::InvalidResponse(
                    "DeepSeek Anthropic does not support document/redacted_thinking".into(),
                )
                .into());
            }
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                if *is_error == Some(true) {
                    return Err(LlmError::InvalidResponse("DeepSeek Anthropic ignores tool_result.is_error=true; native error observation cannot be represented".into()).into());
                }
                if let AnthropicToolResultContent::Blocks(parts) = content {
                    validate_deepseek_blocks(parts)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

struct AnthropicControls {
    tool_choice: Option<AnthropicToolChoice>,
    thinking: Option<AnthropicThinking>,
    output_config: Option<AnthropicOutputConfig>,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicToolChoice {
    Auto,
    None,
    Any,
    Tool { name: String },
}

#[derive(Serialize)]
struct AnthropicOutputConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<AnthropicJsonFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    effort: Option<String>,
}

#[derive(Serialize)]
struct AnthropicJsonFormat {
    #[serde(rename = "type")]
    format_type: &'static str,
    schema: serde_json::Value,
}

#[derive(Serialize)]
struct AnthropicMessage {
    role: String,
    content: AnthropicContent,
}

impl AnthropicMessage {
    fn add_cache_control_ephemeral(&mut self) {
        self.content.add_cache_control_ephemeral();
    }

    fn is_runtime_context(&self) -> bool {
        self.content.is_runtime_context()
    }
}

#[derive(Serialize)]
#[serde(untagged)]
enum AnthropicContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

impl AnthropicContent {
    fn add_cache_control_ephemeral(&mut self) {
        match self {
            AnthropicContent::Text(text) => {
                let text = std::mem::take(text);
                *self = AnthropicContent::Blocks(vec![ContentBlock::Text {
                    text,
                    cache_control: Some(CacheControl::ephemeral()),
                }]);
            }
            AnthropicContent::Blocks(blocks) => {
                if let Some(block) = blocks.iter_mut().rev().find(|block| block.is_cacheable()) {
                    block.set_cache_control(CacheControl::ephemeral());
                }
            }
        }
    }

    fn is_runtime_context(&self) -> bool {
        match self {
            AnthropicContent::Text(text) => is_runtime_context_text(text),
            AnthropicContent::Blocks(blocks) => blocks.iter().any(ContentBlock::is_runtime_context),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type")]
enum ContentBlock {
    #[serde(rename = "text")]
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    #[serde(rename = "image")]
    Image {
        source: ImageSource,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    /// PDF document content block.
    ///
    /// Anthropic only accepts `application/pdf` for base64 document sources
    /// (other mime types fail). Text-class files are inlined as Text blocks
    /// instead — see the `ContentPart::File` handling below.
    #[serde(rename = "document")]
    Document {
        source: ImageSource,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: AnthropicToolResultContent,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    #[serde(rename = "thinking")]
    Thinking {
        thinking: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    #[serde(rename = "redacted_thinking")]
    RedactedThinking { data: String },
    #[serde(other)]
    Other,
}

impl ContentBlock {
    fn is_cacheable(&self) -> bool {
        matches!(
            self,
            ContentBlock::Text { .. }
                | ContentBlock::Image { .. }
                | ContentBlock::ToolResult { .. }
        )
    }

    fn set_cache_control(&mut self, cache_control: CacheControl) {
        match self {
            ContentBlock::Text {
                cache_control: field,
                ..
            }
            | ContentBlock::Image {
                cache_control: field,
                ..
            }
            | ContentBlock::Document {
                cache_control: field,
                ..
            }
            | ContentBlock::ToolResult {
                cache_control: field,
                ..
            } => *field = Some(cache_control),
            ContentBlock::ToolUse { .. }
            | ContentBlock::Thinking { .. }
            | ContentBlock::RedactedThinking { .. }
            | ContentBlock::Other => {}
        }
    }

    fn is_runtime_context(&self) -> bool {
        match self {
            ContentBlock::Text { text, .. } => is_runtime_context_text(text),
            ContentBlock::ToolResult { content, .. } => content.is_runtime_context(),
            _ => false,
        }
    }
}

fn append_reasoning_blocks(
    target: &mut Vec<ContentBlock>,
    reasoning_blocks: Option<&[ReasoningBlock]>,
    allow_unsigned: bool,
    provider: &str,
) -> Result<()> {
    for block in reasoning_blocks.unwrap_or_default() {
        let content = match block {
            ReasoningBlock::Signed {
                thinking,
                signature,
            } => {
                if signature.is_empty() {
                    return Err(LlmError::InvalidResponse(
                        "Anthropic replay thinking signature is empty".into(),
                    )
                    .into());
                }
                ContentBlock::Thinking {
                    thinking: thinking.clone(),
                    signature: Some(signature.clone()),
                }
            }
            ReasoningBlock::Redacted { data } => {
                ContentBlock::RedactedThinking { data: data.clone() }
            }
            // Opaque state is owned by another provider and must not cross the
            // protocol boundary when a conversation switches models.
            ReasoningBlock::Opaque {
                provider: owner,
                data,
                ..
            } if allow_unsigned && owner == provider => unsigned_thinking_replay(data)?,
            ReasoningBlock::Opaque { .. } => continue,
        };
        target.push(content);
    }
    Ok(())
}

fn apply_conversation_cache_breakpoints(
    messages: &mut [AnthropicMessage],
    remaining_breakpoints: usize,
) {
    if remaining_breakpoints == 0 || messages.is_empty() {
        return;
    }

    let stable_end = messages
        .iter()
        .rposition(|message| !message.is_runtime_context())
        .map(|index| index + 1)
        .unwrap_or(0);
    if stable_end == 0 {
        return;
    }

    let mut indexes = Vec::new();
    if remaining_breakpoints >= 2 && stable_end >= 4 {
        indexes.push((stable_end - 1) * 3 / 4);
    }
    indexes.push(stable_end - 1);
    indexes.sort_unstable();
    indexes.dedup();

    for index in indexes.into_iter().take(remaining_breakpoints) {
        if let Some(message) = messages.get_mut(index) {
            message.add_cache_control_ephemeral();
        }
    }
}

fn is_runtime_context_text(text: &str) -> bool {
    text.trim_start().starts_with("[runtime_context:")
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

/// 工具内容使用 native string/blocks union；不能经 as_text 吞掉 Image/File。
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(untagged)]
enum AnthropicToolResultContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

impl AnthropicToolResultContent {
    fn is_runtime_context(&self) -> bool {
        match self {
            Self::Text(text) => is_runtime_context_text(text),
            Self::Blocks(blocks) => blocks.iter().any(ContentBlock::is_runtime_context),
        }
    }
}

fn tool_result_content(content: &MessageContent) -> Result<AnthropicToolResultContent> {
    Ok(match content {
        MessageContent::Text(text) => AnthropicToolResultContent::Text(text.clone()),
        MessageContent::Empty => AnthropicToolResultContent::Text(String::new()),
        MessageContent::Parts(parts) => AnthropicToolResultContent::Blocks(
            parts
                .iter()
                .map(part_to_content_block)
                .collect::<Result<Vec<_>>>()?,
        ),
    })
}

fn part_to_content_block(part: &ContentPart) -> Result<ContentBlock> {
    Ok(match part {
        ContentPart::Text { text } => ContentBlock::Text {
            text: text.clone(),
            cache_control: None,
        },
        ContentPart::ImageUrl { image_url } => ContentBlock::Image {
            source: checked_image_source(&image_url.url)?,
            cache_control: None,
        },
        ContentPart::ResourceLink { resource } => ContentBlock::Text {
            text: resource.model_text(),
            cache_control: None,
        },
        ContentPart::File { name, content } => file_to_content_block(name, content)?,
    })
}

fn checked_image_source(url: &str) -> Result<ImageSource> {
    let source = data_url_to_image_source(url);
    let invalid = || {
        LlmError::InvalidResponse(
            "Anthropic image source must be a supported base64 image or an HTTP(S) URL".into(),
        )
    };
    match &source {
        ImageSource::Base64 { media_type, data } => {
            if !matches!(
                media_type.as_str(),
                "image/jpeg" | "image/png" | "image/gif" | "image/webp"
            ) || data.is_empty()
                || base64::Engine::decode(&base64::engine::general_purpose::STANDARD, data).is_err()
            {
                return Err(invalid().into());
            }
        }
        ImageSource::Url_ { url } => {
            if !reqwest::Url::parse(url).ok().is_some_and(|parsed| {
                matches!(parsed.scheme(), "https" | "http") && parsed.host_str().is_some()
            }) {
                return Err(invalid().into());
            }
        }
    }
    Ok(source)
}

/// PDF 保留 base64；文本保留完整 UTF-8 原文；其它二进制显式拒绝，不能伪名称占位。
fn file_to_content_block(name: &str, content_base64: &str) -> Result<ContentBlock> {
    let invalid = |reason: &str| {
        LlmError::InvalidResponse(format!(
            "Anthropic file representation unsupported: {reason}"
        ))
    };
    let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, content_base64)
        .map_err(|_| invalid("invalid base64"))?;
    let ext = name
        .rsplit('.')
        .next()
        .map(|value| value.to_ascii_lowercase());
    match ext.as_deref() {
        Some("pdf") => Ok(ContentBlock::Document {
            source: ImageSource::Base64 {
                media_type: "application/pdf".into(),
                data: content_base64.into(),
            },
            cache_control: None,
        }),
        Some("png" | "jpg" | "jpeg" | "gif" | "webp") => {
            let mime = match ext.as_deref() {
                Some("jpg" | "jpeg") => "image/jpeg",
                Some("gif") => "image/gif",
                Some("webp") => "image/webp",
                _ => "image/png",
            };
            Ok(ContentBlock::Image {
                source: checked_image_source(&format!("data:{mime};base64,{content_base64}"))?,
                cache_control: None,
            })
        }
        Some(
            "txt" | "md" | "markdown" | "json" | "xml" | "yaml" | "yml" | "csv" | "tsv" | "rs"
            | "py" | "js" | "ts" | "tsx" | "jsx" | "go" | "java" | "c" | "cpp" | "h" | "sh"
            | "toml" | "ini" | "log" | "sql",
        ) => {
            let text = String::from_utf8(bytes).map_err(|_| invalid("text file is not UTF-8"))?;
            Ok(ContentBlock::Text {
                text: format!("\n[Attachment: {name}]\n```\n{text}\n```"),
                cache_control: None,
            })
        }
        _ => Err(invalid(
            "binary MIME is not representable by the current native content contract",
        )
        .into()),
    }
}

fn parse_tool_input(arguments: &str) -> Result<serde_json::Value> {
    let value: serde_json::Value = serde_json::from_str(arguments)
        .map_err(|_| LlmError::InvalidResponse("Anthropic tool input is not valid JSON".into()))?;
    if !value.is_object() {
        return Err(
            LlmError::InvalidResponse("Anthropic tool input must be an object".into()).into(),
        );
    }
    Ok(value)
}

#[derive(Serialize)]
struct AnthropicToolDef {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    input_schema: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    strict: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<CacheControl>,
}

/// 保原响应存在性；在同一 decode 路径派生 typed 内容与固定公共观察。
#[derive(Deserialize)]
#[serde(transparent)]
struct AnthropicResponse(serde_json::Value);

#[derive(Serialize, Deserialize, Clone)]
struct AnthropicUsage {
    input_tokens: u32,
    output_tokens: u32,
    #[serde(default)]
    cache_creation_input_tokens: Option<u32>,
    #[serde(default)]
    cache_read_input_tokens: Option<u32>,
    #[serde(flatten)]
    extra: std::collections::BTreeMap<String, serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;

    fn text_message(text: &str) -> AnthropicMessage {
        AnthropicMessage {
            role: "user".to_string(),
            content: AnthropicContent::Text(text.to_string()),
        }
    }

    fn has_cache_control(message: &AnthropicMessage) -> bool {
        match &message.content {
            AnthropicContent::Text(_) => false,
            AnthropicContent::Blocks(blocks) => blocks.iter().any(|block| match block {
                ContentBlock::Text { cache_control, .. }
                | ContentBlock::Image { cache_control, .. }
                | ContentBlock::Document { cache_control, .. }
                | ContentBlock::ToolResult { cache_control, .. } => cache_control.is_some(),
                ContentBlock::ToolUse { .. }
                | ContentBlock::Thinking { .. }
                | ContentBlock::RedactedThinking { .. }
                | ContentBlock::Other => false,
            }),
        }
    }

    #[test]
    fn conversation_cache_breakpoints_skip_trailing_runtime_context() {
        let mut messages = vec![
            text_message("older turn"),
            text_message("middle turn"),
            text_message("current request"),
            text_message("[runtime_context:turn]\nvolatile cwd and memory"),
        ];

        apply_conversation_cache_breakpoints(&mut messages, 2);

        assert!(messages.get(2).is_some_and(has_cache_control));
        assert!(
            messages
                .get(3)
                .is_some_and(|message| !has_cache_control(message))
        );
    }

    // ── stage4 P4.1: cache_user_id single-source ────────────────────────────
    // DeepSeek/Anthropic endpoint uses metadata:{user_id} for KVCache isolation.
    // Verify AnthropicClient.convert_request fills metadata when ChatRequest.user_id
    // is set, and omits it entirely when None.

    fn chat_request_with_user(user: Option<&str>) -> ChatRequest {
        ChatRequest {
            messages: vec![Message::user("hi".to_string())],
            user_id: user.map(str::to_string),
            ..ChatRequest::default()
        }
    }

    #[test]
    fn claude_46_max_effort_remains_max() -> Result<()> {
        let (thinking, effort) = build_anthropic_thinking(
            "claude-opus-4-6",
            ThinkingProtocol::AnthropicEffort,
            &Some(echo_core::llm::ThinkingConfig::Level(
                echo_core::llm::ThinkingLevel::Max,
            )),
            8_192,
        )?;
        assert_eq!(effort.as_deref(), Some("max"));
        assert_eq!(thinking.map(|block| block.block_type), Some("adaptive"));
        Ok(())
    }

    #[test]
    fn metadata_user_id_present_when_set() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("ds-xxx".to_string(), "deepseek-chat".to_string());
        let body =
            serde_json::to_value(client.convert_request(&chat_request_with_user(Some("user-7")))?)?;
        assert_eq!(
            body.pointer("/metadata/user_id")
                .and_then(serde_json::Value::as_str),
            Some("user-7")
        );
        Ok(())
    }

    #[test]
    fn metadata_absent_when_user_id_none() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("ds-xxx".to_string(), "deepseek-chat".to_string());
        let body = serde_json::to_value(client.convert_request(&chat_request_with_user(None))?)?;
        // metadata is skip_serializing_if Option::is_none → absent (not null).
        assert!(
            body.get("metadata").is_none(),
            "metadata should be absent when user_id is None, got: {body}"
        );
        Ok(())
    }

    #[test]
    fn multiple_system_messages_are_preserved_in_order()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("sk-test", "claude-sonnet-4-6");
        let request = ChatRequest {
            messages: vec![
                Message::system("base rules".to_string()),
                Message::system("restored context".to_string()),
                Message::user("hello".to_string()),
            ],
            ..ChatRequest::default()
        };
        let body = serde_json::to_value(client.convert_request(&request)?)?;
        assert_eq!(
            body.pointer("/system/0/text")
                .and_then(serde_json::Value::as_str),
            Some("base rules\n\nrestored context")
        );
        Ok(())
    }

    #[test]
    fn message_delta_usage_accepts_output_tokens_only() -> Result<()> {
        let mut adapter = NativeAnthropicStream::default();
        adapter.map(serde_json::json!({"type":"message_start","message":{"id":"msg_usage","usage":{"input_tokens":0,"output_tokens":0}}}))?;
        let chunks = adapter.map(serde_json::json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":17}}))?;
        assert!(chunks.iter().all(|chunk| chunk.finish_reason.is_none()));
        assert!(
            chunks
                .iter()
                .any(|chunk| chunk.delta.provider_response.is_some())
        );
        assert_eq!(adapter.usage.completion_tokens, Some(17));
        Ok(())
    }

    #[test]
    fn tool_start_keeps_provider_block_index() -> Result<()> {
        let mut adapter = NativeAnthropicStream::default();
        adapter.map(serde_json::json!({"type":"message_start","message":{"id":"msg_1"}}))?;
        let chunks = adapter.map(serde_json::json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"call-1","name":"read_file","input":{}}}))?;
        assert_eq!(
            chunks
                .first()
                .and_then(|c| c.delta.tool_calls.as_ref())
                .and_then(|t| t.first())
                .map(|t| t.index),
            Some(1)
        );
        Ok(())
    }

    #[test]
    fn thinking_response_is_projected_to_reasoning_content()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let raw = r#"{"id":"msg-thinking-projection","content":[{"type":"thinking","thinking":"reason","signature":"sig"},{"type":"text","text":"answer"}],"stop_reason":"end_turn","usage":{"input_tokens":2,"output_tokens":3}}"#;
        let response = serde_json::from_str::<AnthropicResponse>(raw)?;
        let converted =
            AnthropicClient::new("sk-test", "claude-sonnet-4-6").convert_response(response)?;
        assert_eq!(
            converted
                .message
                .provider_response
                .as_ref()
                .and_then(|metadata| metadata.request_id.as_deref()),
            Some("msg-thinking-projection")
        );
        assert_eq!(
            converted.message.reasoning_content.as_deref(),
            Some("reason")
        );
        assert_eq!(
            converted.message.content.as_text().as_deref(),
            Some("answer")
        );
        assert_eq!(
            converted.message.reasoning_blocks,
            Some(vec![ReasoningBlock::Signed {
                thinking: "reason".to_string(),
                signature: "sig".to_string(),
            }])
        );
        assert_eq!(
            converted.usage.and_then(|usage| usage.total_tokens),
            Some(5)
        );
        Ok(())
    }

    #[test]
    fn signed_reasoning_blocks_are_replayed_before_tool_calls() -> Result<()> {
        let client = AnthropicClient::new("sk-test", "claude-sonnet-4-6");
        let mut assistant = Message::assistant_with_tools(vec![ToolCall {
            provider_source: None,
            id: "call-1".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "read_file".to_string(),
                arguments: "{}".to_string(),
            },
        }]);
        assistant.reasoning_blocks = Some(vec![ReasoningBlock::Signed {
            thinking: "reason".to_string(),
            signature: "sig".to_string(),
        }]);
        let request = client.convert_request(&ChatRequest::new(vec![assistant]))?;

        let blocks = request
            .messages
            .first()
            .and_then(|message| match &message.content {
                AnthropicContent::Blocks(blocks) => Some(blocks),
                AnthropicContent::Text(_) => None,
            });
        assert!(matches!(
            blocks.and_then(|values| values.first()),
            Some(ContentBlock::Thinking { thinking, signature })
                if thinking == "reason" && signature.as_deref() == Some("sig")
        ));
        assert!(matches!(
            blocks.and_then(|values| values.get(1)),
            Some(ContentBlock::ToolUse { name, .. }) if name == "read_file"
        ));
        Ok(())
    }

    #[test]
    fn native_none_and_text_are_supported_without_silent_json_mode() -> Result<()> {
        let client = AnthropicClient::new("fixture-only", "claude-sonnet-4-5");
        let mut request = ChatRequest::new(vec![Message::user("hello".into())]);
        request.tool_choice = Some(ToolChoice::None);
        request.response_format = Some(ResponseFormat::Text);
        let wire = serde_json::to_value(client.convert_request(&request)?)
            .map_err(|error| LlmError::SerializationError(error.to_string()))?;
        assert_eq!(
            wire.get("tool_choice"),
            Some(&serde_json::json!({"type":"none"}))
        );
        assert!(wire.get("output_config").is_none());
        request.response_format = Some(ResponseFormat::JsonObject);
        assert!(client.convert_request(&request).is_err());
        Ok(())
    }

    /// Sprint 2: main think path sends `cache_hints: Some` with **empty**
    /// breakpoints (agent computes layout for the hash but leaves breakpoint
    /// derivation to the provider). The provider MUST fall back to
    /// `AnthropicCachePlan::from_layout` and place cache_control — not silently
    /// emit zero breakpoints (pre-fix bug: has_system/tool=false,
    /// history_breakpoint_count=0 → no cache_control on the main path).
    #[test]
    fn cache_hints_with_empty_breakpoints_still_places_cache_control()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        use echo_core::llm::cache::CacheHints;
        use echo_core::llm::types::{FunctionSpec, Message, ToolDefinition};

        let messages = vec![
            Message::system("You are Echo Agent".to_string()),
            Message::user("h1".to_string()),
            Message::user("h2".to_string()),
            Message::user("h3".to_string()),
            Message::user("h4".to_string()),
        ];
        let tools = vec![ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionSpec {
                strict: None,
                name: "t".to_string(),
                description: "d".to_string(),
                parameters: serde_json::json!({}),
            },
        }];
        // Main-path shape: Some(CacheHints { breakpoints: vec![], .. }). The
        // provider recomputes the layout from request.messages+tools itself
        // (it does not read hints.segments), so segments can be default.
        let req = ChatRequest {
            messages,
            tools: Some(tools),
            cache_hints: Some(CacheHints {
                breakpoints: vec![],
                stable_prefix_hash: Some("deadbeef".to_string()),
                segments: Default::default(),
            }),
            ..ChatRequest::default()
        };

        let client = AnthropicClient::new("sk-xxx".to_string(), "claude-sonnet-4-6".to_string());
        let body = serde_json::to_value(client.convert_request(&req)?)?;
        // Before fix: zero cache_control. After: from_layout places system +
        // tools + history breakpoints.
        let body_str = body.to_string();
        assert!(
            body_str.contains("cache_control"),
            "main-path (empty breakpoints) must still place cache_control via from_layout fallback; got: {body_str}"
        );
        assert!(
            body_str.contains("ephemeral"),
            "cache_control must be ephemeral; got: {body_str}"
        );
        Ok(())
    }

    // ── 2B: file_to_content_block dispatches by inferred media type ──────────

    #[test]
    fn pdf_attachment_becomes_document_block() -> std::result::Result<(), String> {
        let b64 = base64::engine::general_purpose::STANDARD.encode(b"%PDF-1.4 fake");
        let block = file_to_content_block("report.pdf", &b64).map_err(|error| error.to_string())?;
        let ContentBlock::Document { source, .. } = block else {
            return Err("expected Document block".to_string());
        };
        let ImageSource::Base64 { media_type, data } = source else {
            return Err("expected base64 document source".to_string());
        };
        assert_eq!(media_type, "application/pdf");
        assert_eq!(data, b64);
        Ok(())
    }

    #[test]
    fn text_attachment_inlined_as_text() -> std::result::Result<(), String> {
        let b64 = base64::engine::general_purpose::STANDARD.encode(b"hello notes");
        let block = file_to_content_block("notes.txt", &b64).map_err(|error| error.to_string())?;
        let ContentBlock::Text { text, .. } = block else {
            return Err("expected Text block".to_string());
        };
        assert!(
            text.contains("hello notes"),
            "text should contain decoded content"
        );
        assert!(
            text.contains("notes.txt"),
            "text should mention the filename"
        );
        Ok(())
    }

    #[test]
    fn binary_non_pdf_attachment_is_explicitly_unsupported() {
        let b64 = base64::engine::general_purpose::STANDARD.encode(b"\x00\x01binary");
        assert!(file_to_content_block("archive.zip", &b64).is_err());
    }

    #[test]
    fn literal_stream_events_preserve_usage_tool_identity_and_partial_json() -> Result<()> {
        let mut adapter = NativeAnthropicStream::default();
        adapter.map(serde_json::json!({"type":"message_start","message":{"id":"msg_literal","usage":{"input_tokens":9,"output_tokens":0}}}))?;
        adapter.map(serde_json::json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"call-1","name":"read_file","input":{}}}))?;
        let delta = adapter.map(serde_json::json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"中文.md\"}"}}))?;
        assert_eq!(
            delta
                .first()
                .and_then(|c| c.delta.tool_calls.as_ref())
                .and_then(|c| c.first())
                .and_then(|c| c.function.as_ref())
                .and_then(|c| c.arguments.as_deref()),
            Some("{\"path\":\"中文.md\"}")
        );
        adapter.map(serde_json::json!({"type":"content_block_stop","index":1}))?;
        adapter.map(serde_json::json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":7}}))?;
        let terminal = adapter.map(serde_json::json!({"type":"message_stop"}))?;
        assert_eq!(
            terminal.first().and_then(|c| c.finish_reason.as_deref()),
            Some("tool_calls")
        );
        Ok(())
    }
}

/// Native provider source alone orders replay. Runtime-approved presentation
/// blocks are a separate projection and are never serialized to vendor wire.
fn native_anthropic_history(
    message: &Message,
    allow_unsigned: bool,
    provider: &str,
) -> Result<Option<Vec<ContentBlock>>> {
    let native = message.assistant_blocks.as_deref().unwrap_or_default();
    let tools = message.tool_calls.as_deref().unwrap_or_default();
    let observation = message
        .provider_response
        .as_ref()
        .filter(|metadata| metadata.protocol == LlmApiProtocol::Anthropic);
    // D2未知原生身份只供检查；不能因缺Source而退回普通历史路径。
    if observation.is_some_and(|metadata| metadata.request_id.as_deref().is_none_or(str::is_empty))
    {
        return Err(LlmError::InvalidResponse(
            "Anthropic native replay omitted observed request id".into(),
        )
        .into());
    }
    let has_native = native.iter().any(|block| {
        block
            .provider_source
            .as_ref()
            .is_some_and(|source| source.protocol == LlmApiProtocol::Anthropic)
    }) || tools.iter().any(|tool| {
        tool.provider_source
            .as_ref()
            .is_some_and(|source| source.protocol == LlmApiProtocol::Anthropic)
    });
    if !has_native {
        return Ok(None);
    }
    let invalid =
        |reason: &str| LlmError::InvalidResponse(format!("Anthropic native replay {reason}"));
    // 先沿既有canonical身份收齐前缀，再用原生Source决定可重放组与顺序。
    // None本身不授权wire；只有同组后继真实Source可补全，不能借其它组/协议的Source。
    let mut canonical: Vec<AssistantMessageBlock> = Vec::new();
    for block in native {
        if let Some(saved) = canonical
            .iter_mut()
            .find(|saved| saved.request_id == block.request_id && saved.block_id == block.block_id)
        {
            match (&saved.provider_source, &block.provider_source) {
                (None, Some(_)) => saved.provider_source = block.provider_source.clone(),
                (Some(before), Some(after)) if before == after => {}
                (None, None) => {}
                _ => return Err(invalid("source changed or was removed").into()),
            }
            if let Some(after) = block.provider.as_ref() {
                if saved
                    .provider
                    .as_ref()
                    .is_some_and(|before| before != after)
                {
                    return Err(invalid("canonical provider changed").into());
                }
                // provider也是单调观察；缺字段不能擦除已知身份或跳过中间foreign段。
                saved.provider = Some(after.clone());
            }
            saved.delta.push_str(&block.delta);
            saved.complete |= block.complete;
            if block.phase != AssistantPhase::Unknown {
                saved.phase = block.phase;
            }
            if block.reasoning.is_some() {
                saved.reasoning = block.reasoning.clone();
            }
        } else {
            canonical.push(block.clone());
        }
    }
    let mut merged: std::collections::BTreeMap<u32, AssistantMessageBlock> =
        std::collections::BTreeMap::new();
    let mut owner = None::<String>;
    for block in canonical.into_iter().filter(|block| {
        block
            .provider_source
            .as_ref()
            .is_some_and(|source| source.protocol == LlmApiProtocol::Anthropic)
    }) {
        let source = block
            .provider_source
            .as_ref()
            .ok_or_else(|| invalid("source omitted"))?;
        if source.request_id.is_empty()
            || observation.is_some_and(|metadata| {
                metadata.request_id.as_deref() != Some(source.request_id.as_str())
            })
        {
            return Err(invalid("source request id is empty or differs from observation").into());
        }
        if observation.is_some_and(|metadata| {
            block
                .provider
                .as_ref()
                .is_some_and(|owner| owner != &metadata.provider)
        }) {
            return Err(invalid("canonical provider differs from observation").into());
        }
        if owner
            .as_ref()
            .is_some_and(|owner| owner != &source.request_id)
        {
            return Err(invalid("mixed request owners").into());
        }
        owner.get_or_insert_with(|| source.request_id.clone());
        if merged.insert(source.output_index, block).is_some() {
            return Err(invalid("duplicate native position across canonical groups").into());
        }
    }
    let mut ordered = std::collections::BTreeMap::new();
    for (index, block) in merged {
        let value = match block.reasoning {
            Some(ReasoningBlock::Signed {
                thinking,
                signature,
            }) => {
                if signature.is_empty() {
                    return Err(invalid("thinking signature is empty").into());
                }
                ContentBlock::Thinking {
                    thinking,
                    signature: Some(signature),
                }
            }
            Some(ReasoningBlock::Redacted { data }) => ContentBlock::RedactedThinking { data },
            Some(ReasoningBlock::Opaque {
                provider: owner,
                data,
                ..
            }) if allow_unsigned && owner == provider => unsigned_thinking_replay(&data)?,
            Some(ReasoningBlock::Opaque { .. }) => {
                return Err(invalid("foreign reasoning payload").into());
            }
            None if block.phase != AssistantPhase::Reasoning => ContentBlock::Text {
                text: block.delta,
                cache_control: None,
            },
            None => return Err(invalid("thinking signature missing").into()),
        };
        ordered.insert(index, value);
    }
    for tool in tools {
        let source = tool
            .provider_source
            .as_ref()
            .filter(|source| source.protocol == LlmApiProtocol::Anthropic)
            .ok_or_else(|| invalid("tool source missing"))?;
        if source.request_id.is_empty()
            || observation.is_some_and(|metadata| {
                metadata.request_id.as_deref() != Some(source.request_id.as_str())
            })
        {
            return Err(invalid("tool request id is empty or differs from observation").into());
        }
        if owner
            .as_ref()
            .is_some_and(|owner| owner != &source.request_id)
        {
            return Err(invalid("tool request owner changed").into());
        }
        owner.get_or_insert_with(|| source.request_id.clone());
        let input = serde_json::from_str(&tool.function.arguments)
            .map_err(|error| invalid(&format!("tool arguments invalid: {error}")))?;
        if ordered
            .insert(
                source.output_index,
                ContentBlock::ToolUse {
                    id: tool.id.clone(),
                    name: tool.function.name.clone(),
                    input,
                },
            )
            .is_some()
        {
            return Err(invalid("duplicate native position").into());
        }
    }
    Ok(Some(ordered.into_values().collect()))
}

#[derive(Default)]
struct NativeAnthropicStream {
    request_id: String,
    provider: String,
    sequence: u64,
    blocks: std::collections::BTreeMap<usize, serde_json::Value>,
    seen_indexes: std::collections::HashSet<usize>,
    has_native_content: bool,
    deepseek_dialect: bool,
    reasoning: std::collections::HashMap<usize, ReasoningBlock>,
    tools: std::collections::HashMap<usize, (String, String, String)>,
    usage: Usage,
    finish: Option<String>,
    has_output: bool,
    output_text: String,
    completed_tools: Vec<ToolCall>,
    provider_response: Option<ProviderResponseMetadata>,
    observed_blocks: Vec<AssistantMessageBlock>,
}

impl NativeAnthropicStream {
    fn native_request_id(&self) -> Option<&str> {
        self.provider_response
            .as_ref()
            .and_then(|metadata| metadata.request_id.as_deref())
            .filter(|id| !id.is_empty())
    }

    fn block(
        &mut self,
        index: usize,
        phase: AssistantPhase,
        delta: String,
        complete: bool,
        reasoning: Option<ReasoningBlock>,
    ) -> Result<AssistantMessageBlock> {
        let output_index = u32::try_from(index)
            .map_err(|_| LlmError::InvalidResponse("Anthropic block index exceeds u32".into()))?;
        let kind = match &reasoning {
            Some(ReasoningBlock::Redacted { .. }) => "redacted_thinking",
            _ if phase == AssistantPhase::Reasoning => "thinking",
            _ => "text",
        };
        let block = AssistantMessageBlock {
            provider_source: self.native_request_id().map(|native_id| {
                output_source(
                    LlmApiProtocol::Anthropic,
                    native_id,
                    &format!("{}:content:{index}", self.request_id),
                    output_index,
                    None,
                    kind,
                    None,
                )
            }),
            block_id: format!("{}:content:{index}", self.request_id),
            request_id: self.request_id.clone(),
            sequence: self.sequence,
            phase,
            delta,
            complete,
            provider: Some(self.provider.clone()),
            reasoning,
        };
        self.sequence = self.sequence.saturating_add(1);
        Ok(block)
    }

    fn canonical(
        &mut self,
        index: usize,
        phase: AssistantPhase,
        text: String,
        complete: bool,
        reasoning: Option<ReasoningBlock>,
    ) -> Result<ChatChunk> {
        let block = self.block(index, phase, text.clone(), complete, reasoning.clone())?;
        self.observed_blocks.push(block.clone());
        Ok(ChatChunk {
            delta: DeltaMessage {
                content: (phase != AssistantPhase::Reasoning && !text.is_empty())
                    .then_some(text.clone()),
                reasoning_content: (phase == AssistantPhase::Reasoning && !text.is_empty())
                    .then_some(text),
                reasoning_blocks: reasoning.map(|value| vec![value]),
                assistant_blocks: Some(vec![block]),
                ..Default::default()
            },
            finish_reason: None,
            usage: None,
        })
    }

    fn metadata_chunk(&self) -> ChatChunk {
        ChatChunk {
            delta: DeltaMessage {
                provider_response: self.provider_response.clone(),
                ..Default::default()
            },
            finish_reason: None,
            usage: None,
        }
    }

    fn update_metadata(&mut self, event: &serde_json::Value) -> Result<()> {
        let metadata = self.provider_response.as_mut().ok_or_else(|| {
            LlmError::InvalidResponse("Anthropic metadata update preceded message_start".into())
        })?;
        if let Some(delta) = event.get("delta") {
            if !delta.is_object() {
                return Err(LlmError::InvalidResponse(
                    "Anthropic message_delta.delta must be object".into(),
                )
                .into());
            }
            if delta.get("stop_reason").is_some() {
                metadata.stop_reason = delta.get("stop_reason").cloned();
            }
            if delta.get("stop_sequence").is_some() {
                metadata.stop_sequence = delta.get("stop_sequence").cloned();
            }
        }
        if let Some(update) = event.get("usage").cloned() {
            if let Some(saved) = metadata.usage.as_mut() {
                merge_anthropic_usage_snapshot(saved, &update);
            } else {
                metadata.usage = Some(update);
            }
        }
        // serde是唯一共享shape校验；缺失/null和未知嵌套值沿其公开合同。
        *metadata = serde_json::from_value(
            serde_json::to_value(&*metadata)
                .map_err(|error| LlmError::SerializationError(error.to_string()))?,
        )
        .map_err(|error| {
            LlmError::InvalidResponse(format!("Anthropic response observation invalid: {error}"))
        })?;
        Ok(())
    }

    /// 用同一canonical事实构造失败观察；不会发布、执行工具或建立完成资格。
    fn incomplete_error(&mut self) -> Result<echo_core::error::ReactError> {
        let mut blocks = self.observed_blocks.clone();
        let mut active_reasoning = self
            .reasoning
            .iter()
            .map(|(index, reasoning)| (*index, reasoning.clone()))
            .collect::<Vec<_>>();
        active_reasoning.sort_by_key(|(index, _)| *index);
        for (index, reasoning) in active_reasoning {
            let reasoning = match reasoning {
                ReasoningBlock::Signed {
                    thinking,
                    signature,
                } if self.deepseek_dialect && signature.is_empty() => thinking_reasoning(
                    &thinking,
                    self.blocks
                        .get(&index)
                        .and_then(|block| block.get("signature"))
                        .and_then(serde_json::Value::as_str),
                    &self.provider,
                    &format!("{}:content:{index}", self.request_id),
                    true,
                )?,
                other => other,
            };
            blocks.push(self.block(
                index,
                AssistantPhase::Reasoning,
                String::new(),
                false,
                Some(reasoning),
            )?);
        }
        let mut reasoning_by_index = std::collections::BTreeMap::new();
        let block_prefix = format!("{}:content:", self.request_id);
        for block in &blocks {
            if let Some(reasoning) = block.reasoning.as_ref() {
                // 这是本adapter生成的内部块坐标；未知native ID时不能借Source排序。
                let index = block
                    .block_id
                    .strip_prefix(block_prefix.as_str())
                    .and_then(|index| index.parse::<u32>().ok())
                    .ok_or_else(|| {
                        LlmError::InvalidResponse(
                            "Anthropic partial block omitted internal index".into(),
                        )
                    })?;
                reasoning_by_index.insert(index, reasoning.clone());
            }
        }
        let reasoning_blocks = reasoning_by_index.into_values().collect::<Vec<_>>();
        let mut reasoning_text = String::new();
        for reasoning in &reasoning_blocks {
            match reasoning {
                ReasoningBlock::Signed { thinking, .. } => reasoning_text.push_str(thinking),
                ReasoningBlock::Opaque { data, .. } => {
                    let raw: serde_json::Value = serde_json::from_str(data)
                        .map_err(|error| LlmError::InvalidResponse(error.to_string()))?;
                    if let Some(text) = raw.get("thinking").and_then(serde_json::Value::as_str) {
                        reasoning_text.push_str(text);
                    }
                }
                ReasoningBlock::Redacted { .. } => {}
            }
        }
        let mut tools = self.completed_tools.clone();
        for (index, (id, name, arguments)) in &self.tools {
            let index_u32 = u32::try_from(*index).map_err(|_| {
                LlmError::InvalidResponse("Anthropic partial tool index exceeds u32".into())
            })?;
            let arguments = if arguments.is_empty() {
                self.blocks
                    .get(index)
                    .and_then(|block| block.get("input"))
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({}))
                    .to_string()
            } else {
                arguments.clone()
            };
            tools.push(ToolCall {
                provider_source: Some(output_source(
                    LlmApiProtocol::Anthropic,
                    self.native_request_id().unwrap_or_default(),
                    id,
                    index_u32,
                    None,
                    "tool_use",
                    None,
                )),
                id: id.clone(),
                call_type: "function".into(),
                function: FunctionCall {
                    name: name.clone(),
                    arguments,
                },
            });
        }
        tools.sort_by_key(|tool| {
            tool.provider_source
                .as_ref()
                .map(|source| source.output_index)
        });
        let partial = Message {
            role: Role::Assistant,
            content: if self.output_text.is_empty() {
                MessageContent::Empty
            } else {
                MessageContent::Text(self.output_text.clone())
            },
            tool_calls: (!tools.is_empty()).then_some(tools),
            reasoning_content: (!reasoning_text.is_empty()).then_some(reasoning_text),
            reasoning_blocks: (!reasoning_blocks.is_empty()).then_some(reasoning_blocks),
            assistant_blocks: (!blocks.is_empty()).then_some(blocks),
            provider_response: self.provider_response.clone(),
            execution_status: None,
            approved_assistant_blocks: None,
            ..Default::default()
        };
        Ok(LlmError::IncompleteResponse {
            partial: Box::new(partial),
        }
        .into())
    }

    fn merge_usage(&mut self, value: &serde_json::Value, from_delta: bool) -> Result<()> {
        if !from_delta && value.is_object() {
            serde_json::from_value::<AnthropicUsage>(value.clone()).map_err(|error| {
                LlmError::InvalidResponse(format!("Anthropic message_start usage invalid: {error}"))
            })?;
        }
        let read = |name: &str| -> Result<Option<u32>> {
            match value.get(name) {
                None => Ok(None),
                Some(serde_json::Value::Null)
                    if name.starts_with("cache_") || (from_delta && name == "input_tokens") =>
                {
                    Ok(None)
                }
                Some(value) => value
                    .as_u64()
                    .and_then(|number| u32::try_from(number).ok())
                    .map(Some)
                    .ok_or_else(|| {
                        LlmError::InvalidResponse(format!(
                            "Anthropic usage.{name} is not representable as u32"
                        ))
                        .into()
                    }),
            }
        };
        if let Some(count) = read("input_tokens")? {
            self.usage.prompt_tokens = Some(count);
        }
        if let Some(count) = read("output_tokens")? {
            self.usage.completion_tokens = Some(count);
        }
        if let Some(count) = read("cache_creation_input_tokens")? {
            self.usage.cache_creation_input_tokens = Some(count);
        }
        if let Some(count) = read("cache_read_input_tokens")? {
            self.usage.cache_read_input_tokens = Some(count);
        }
        self.usage.total_tokens = self
            .usage
            .prompt_tokens
            .zip(self.usage.completion_tokens)
            .map(|(input, output)| input.saturating_add(output));
        Ok(())
    }

    fn map(&mut self, mut event: serde_json::Value) -> Result<Vec<ChatChunk>> {
        strip_anthropic_framework_extensions(&mut event);
        use serde_json::Value;
        let invalid = |text: &str| LlmError::InvalidResponse(format!("Anthropic stream {text}"));
        let kind = event
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("event omitted type"))?;
        if self.finish.is_some()
            && !matches!(kind, "message_delta" | "message_stop" | "ping" | "error")
        {
            return Err(invalid("content followed message_delta").into());
        }
        let mut chunks = Vec::new();
        match kind {
            "message_start" => {
                if !self.request_id.is_empty() {
                    return Err(invalid("duplicate message_start").into());
                }
                let message = event
                    .get("message")
                    .ok_or_else(|| invalid("omitted message_start body"))?;
                self.provider_response =
                    Some(anthropic_response_metadata(message, &self.provider)?);
                self.request_id = message
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                    .map(ToString::to_string)
                    .unwrap_or_else(|| super::native_message::fallback_request_id("anthropic"));
                if let Some(usage) = message.get("usage") {
                    self.merge_usage(usage, false)?;
                }
                chunks.push(self.metadata_chunk());
            }
            "content_block_start" => {
                let index = event
                    .get("index")
                    .and_then(Value::as_u64)
                    .and_then(|n| usize::try_from(n).ok())
                    .ok_or_else(|| invalid("omitted block index"))?;
                let block = event
                    .get("content_block")
                    .ok_or_else(|| invalid("omitted content_block"))?
                    .clone();
                let decoded: ContentBlock = serde_json::from_value(block.clone())
                    .map_err(|error| invalid(&format!("content block start invalid: {error}")))?;
                match &decoded {
                    ContentBlock::Text { .. } => {}
                    ContentBlock::Thinking { .. } => {
                        if block
                            .get("signature")
                            .is_some_and(|value| !value.is_string())
                        {
                            return Err(invalid(
                                "thinking signature must be a string when present",
                            )
                            .into());
                        }
                    }
                    ContentBlock::ToolUse { id, name, input } => {
                        if id.is_empty()
                            || name.is_empty()
                            || !input.is_object()
                            || self.tools.values().any(|(other, _, _)| other == id)
                            || self.completed_tools.iter().any(|tool| tool.id == *id)
                        {
                            return Err(invalid("tool start has malformed identity/input").into());
                        }
                    }
                    ContentBlock::RedactedThinking { data }
                        if !data.is_empty() && !self.deepseek_dialect => {}
                    _ => return Err(invalid("unknown or unsupported content block start").into()),
                }
                if self.request_id.is_empty()
                    || !self.seen_indexes.insert(index)
                    || self.blocks.insert(index, block.clone()).is_some()
                {
                    return Err(invalid("block start preceded message or duplicated index").into());
                }
                self.has_native_content = true;
                match decoded {
                    ContentBlock::Text { text, .. } => {
                        if !text.is_empty() {
                            self.has_output = true;
                            self.output_text.push_str(&text);
                            chunks.push(self.canonical(
                                index,
                                AssistantPhase::Unknown,
                                text,
                                false,
                                None,
                            )?);
                        }
                    }
                    ContentBlock::Thinking {
                        thinking,
                        signature,
                    } => {
                        self.reasoning.insert(
                            index,
                            ReasoningBlock::Signed {
                                thinking: thinking.clone(),
                                signature: signature.unwrap_or_default(),
                            },
                        );
                        if !thinking.is_empty() {
                            chunks.push(self.canonical(
                                index,
                                AssistantPhase::Reasoning,
                                thinking,
                                false,
                                None,
                            )?);
                        }
                    }
                    ContentBlock::RedactedThinking { data } => {
                        self.reasoning
                            .insert(index, ReasoningBlock::Redacted { data });
                    }
                    ContentBlock::ToolUse { id, name, .. } => {
                        self.tools
                            .insert(index, (id.clone(), name.clone(), String::new()));
                        chunks.push(anthropic_tool_chunk(index, Some(id), Some(name), None)?);
                    }
                    _ => return Err(invalid("unsupported decoded content block start").into()),
                }
            }
            "content_block_delta" => {
                let index = event
                    .get("index")
                    .and_then(Value::as_u64)
                    .and_then(|n| usize::try_from(n).ok())
                    .ok_or_else(|| invalid("omitted block index"))?;
                let block = self
                    .blocks
                    .get(&index)
                    .ok_or_else(|| invalid("delta preceded block start"))?;
                let block_kind = block
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let delta = event.get("delta").ok_or_else(|| invalid("omitted delta"))?;
                match delta.get("type").and_then(Value::as_str) {
                    Some("text_delta") if block_kind == "text" => {
                        let text = delta
                            .get("text")
                            .and_then(Value::as_str)
                            .ok_or_else(|| invalid("text delta omitted text"))?;
                        self.has_output |= !text.is_empty();
                        self.output_text.push_str(text);
                        chunks.push(self.canonical(
                            index,
                            AssistantPhase::Unknown,
                            text.to_string(),
                            false,
                            None,
                        )?);
                    }
                    Some("thinking_delta") if block_kind == "thinking" => {
                        let text = delta
                            .get("thinking")
                            .and_then(Value::as_str)
                            .ok_or_else(|| invalid("thinking delta omitted text"))?;
                        if let Some(ReasoningBlock::Signed { thinking, .. }) =
                            self.reasoning.get_mut(&index)
                        {
                            thinking.push_str(text);
                        }
                        chunks.push(self.canonical(
                            index,
                            AssistantPhase::Reasoning,
                            text.to_string(),
                            false,
                            None,
                        )?);
                    }
                    Some("signature_delta") if block_kind == "thinking" => {
                        let text = delta
                            .get("signature")
                            .and_then(Value::as_str)
                            .ok_or_else(|| invalid("signature delta omitted signature"))?;
                        if let Some(ReasoningBlock::Signed { signature, .. }) =
                            self.reasoning.get_mut(&index)
                        {
                            signature.push_str(text);
                            if let Some(fields) =
                                self.blocks.get_mut(&index).and_then(Value::as_object_mut)
                            {
                                fields.insert("signature".into(), Value::String(signature.clone()));
                            }
                        }
                    }
                    Some("input_json_delta") if block_kind == "tool_use" => {
                        let partial = delta
                            .get("partial_json")
                            .and_then(Value::as_str)
                            .ok_or_else(|| invalid("input delta omitted partial_json"))?;
                        if let Some((_, _, args)) = self.tools.get_mut(&index) {
                            args.push_str(partial);
                        }
                        chunks.push(anthropic_tool_chunk(
                            index,
                            None,
                            None,
                            Some(partial.to_string()),
                        )?);
                    }
                    _ => return Err(invalid("delta type disagreed with block type").into()),
                }
            }
            "content_block_stop" => {
                let index = event
                    .get("index")
                    .and_then(Value::as_u64)
                    .and_then(|n| usize::try_from(n).ok())
                    .ok_or_else(|| invalid("omitted block index"))?;
                let block = self
                    .blocks
                    .remove(&index)
                    .ok_or_else(|| invalid("stop preceded block start or duplicated stop"))?;
                if let Some((id, name, args)) = self.tools.remove(&index) {
                    let arguments = if args.is_empty() {
                        block
                            .get("input")
                            .cloned()
                            .unwrap_or_else(|| serde_json::json!({}))
                            .to_string()
                    } else {
                        args.clone()
                    };
                    let value: Value = serde_json::from_str(&arguments)
                        .map_err(|error| invalid(&format!("invalid tool arguments: {error}")))?;
                    if !value.is_object() {
                        return Err(invalid("tool arguments must be an object").into());
                    }
                    self.completed_tools.push(ToolCall {
                        provider_source: Some(output_source(
                            LlmApiProtocol::Anthropic,
                            self.native_request_id().unwrap_or_default(),
                            &id,
                            u32::try_from(index).map_err(|_| invalid("tool index exceeds u32"))?,
                            None,
                            "tool_use",
                            None,
                        )),
                        id,
                        call_type: "function".into(),
                        function: FunctionCall {
                            name,
                            arguments: arguments.clone(),
                        },
                    });
                    if args.is_empty() {
                        chunks.push(anthropic_tool_chunk(index, None, None, Some(arguments))?);
                    }
                    self.has_output = true;
                } else if let Some(reasoning) = self.reasoning.remove(&index) {
                    let reasoning = match reasoning {
                        ReasoningBlock::Signed {
                            thinking,
                            signature,
                        } if signature.is_empty() => thinking_reasoning(
                            &thinking,
                            block.get("signature").and_then(Value::as_str),
                            &self.provider,
                            &format!("{}:content:{index}", self.request_id),
                            self.deepseek_dialect,
                        )?,
                        other => other,
                    };
                    chunks.push(self.canonical(
                        index,
                        AssistantPhase::Reasoning,
                        String::new(),
                        true,
                        Some(reasoning),
                    )?);
                } else {
                    chunks.push(self.canonical(
                        index,
                        AssistantPhase::Unknown,
                        String::new(),
                        true,
                        None,
                    )?);
                }
            }
            "message_delta" => {
                if self.request_id.is_empty() {
                    return Err(invalid("message_delta preceded message_start").into());
                }
                if let Some(usage) = event.get("usage") {
                    self.merge_usage(usage, true)?;
                }
                self.update_metadata(&event)?;
                let reason = self
                    .provider_response
                    .as_ref()
                    .and_then(|metadata| metadata.stop_reason.as_ref())
                    .and_then(Value::as_str);
                self.finish = match reason {
                    None => None,
                    Some(reason) => match anthropic_finish_reason(Some(reason)) {
                        Ok(finish) => finish,
                        Err(_) => return Err(self.incomplete_error()?),
                    },
                };
                if !self.blocks.is_empty() && self.finish.is_some() {
                    return Err(self.incomplete_error()?);
                }
                chunks.push(self.metadata_chunk());
            }
            "message_stop" => {
                if !self.has_native_content {
                    return Err(LlmError::EmptyResponse.into());
                }
                if self.finish.is_none()
                    || !self.blocks.is_empty()
                    || (!self.has_output
                        && !(self.finish.as_deref() == Some("continue") && self.has_native_content))
                {
                    return Err(self.incomplete_error()?);
                }
                let message = Message {
                    content: MessageContent::Text(self.output_text.clone()),
                    tool_calls: (!self.completed_tools.is_empty())
                        .then(|| self.completed_tools.clone()),
                    ..Default::default()
                };
                if self.finish.as_deref() != Some("continue") {
                    super::native_message::validate_completion(&message, self.finish.as_deref())?;
                } else if self.has_output {
                    super::native_message::validate_completion(&message, Some("stop"))?;
                }
                if self.native_request_id().is_none() {
                    return Err(invalid("completed response omitted native id").into());
                }
                chunks.push(ChatChunk {
                    delta: DeltaMessage {
                        provider_response: self.provider_response.clone(),
                        ..Default::default()
                    },
                    finish_reason: self.finish.clone(),
                    usage: self
                        .provider_response
                        .as_ref()
                        .and_then(|observation| observation.usage.as_ref())
                        .filter(|usage| usage.is_object())
                        .map(|_| self.usage.clone()),
                });
            }
            "error" => {
                // 保留流内原错误type/message及其它字段；没有真实HTTP失败码时不捏造status。
                let detail = event.get("error").unwrap_or(&event);
                return Err(super::client::reported_provider_error(&self.provider, detail).into());
            }
            "ping" => {}
            _ => {}
        }
        for chunk in &mut chunks {
            for tool in chunk.delta.tool_calls.as_mut().into_iter().flatten() {
                let index =
                    usize::try_from(tool.index).map_err(|_| invalid("tool index exceeds usize"))?;
                let id = self
                    .tools
                    .get(&index)
                    .map(|value| value.0.as_str())
                    .or_else(|| {
                        self.completed_tools
                            .iter()
                            .find(|call| {
                                call.provider_source
                                    .as_ref()
                                    .is_some_and(|source| source.output_index == tool.index)
                            })
                            .map(|call| call.id.as_str())
                    })
                    .ok_or_else(|| invalid("tool source identity missing"))?;
                tool.provider_source = self.native_request_id().map(|native_id| {
                    output_source(
                        LlmApiProtocol::Anthropic,
                        native_id,
                        id,
                        tool.index,
                        None,
                        "tool_use",
                        None,
                    )
                });
            }
        }
        Ok(chunks)
    }
}

fn anthropic_tool_chunk(
    index: usize,
    id: Option<String>,
    name: Option<String>,
    arguments: Option<String>,
) -> Result<ChatChunk> {
    let index = u32::try_from(index)
        .map_err(|_| LlmError::InvalidResponse("Anthropic tool index exceeds u32".into()))?;
    Ok(ChatChunk {
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
    })
}

fn adapt_anthropic_stream<S>(
    raw_stream: S,
    provider: String,
    deepseek_dialect: bool,
) -> BoxStream<'static, Result<ChatChunk>>
where
    S: futures::Stream<Item = Result<JsonSseEvent>> + Send + 'static,
{
    Box::pin(async_stream::try_stream! {
        let mut adapter = NativeAnthropicStream { provider, deepseek_dialect, ..Default::default() };
        futures::pin_mut!(raw_stream);
        while let Some(event) = raw_stream.next().await {
            match event? {
                JsonSseEvent::Done => Err(LlmError::InvalidResponse("Anthropic [DONE] preceded message_stop".into()))?,
                JsonSseEvent::Data(value) => {
                    let stopped = value.get("type").and_then(serde_json::Value::as_str) == Some("message_stop");
                    for chunk in adapter.map(value)? { yield chunk; }
                    if stopped { return; }
                }
            }
        }
        // 配置身份本身不是原生观察；已接纳的实际字段（含显式null）或内容才构成D2 partial。
        let observed = adapter.provider_response.as_ref().is_some_and(|metadata| {
            adapter.has_native_content
                || metadata.request_id.is_some()
                || metadata.model.is_some()
                || metadata.stop_reason.is_some()
                || metadata.stop_sequence.is_some()
                || metadata.usage.is_some()
        });
        if observed { Err(adapter.incomplete_error()?)?; }
        Err(LlmError::InvalidResponse("Anthropic SSE EOF before message_stop".into()))?;
    })
}

#[cfg(test)]
mod native_replay_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn provider_native_anthropic_signed_blocks_keep_order_and_replay() -> Result<()> {
        let client = AnthropicClient::new("test-key", "test-model");
        let raw = json!({"id":"msg_native","content":[{"type":"thinking","thinking":"中文推理","signature":"signature-完整"},{"type":"text","text":"公开进度"},{"type":"tool_use","id":"call_native","name":"lookup","input":{}},{"type":"redacted_thinking","data":"opaque"},{"type":"text","text":"后续正文"}],"stop_reason":"tool_use"});
        let response = client.convert_response(
            serde_json::from_value(raw.clone())
                .map_err(|error| LlmError::InvalidResponse(error.to_string()))?,
        )?;
        let request = client.convert_request(&ChatRequest::new(vec![response.message]))?;
        let wire = serde_json::to_value(request)
            .map_err(|error| LlmError::InvalidResponse(error.to_string()))?;
        let content = wire
            .pointer("/messages/0/content")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| LlmError::InvalidResponse("missing replay content".into()))?;
        let kinds = content
            .iter()
            .filter_map(|b| b.get("type").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            vec!["thinking", "text", "tool_use", "redacted_thinking", "text"]
        );
        assert_eq!(
            content
                .first()
                .and_then(|b| b.get("signature"))
                .and_then(serde_json::Value::as_str),
            Some("signature-完整")
        );
        Ok(())
    }

    #[test]
    fn provider_native_anthropic_stream_retains_full_thinking_and_signature() -> Result<()> {
        let mut adapter = NativeAnthropicStream {
            provider: "anthropic".into(),
            ..Default::default()
        };
        let events = [
            json!({"type":"message_start","message":{"id":"msg_sig"}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"中文😀"}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig-A"}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"-B"}}),
            json!({"type":"content_block_stop","index":0}),
        ];
        let mut chunks = Vec::new();
        for event in events {
            chunks.extend(adapter.map(event)?);
        }
        let block = chunks
            .iter()
            .flat_map(|c| c.delta.assistant_blocks.as_deref().unwrap_or_default())
            .find(|b| b.reasoning.is_some())
            .ok_or_else(|| LlmError::InvalidResponse("signature block missing".into()))?;
        assert!(
            matches!(&block.reasoning,Some(ReasoningBlock::Signed {thinking,signature}) if thinking=="中文😀" && signature=="sig-A-B")
        );
        assert_eq!(block.block_id, "msg_sig:content:0");
        assert!(block.complete);
        Ok(())
    }
}

#[cfg(test)]
#[path = "anthropic_compatibility_tests.rs"]
mod compatibility_tests;

#[cfg(test)]
#[path = "anthropic_resume_tests.rs"]
mod resume_tests;

#[cfg(test)]
#[path = "anthropic_metadata_tests.rs"]
mod metadata_tests;

#[cfg(test)]
#[path = "anthropic_tool_result_default_tests.rs"]
mod tool_result_default_tests;
