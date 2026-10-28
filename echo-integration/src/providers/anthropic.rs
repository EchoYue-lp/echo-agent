//! Anthropic Messages API provider
//!
//! Implements [`LlmClient`] for Anthropic's `/v1/messages` endpoint.
//! System messages are sent as a top-level `system` field (not in the messages array).

use echo_core::agent::{AssistantMessageBlock, AssistantPhase};
use echo_core::error::{LlmError, Result};
use echo_core::llm::capabilities::{
    ModelProfileResolution, ModelProfileResolver, ProviderCapabilityOverride,
};
use echo_core::llm::types::{
    ChatCompletionResponse, ContentPart, DeltaFunctionCall, DeltaMessage, DeltaToolCall,
    FunctionCall, Message, MessageContent, ReasoningBlock, Role, ToolCall, Usage,
};
use echo_core::llm::{
    ChatChunk, ChatRequest, ChatResponse, LlmApiProtocol, LlmClient, LlmTimeouts,
    ModelInputModality, ThinkingProtocol,
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
use tracing::{Instrument, info_span, warn};

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
    fn validate_request_features(&self, request: &ChatRequest) -> Result<()> {
        if request.tool_choice.is_some() {
            return Err(LlmError::InvalidResponse(
                "Anthropic tool_choice translation is not implemented; refusing to silently ignore it"
                    .to_string(),
            )
            .into());
        }
        if request.response_format.is_some() {
            return Err(LlmError::InvalidResponse(
                "Anthropic structured response format translation is not implemented; refusing to silently ignore it"
                    .to_string(),
            )
            .into());
        }
        validate_model_input_modalities(&self.model, &self.input_modalities, &request.messages)?;
        Ok(())
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
        let mut system_parts: Vec<String> = Vec::new();
        let mut messages = Vec::new();

        for msg in &request.messages {
            if msg.role == Role::System {
                if let Some(text) = msg.content.as_text() {
                    system_parts.push(text);
                }
                continue;
            }

            if msg.role == Role::Tool {
                messages.push(AnthropicMessage {
                    role: "user".to_string(),
                    content: AnthropicContent::Blocks(vec![ContentBlock::ToolResult {
                        tool_use_id: msg.tool_call_id.clone().unwrap_or_default(),
                        content: msg.content.as_text().unwrap_or_default(),
                        cache_control: None,
                    }]),
                });
                continue;
            }

            if msg.role == Role::Assistant
                && let Some(blocks) = native_anthropic_history(msg)?
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
                append_reasoning_blocks(&mut blocks, msg.reasoning_blocks.as_deref());
                if let Some(ref text) = msg.content.as_text()
                    && !text.is_empty()
                {
                    blocks.push(ContentBlock::Text {
                        text: text.clone(),
                        cache_control: None,
                    });
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
                    let mut blocks: Vec<ContentBlock> = Vec::new();
                    if msg.role == Role::Assistant {
                        append_reasoning_blocks(&mut blocks, msg.reasoning_blocks.as_deref());
                    }
                    blocks.extend(
                        parts
                            .iter()
                            .map(|part| match part {
                                ContentPart::Text { text } => ContentBlock::Text {
                                    text: text.clone(),
                                    cache_control: None,
                                },
                                ContentPart::ImageUrl { image_url } => ContentBlock::Image {
                                    source: data_url_to_image_source(&image_url.url),
                                    cache_control: None,
                                },
                                ContentPart::ResourceLink { resource } => ContentBlock::Text {
                                    text: resource.model_text(),
                                    cache_control: None,
                                },
                                // File attachments: dispatch by inferred media type.
                                //   - application/pdf → document content block (the only
                                //     type Anthropic accepts as base64 document source)
                                //   - text-class (txt/md/json/xml/...) → decode and inline
                                //     as text so the model can read it directly
                                //   - other binary → name-only placeholder (the API has
                                //     no generic binary attachment block)
                                ContentPart::File { name, content } => {
                                    file_to_content_block(name, content)
                                }
                            })
                            .collect::<Vec<_>>(),
                    );
                    if blocks.is_empty() {
                        AnthropicContent::Text(String::new())
                    } else {
                        AnthropicContent::Blocks(blocks)
                    }
                }
                _ if msg.role == Role::Assistant && msg.reasoning_blocks.is_some() => {
                    let mut blocks = Vec::new();
                    append_reasoning_blocks(&mut blocks, msg.reasoning_blocks.as_deref());
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

        // Translate thinking config. Claude 3.7–4.5 accept the budget block;
        // 4.6+/4.7+ use adaptive thinking and reject the older budget block.
        // The configured model contract is authoritative for the wire dialect.
        let max_tokens = request.max_tokens.unwrap_or(4096);
        let (thinking, effort) = build_anthropic_thinking(
            &self.model,
            self.model_profile_resolution().profile.thinking_protocol,
            &request.thinking,
            max_tokens,
        );

        Ok(AnthropicRequest {
            model: self.model.clone(),
            max_tokens,
            system,
            messages,
            temperature: request.temperature,
            tools,
            stream: None,
            thinking,
            effort,
            metadata: request.user_id.as_ref().map(|uid| AnthropicMetadata {
                user_id: uid.clone(),
            }),
        })
    }

    fn convert_response(&self, resp: AnthropicResponse) -> Result<ChatResponse> {
        let finish_reason = anthropic_finish_reason(resp.stop_reason.as_deref())?;
        if resp.content.is_empty() {
            return Err(LlmError::EmptyResponse.into());
        }
        let request_id = if resp.id.is_empty() {
            super::native_message::fallback_request_id("anthropic")
        } else {
            resp.id.clone()
        };
        let mut assistant_blocks = Vec::new();
        let mut content_parts: Vec<String> = Vec::new();
        let mut reasoning_parts: Vec<String> = Vec::new();
        let mut reasoning_blocks: Vec<ReasoningBlock> = Vec::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();

        for (index, block) in resp.content.iter().enumerate() {
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
                    Some(ReasoningBlock::Signed {
                        thinking: thinking.clone(),
                        signature: signature.clone(),
                    }),
                ),
                ContentBlock::RedactedThinking { data } => (
                    AssistantPhase::Reasoning,
                    String::new(),
                    Some(ReasoningBlock::Redacted { data: data.clone() }),
                ),
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
                    provider_source: Some(output_source(
                        LlmApiProtocol::Anthropic,
                        &request_id,
                        &format!("{request_id}:content:{index}"),
                        output_index,
                        None,
                        content_kind,
                        None,
                    )),
                    block_id: format!("{request_id}:content:{index}"),
                    request_id: request_id.clone(),
                    sequence,
                    phase,
                    delta,
                    complete: true,
                    provider: Some(self.provider_name.clone()),
                    reasoning,
                });
            }
            match block {
                ContentBlock::Text { text, .. } => content_parts.push(text.clone()),
                ContentBlock::ToolUse { id, name, input } => {
                    tool_calls.push(ToolCall {
                        provider_source: Some(output_source(
                            LlmApiProtocol::Anthropic,
                            &request_id,
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
                            arguments: serde_json::to_string(input).unwrap_or_default(),
                        },
                    });
                }
                ContentBlock::Thinking {
                    thinking,
                    signature,
                } => {
                    if signature.is_empty() {
                        return Err(LlmError::InvalidResponse(
                            "Anthropic thinking omitted signature".into(),
                        )
                        .into());
                    }
                    reasoning_parts.push(thinking.clone());
                    reasoning_blocks.push(ReasoningBlock::Signed {
                        thinking: thinking.clone(),
                        signature: signature.clone(),
                    });
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
        };

        super::native_message::validate_completion(
            &message,
            if finish_reason.as_deref() == Some("continue") {
                Some("stop")
            } else {
                finish_reason.as_deref()
            },
        )?;
        // Extract token usage from Anthropic response
        let usage = resp.usage.map(|u| {
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
                model: None,
                usage,
                extra: Some(
                    serde_json::json!({"id":request_id,"stop_reason":resp.stop_reason,"content":resp.content}),
                ),
            },
        })
    }
}

impl LlmClient for AnthropicClient {
    fn chat(&self, request: ChatRequest) -> BoxFuture<'_, Result<ChatResponse>> {
        let model = self.model.clone();
        Box::pin(
            async move {
                self.validate_request_features(&request)?;
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
                let anthropic_resp: AnthropicResponse =
                    post_json_request(request_builder, request.cancel_token.clone()).await?;

                let response = self.convert_response(anthropic_resp)?;
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
                self.validate_request_features(&request)?;
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
        ProviderCapabilityOverride::anthropic_protocol()
    }

    fn model_profile_resolution(&self) -> ModelProfileResolution {
        self.model_profile_resolver.resolve_for_protocol_at(
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
    /// Extended-thinking block.
    ///
    /// - Claude 3.7–4.5 (`AnthropicThinkingBudget`): `{type:"enabled", budget_tokens:N}`.
    /// - Claude 4.6 (`AnthropicEffort`): `{type:"adaptive"}` (the model decides
    ///   depth based on the `effort` field); budget_tokens is rejected here.
    /// - Claude Opus 4.7+ (`AnthropicAdaptive`): the block is dropped entirely
    ///   (sending it returns a 400).
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<AnthropicThinking>,
    /// Effort control (Claude 4.5+, primarily 4.6). One of
    /// `low`/`medium`/`high`/`xhigh`/`max`. Replaces `budget_tokens` as the
    /// recommended depth knob on newer models.
    #[serde(skip_serializing_if = "Option::is_none")]
    effort: Option<String>,
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

/// Resolve the Anthropic thinking fields for a request.
///
/// Returns `(thinking_block, effort)`:
/// - Claude 3.7–4.5 (`AnthropicThinkingBudget`): `{type:"enabled", budget_tokens:N}`
///   + optional `effort` (4.5 accepts effort alongside budget).
/// - Claude 4.6 (`AnthropicEffort`): `{type:"adaptive"}` + `effort`. budget_tokens
///   is rejected on 4.6, so it is NOT emitted.
/// - Claude Opus 4.7+ (`AnthropicAdaptive`): both fields dropped (legacy block
///   returns a 400; the model decides its own depth).
/// - Other protocols / `None`: no fields.
fn build_anthropic_thinking(
    model: &str,
    thinking_protocol: ThinkingProtocol,
    thinking: &Option<echo_core::llm::ThinkingConfig>,
    max_tokens: u32,
) -> (Option<AnthropicThinking>, Option<String>) {
    use echo_core::llm::ThinkingProtocol as T;
    let Some(cfg) = thinking.as_ref() else {
        return (None, None);
    };
    match thinking_protocol {
        T::AnthropicThinkingBudget => {
            // 3.7–4.5: budget_tokens (effort is also accepted on 4.5).
            let block = cfg
                .to_anthropic_budget(max_tokens)
                .map(|budget| AnthropicThinking {
                    block_type: "enabled",
                    budget_tokens: Some(budget),
                });
            (block, None)
        }
        T::AnthropicEffort => {
            // Claude 4.6: adaptive thinking block + effort. budget_tokens 400s.
            let block = if matches!(cfg, echo_core::llm::ThinkingConfig::Disabled) {
                None
            } else {
                Some(AnthropicThinking {
                    block_type: "adaptive",
                    budget_tokens: None,
                })
            };
            (block, cfg.to_anthropic_effort().map(str::to_string))
        }
        T::AnthropicAdaptive => {
            warn!(
                model = model,
                "thinking config ignored: Claude Opus 4.7+ use adaptive-only thinking (any field would 400)"
            );
            (None, None)
        }
        _ => (None, None),
    }
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
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    #[serde(rename = "thinking")]
    Thinking { thinking: String, signature: String },
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
            ContentBlock::ToolResult { content, .. } => is_runtime_context_text(content),
            _ => false,
        }
    }
}

fn append_reasoning_blocks(
    target: &mut Vec<ContentBlock>,
    reasoning_blocks: Option<&[ReasoningBlock]>,
) {
    for block in reasoning_blocks.unwrap_or_default() {
        let content = match block {
            ReasoningBlock::Signed {
                thinking,
                signature,
            } => ContentBlock::Thinking {
                thinking: thinking.clone(),
                signature: signature.clone(),
            },
            ReasoningBlock::Redacted { data } => {
                ContentBlock::RedactedThinking { data: data.clone() }
            }
            // Opaque state is owned by another provider and must not cross the
            // protocol boundary when a conversation switches models.
            ReasoningBlock::Opaque { .. } => continue,
        };
        target.push(content);
    }
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

/// Convert a `ContentPart::File` into the best-fit Anthropic content block.
///
/// Anthropic's Messages API only accepts `application/pdf` as a base64
/// `document` source — other binary types have no generic attachment block.
/// To make text-class attachments (txt/md/json/src/...) actually readable by
/// the model, we decode and inline them as text. Binary non-PDF falls back to
/// a name-only placeholder (the previous behaviour for all files).
fn file_to_content_block(name: &str, content_base64: &str) -> ContentBlock {
    let ext = name.rsplit('.').next().map(|e| e.to_ascii_lowercase());
    match ext.as_deref() {
        // PDF → document content block (the only base64 document type Anthropic
        // accepts).
        Some("pdf") => ContentBlock::Document {
            source: ImageSource::Base64 {
                media_type: "application/pdf".to_string(),
                data: content_base64.to_string(),
            },
            cache_control: None,
        },
        // Text-class files → decode base64 and inline as text so the model can
        // read the contents directly.
        Some(
            "txt" | "md" | "markdown" | "json" | "xml" | "yaml" | "yml" | "csv" | "tsv" | "rs"
            | "py" | "js" | "ts" | "tsx" | "jsx" | "go" | "java" | "c" | "cpp" | "h" | "sh"
            | "toml" | "ini" | "log" | "sql",
        ) => {
            let decoded =
                base64::Engine::decode(&base64::engine::general_purpose::STANDARD, content_base64)
                    .ok();
            let text = decoded
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .unwrap_or_else(|| format!("[Attachment: {name}] (binary, undecodable)"));
            ContentBlock::Text {
                text: format!("\n[Attachment: {name}]\n```\n{text}\n```"),
                cache_control: None,
            }
        }
        // Other binary → name-only placeholder (no generic attachment block).
        _ => ContentBlock::Text {
            text: format!("\n[Attachment: {name}]"),
            cache_control: None,
        },
    }
}

#[derive(Serialize)]
struct AnthropicToolDef {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    input_schema: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<CacheControl>,
}

#[derive(Deserialize)]
struct AnthropicResponse {
    #[serde(default)]
    id: String,
    content: Vec<ContentBlock>,
    stop_reason: Option<String>,
    #[serde(default)]
    usage: Option<AnthropicUsage>,
}

#[derive(Deserialize, Clone)]
struct AnthropicUsage {
    input_tokens: u32,
    output_tokens: u32,
    #[serde(default)]
    cache_creation_input_tokens: Option<u32>,
    #[serde(default)]
    cache_read_input_tokens: Option<u32>,
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
    fn claude_46_max_effort_remains_max() {
        let (thinking, effort) = build_anthropic_thinking(
            "claude-opus-4-6",
            ThinkingProtocol::AnthropicEffort,
            &Some(echo_core::llm::ThinkingConfig::Level(
                echo_core::llm::ThinkingLevel::Max,
            )),
            8_192,
        );
        assert_eq!(effort.as_deref(), Some("max"));
        assert_eq!(thinking.map(|block| block.block_type), Some("adaptive"));
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
        let chunks = adapter.map(serde_json::json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":17}}))?;
        assert!(chunks.is_empty());
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
        let raw = r#"{"content":[{"type":"thinking","thinking":"reason","signature":"sig"},{"type":"text","text":"answer"}],"stop_reason":"end_turn","usage":{"input_tokens":2,"output_tokens":3}}"#;
        let response = serde_json::from_str::<AnthropicResponse>(raw)?;
        let converted =
            AnthropicClient::new("sk-test", "claude-sonnet-4-6").convert_response(response)?;
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
                if thinking == "reason" && signature == "sig"
        ));
        assert!(matches!(
            blocks.and_then(|values| values.get(1)),
            Some(ContentBlock::ToolUse { name, .. }) if name == "read_file"
        ));
        Ok(())
    }

    #[test]
    fn unsupported_request_features_fail_instead_of_being_dropped() {
        let client = AnthropicClient::new("test-key", "test-model");
        let mut request = ChatRequest::new(vec![Message::user("hello".to_string())]);
        request.tool_choice = Some("none".to_string());
        let tool_error = client
            .validate_request_features(&request)
            .err()
            .map(|error| error.to_string())
            .unwrap_or_default();
        assert!(tool_error.contains("tool_choice"));

        request.tool_choice = None;
        request.response_format = Some(echo_core::llm::types::ResponseFormat::JsonObject);
        let format_error = client
            .validate_request_features(&request)
            .err()
            .map(|error| error.to_string())
            .unwrap_or_default();
        assert!(format_error.contains("response format"));
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
        let block = file_to_content_block("report.pdf", &b64);
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
        let block = file_to_content_block("notes.txt", &b64);
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
    fn binary_non_pdf_attachment_is_placeholder() -> std::result::Result<(), String> {
        let b64 = base64::engine::general_purpose::STANDARD.encode(b"\x00\x01binary");
        let block = file_to_content_block("archive.zip", &b64);
        let ContentBlock::Text { text, .. } = block else {
            return Err("expected Text placeholder".to_string());
        };
        assert!(text.contains("archive.zip"));
        // Should NOT contain decoded binary garbage.
        assert!(!text.contains("binary"));
        Ok(())
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
fn native_anthropic_history(message: &Message) -> Result<Option<Vec<ContentBlock>>> {
    let native = message.assistant_blocks.as_deref().unwrap_or_default();
    let tools = message.tool_calls.as_deref().unwrap_or_default();
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
    let mut merged: std::collections::BTreeMap<u32, AssistantMessageBlock> =
        std::collections::BTreeMap::new();
    let mut owner = None::<String>;
    for block in native.iter().filter(|block| {
        block
            .provider_source
            .as_ref()
            .is_some_and(|source| source.protocol == LlmApiProtocol::Anthropic)
    }) {
        let source = block
            .provider_source
            .as_ref()
            .ok_or_else(|| invalid("source omitted"))?;
        if owner
            .as_ref()
            .is_some_and(|owner| owner != &source.request_id)
        {
            return Err(invalid("mixed request owners").into());
        }
        owner.get_or_insert_with(|| source.request_id.clone());
        if let Some(saved) = merged.get_mut(&source.output_index) {
            if saved.provider_source != block.provider_source {
                return Err(invalid("source changed").into());
            }
            saved.delta.push_str(&block.delta);
            if block.reasoning.is_some() {
                saved.reasoning = block.reasoning.clone();
            }
        } else {
            merged.insert(source.output_index, block.clone());
        }
    }
    let mut ordered = std::collections::BTreeMap::new();
    for (index, block) in merged {
        let value = match block.reasoning {
            Some(ReasoningBlock::Signed {
                thinking,
                signature,
            }) => ContentBlock::Thinking {
                thinking,
                signature,
            },
            Some(ReasoningBlock::Redacted { data }) => ContentBlock::RedactedThinking { data },
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
    reasoning: std::collections::HashMap<usize, ReasoningBlock>,
    tools: std::collections::HashMap<usize, (String, String, String)>,
    usage: Usage,
    finish: Option<String>,
    has_output: bool,
    output_text: String,
    completed_tools: Vec<ToolCall>,
}

impl NativeAnthropicStream {
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
            provider_source: Some(output_source(
                LlmApiProtocol::Anthropic,
                &self.request_id,
                &format!("{}:content:{index}", self.request_id),
                output_index,
                None,
                kind,
                None,
            )),
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

    fn map(&mut self, event: serde_json::Value) -> Result<Vec<ChatChunk>> {
        use serde_json::Value;
        let invalid = |text: &str| LlmError::InvalidResponse(format!("Anthropic stream {text}"));
        let kind = event
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("event omitted type"))?;
        if self.finish.is_some() && !matches!(kind, "message_stop" | "ping" | "error") {
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
                self.request_id = message
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                    .map(ToString::to_string)
                    .unwrap_or_else(|| super::native_message::fallback_request_id("anthropic"));
                if let Some(usage) = message.get("usage") {
                    self.usage.prompt_tokens = usage
                        .get("input_tokens")
                        .and_then(Value::as_u64)
                        .and_then(|n| u32::try_from(n).ok());
                    self.usage.cache_read_input_tokens = usage
                        .get("cache_read_input_tokens")
                        .and_then(Value::as_u64)
                        .and_then(|n| u32::try_from(n).ok());
                    self.usage.cache_creation_input_tokens = usage
                        .get("cache_creation_input_tokens")
                        .and_then(Value::as_u64)
                        .and_then(|n| u32::try_from(n).ok());
                }
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
                if self.request_id.is_empty() || self.blocks.insert(index, block.clone()).is_some()
                {
                    return Err(invalid("block start preceded message or duplicated index").into());
                }
                match block.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        let text = block
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        if !text.is_empty() {
                            self.has_output = true;
                            self.output_text.push_str(text);
                            chunks.push(self.canonical(
                                index,
                                AssistantPhase::Unknown,
                                text.to_string(),
                                false,
                                None,
                            )?);
                        }
                    }
                    Some("thinking") => {
                        let text = block
                            .get("thinking")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        self.reasoning.insert(
                            index,
                            ReasoningBlock::Signed {
                                thinking: text.clone(),
                                signature: block
                                    .get("signature")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_string(),
                            },
                        );
                        if !text.is_empty() {
                            chunks.push(self.canonical(
                                index,
                                AssistantPhase::Reasoning,
                                text,
                                false,
                                None,
                            )?);
                        }
                    }
                    Some("redacted_thinking") => {
                        let data = block
                            .get("data")
                            .and_then(Value::as_str)
                            .filter(|v| !v.is_empty())
                            .ok_or_else(|| invalid("redacted thinking omitted data"))?;
                        self.reasoning.insert(
                            index,
                            ReasoningBlock::Redacted {
                                data: data.to_string(),
                            },
                        );
                    }
                    Some("tool_use") => {
                        let id = block
                            .get("id")
                            .and_then(Value::as_str)
                            .filter(|v| !v.is_empty())
                            .ok_or_else(|| invalid("tool omitted id"))?
                            .to_string();
                        let name = block
                            .get("name")
                            .and_then(Value::as_str)
                            .filter(|v| !v.is_empty())
                            .ok_or_else(|| invalid("tool omitted name"))?
                            .to_string();
                        self.tools
                            .insert(index, (id.clone(), name.clone(), String::new()));
                        chunks.push(anthropic_tool_chunk(index, Some(id), Some(name), None)?);
                    }
                    _ => return Err(invalid("unknown content block type").into()),
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
                            &self.request_id,
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
                    if matches!(&reasoning, ReasoningBlock::Signed { signature, .. } if signature.is_empty())
                    {
                        return Err(invalid("thinking block omitted signature").into());
                    }
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
                self.finish = anthropic_finish_reason(
                    event
                        .get("delta")
                        .and_then(|d| d.get("stop_reason"))
                        .and_then(Value::as_str),
                )?;
                if let Some(n) = event
                    .get("usage")
                    .and_then(|u| u.get("output_tokens"))
                    .and_then(Value::as_u64)
                    .and_then(|n| u32::try_from(n).ok())
                {
                    self.usage.completion_tokens = Some(n);
                }
                self.usage.total_tokens = Some(
                    self.usage
                        .prompt_tokens
                        .unwrap_or(0)
                        .saturating_add(self.usage.completion_tokens.unwrap_or(0)),
                );
            }
            "message_stop" => {
                if self.finish.is_none() || !self.blocks.is_empty() || !self.has_output {
                    return Err(invalid("message_stop without completed nonempty response").into());
                }
                let message = Message {
                    content: MessageContent::Text(self.output_text.clone()),
                    tool_calls: (!self.completed_tools.is_empty())
                        .then(|| self.completed_tools.clone()),
                    ..Default::default()
                };
                super::native_message::validate_completion(
                    &message,
                    if self.finish.as_deref() == Some("continue") {
                        Some("stop")
                    } else {
                        self.finish.as_deref()
                    },
                )?;
                chunks.push(ChatChunk {
                    delta: DeltaMessage::default(),
                    finish_reason: self.finish.clone(),
                    usage: Some(self.usage.clone()),
                });
            }
            "error" => return Err(invalid("reported provider error").into()),
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
                tool.provider_source = Some(output_source(
                    LlmApiProtocol::Anthropic,
                    &self.request_id,
                    id,
                    tool.index,
                    None,
                    "tool_use",
                    None,
                ));
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
) -> BoxStream<'static, Result<ChatChunk>>
where
    S: futures::Stream<Item = Result<JsonSseEvent>> + Send + 'static,
{
    Box::pin(async_stream::try_stream! {
        let mut adapter = NativeAnthropicStream { provider, ..Default::default() };
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
