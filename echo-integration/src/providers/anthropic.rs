//! Anthropic Messages API provider
//!
//! Implements [`LlmClient`] for Anthropic's `/v1/messages` endpoint.
//! System messages are sent as a top-level `system` field (not in the messages array).

use echo_core::error::{LlmError, Result};
use echo_core::llm::capabilities::resolve_thinking_profile;
use echo_core::llm::types::{
    ChatCompletionResponse, ContentPart, DeltaFunctionCall, DeltaMessage, DeltaToolCall,
    FunctionCall, Message, MessageContent, ReasoningBlock, Role, ToolCall, Usage,
};
use echo_core::llm::{
    ChatChunk, ChatRequest, ChatResponse, LlmApiProtocol, LlmClient, ModelInputModality,
    ThinkingProtocol,
};
use futures::StreamExt;
use futures::future::BoxFuture;

use super::anthropic_cache::AnthropicCachePlan;
use super::client::{SseDecoder, parse_sse_data};
use super::config::validate_model_input_modalities;
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
    input_modalities: Vec<ModelInputModality>,
    thinking_protocol: ThinkingProtocol,
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
        let thinking_protocol = resolve_thinking_profile(
            "anthropic",
            &model,
            LlmApiProtocol::Anthropic,
            Some(&base_url),
        )
        .protocol;
        Self {
            client: Arc::new(Self::build_http_client()),
            api_key: api_key.into(),
            model,
            base_url,
            input_modalities: ModelInputModality::all_supported(),
            thinking_protocol,
        }
    }

    pub fn with_base_url(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        let base_url = base_url.into();
        let model = model.into();
        let thinking_protocol = resolve_thinking_profile(
            "anthropic",
            &model,
            LlmApiProtocol::Anthropic,
            Some(&base_url),
        )
        .protocol;
        Self {
            client: Arc::new(Self::build_http_client()),
            api_key: api_key.into(),
            model,
            base_url,
            input_modalities: ModelInputModality::all_supported(),
            thinking_protocol,
        }
    }

    pub fn with_input_modalities(mut self, input_modalities: Vec<ModelInputModality>) -> Self {
        self.input_modalities = if input_modalities.is_empty() {
            ModelInputModality::text_only()
        } else {
            input_modalities
        };
        self
    }

    fn build_http_client() -> Client {
        Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .unwrap_or_default()
    }

    fn convert_request(&self, request: &ChatRequest) -> AnthropicRequest {
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
            AnthropicSystem::Blocks(vec![SystemBlock {
                block_type: "text".to_string(),
                text,
                cache_control: if cache_plan.has_system_breakpoint {
                    Some(CacheControl::ephemeral())
                } else {
                    None
                },
            }])
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
            self.thinking_protocol,
            &request.thinking,
            max_tokens,
        );

        AnthropicRequest {
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
        }
    }

    fn convert_response(&self, resp: AnthropicResponse) -> ChatResponse {
        let mut content_parts: Vec<String> = Vec::new();
        let mut reasoning_parts: Vec<String> = Vec::new();
        let mut reasoning_blocks: Vec<ReasoningBlock> = Vec::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();

        for block in &resp.content {
            match block {
                ContentBlock::Text { text, .. } => content_parts.push(text.clone()),
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
                ContentBlock::Thinking {
                    thinking,
                    signature,
                } => {
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

        let finish_reason = match resp.stop_reason.as_deref() {
            Some("end_turn" | "stop_sequence") => Some("stop".to_string()),
            Some("tool_use") => Some("tool_calls".to_string()),
            Some("max_tokens") => Some("length".to_string()),
            other => other.map(String::from),
        };

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
        };

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

        ChatResponse {
            message,
            finish_reason,
            usage: usage.clone(),
            raw: ChatCompletionResponse {
                id: String::new(),
                choices: Vec::new(),
                created: None,
                model: None,
                usage,
                extra: None,
            },
        }
    }
}

impl LlmClient for AnthropicClient {
    fn chat(&self, request: ChatRequest) -> BoxFuture<'_, Result<ChatResponse>> {
        let model = self.model.clone();
        Box::pin(
            async move {
                self.validate_request_features(&request)?;
                let body = self.convert_request(&request);

                let request_future = async {
                    let resp = self.client
                        .post(&self.base_url)
                        .header("x-api-key", &self.api_key)
                        .header("anthropic-version", "2023-06-01")
                        .header("anthropic-beta", "prompt-caching-2024-07-31")
                        .header("content-type", "application/json")
                        .json(&body)
                        .send()
                        .await
                        .map_err(|e| LlmError::NetworkError(e.to_string()))?;
                    let status = resp.status();
                    if !status.is_success() {
                        let text = resp.text().await.unwrap_or_default();
                        return Err(LlmError::ApiError { status: status.as_u16(), message: text });
                    }
                    Ok(resp)
                };
                let resp = tokio::select! {
                    biased;
                    _ = async {
                        match request.cancel_token.as_ref() {
                            Some(token) => token.cancelled().await,
                            None => std::future::pending().await,
                        }
                    } => return Err(LlmError::NetworkError("Anthropic request cancelled".to_string()).into()),
                    response = request_future => response?,
                };

                let anthropic_resp: AnthropicResponse = resp
                    .json()
                    .await
                    .map_err(|e| LlmError::InvalidResponse(format!("Response parse error: {e}")))?;

                Ok(self.convert_response(anthropic_resp))
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
            let mut body = self.convert_request(&request);
            body.stream = Some(true);

            let request_future = async {
                let resp = self.client
                    .post(&self.base_url)
                    .header("x-api-key", &self.api_key)
                    .header("anthropic-version", "2023-06-01")
                    .header("anthropic-beta", "prompt-caching-2024-07-31")
                    .header("content-type", "application/json")
                    .json(&body)
                    .send()
                    .await
                    .map_err(|e| LlmError::NetworkError(e.to_string()))?;
                let status = resp.status();
                if !status.is_success() {
                    let text = resp.text().await.unwrap_or_default();
                    return Err(LlmError::ApiError { status: status.as_u16(), message: text });
                }
                Ok(resp)
            };
            let resp = tokio::select! {
                biased;
                _ = async {
                    match request.cancel_token.as_ref() {
                        Some(token) => token.cancelled().await,
                        None => std::future::pending().await,
                    }
                } => return Err(LlmError::NetworkError("Anthropic stream request cancelled".to_string()).into()),
                response = request_future => response?,
            };

            let byte_stream = resp.bytes_stream();
            // Track in-progress tool calls during streaming (index → accumulated args)
            let mut tool_call_args: std::collections::HashMap<usize, (String, String, String)> =
                std::collections::HashMap::new();
            let mut reasoning_blocks: std::collections::HashMap<usize, ReasoningBlock> =
                std::collections::HashMap::new();

            // Track cumulative usage across streaming events
            let mut stream_input_tokens: u32 = 0;
            let mut stream_output_tokens: u32 = 0;
            let mut stream_cache_creation_input_tokens: Option<u32> = None;
            let mut stream_cache_read_input_tokens: Option<u32> = None;

            let stream = async_stream::stream! {
                let mut byte_stream = std::pin::pin!(byte_stream);
                let mut decoder = SseDecoder::new();
                loop {
                    let chunk_result = tokio::select! {
                        biased;
                        _ = async {
                            match request.cancel_token.as_ref() {
                                Some(token) => token.cancelled().await,
                                None => std::future::pending().await,
                            }
                        } => {
                            yield Err(LlmError::NetworkError("Anthropic stream cancelled".to_string()).into());
                            return;
                        }
                        next = byte_stream.next() => next,
                    };
                    let Some(chunk_result) = chunk_result else {
                        break;
                    };

                    let chunk = match chunk_result {
                        Ok(c) => c,
                        Err(e) => {
                            yield Err(LlmError::NetworkError(e.to_string()).into());
                            return;
                        }
                    };

                    if let Err(error) = decoder.push(&chunk) {
                        yield Err(error);
                        return;
                    }

                    while let Some(event) = decoder.next_event() {
                        if let Some(data) = parse_sse_data(&event) {
                            if data == "[DONE]" {
                                return;
                            }
                            if let Ok(event) = serde_json::from_str::<AnthropicStreamEvent>(&data) {
                                match event {
                                    AnthropicStreamEvent::MessageStart { message } => {
                                        // Capture initial usage (input_tokens) from message_start
                                        if let Some(u) = message.usage {
                                            stream_input_tokens = u.input_tokens;
                                            stream_cache_creation_input_tokens = u.cache_creation_input_tokens;
                                            stream_cache_read_input_tokens = u.cache_read_input_tokens;
                                        }
                                    }
                                    AnthropicStreamEvent::ContentBlockStart {
                                        index,
                                        content_block:
                                            ContentBlockStartBody::ToolUse { id, name },
                                    } => {
                                        // Start tracking a new tool_use block
                                        tool_call_args.insert(index, (id, name, String::new()));
                                    }
                                    AnthropicStreamEvent::ContentBlockStart {
                                        index,
                                        content_block: ContentBlockStartBody::Thinking { thinking, signature },
                                    } => {
                                        reasoning_blocks.insert(index, ReasoningBlock::Signed { thinking, signature });
                                    }
                                    AnthropicStreamEvent::ContentBlockStart {
                                        index,
                                        content_block: ContentBlockStartBody::RedactedThinking { data },
                                    } => {
                                        reasoning_blocks.insert(index, ReasoningBlock::Redacted { data });
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
                                                    reasoning_blocks: None,
                                                    tool_calls: None,
                                                },
                                                finish_reason: None,
                                                usage: None,
                                            });
                                        } else if let Some(thinking) = delta.thinking {
                                            yield Ok(ChatChunk {
                                                delta: DeltaMessage {
                                                    role: Some("assistant".to_string()),
                                                    content: None,
                                                    reasoning_content: Some(thinking),
                                                    reasoning_blocks: None,
                                                    tool_calls: None,
                                                },
                                                finish_reason: None,
                                                usage: None,
                                            });
                                        } else if let Some(signature) = delta.signature {
                                            if let Some(ReasoningBlock::Signed { signature: value, .. }) = reasoning_blocks.get_mut(&index) {
                                                value.push_str(&signature);
                                            }
                                        } else if let Some(partial) = delta.partial_json {
                                            // Accumulate tool_use arguments
                                            if let Some(entry) = tool_call_args.get_mut(&index) {
                                                entry.2.push_str(&partial);
                                            }
                                        }
                                    }
                                    AnthropicStreamEvent::ContentBlockStop { index } => {
                                        if let Some(block) = reasoning_blocks.remove(&index) {
                                            yield Ok(ChatChunk {
                                                delta: DeltaMessage {
                                                    role: None,
                                                    content: None,
                                                    reasoning_content: None,
                                                    reasoning_blocks: Some(vec![block]),
                                                    tool_calls: None,
                                                },
                                                finish_reason: None,
                                                usage: None,
                                            });
                                        }
                                        // Finalize tool call and emit
                                        if let Some((id, name, args)) =
                                            tool_call_args.remove(&index)
                                        {
                                            let parsed_args = match serde_json::from_str::<serde_json::Value>(&args) {
                                                Ok(value) => value,
                                                Err(error) => {
                                                    yield Err(LlmError::InvalidResponse(format!(
                                                        "invalid Anthropic tool arguments for '{name}': {error}"
                                                    )).into());
                                                    return;
                                                }
                                            };
                                            yield Ok(ChatChunk {
                                                delta: DeltaMessage {
                                                    role: None,
                                                    content: None,
                                                    reasoning_content: None,
                                                    reasoning_blocks: None,
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
                                    AnthropicStreamEvent::MessageDelta { delta, usage } => {
                                        // Capture output_tokens from message_delta
                                        if let Some(u) = usage {
                                            stream_output_tokens = u.output_tokens;
                                        }
                                        let finish = match delta.stop_reason.as_deref() {
                                            Some("end_turn" | "stop_sequence") => Some("stop".to_string()),
                                            Some("tool_use") => Some("tool_calls".to_string()),
                                            other => other.map(String::from),
                                        };
                                        // Emit final chunk with accumulated usage
                                        let usage = if stream_input_tokens > 0 || stream_output_tokens > 0 {
                                            Some(Usage {
                                                prompt_tokens: Some(stream_input_tokens),
                                                completion_tokens: Some(stream_output_tokens),
                                                total_tokens: Some(stream_input_tokens.saturating_add(stream_output_tokens)),
                                                cache_creation_input_tokens: stream_cache_creation_input_tokens,
                                                cache_read_input_tokens: stream_cache_read_input_tokens,
                                                ..Default::default()
                                            })
                                        } else {
                                            None
                                        };
                                        yield Ok(ChatChunk {
                                            delta: DeltaMessage {
                                                role: None,
                                                content: None,
                                                reasoning_content: None,
                                                reasoning_blocks: None,
                                                tool_calls: None,
                                            },
                                            finish_reason: finish,
                                            usage,
                                        });
                                    }
                                    AnthropicStreamEvent::Error { error } => {
                                        yield Err(LlmError::InvalidResponse(format!(
                                            "Anthropic stream error: {}",
                                            error.message
                                        )).into());
                                        return;
                                    }
                                    AnthropicStreamEvent::Other => {}
                                }
                            } else {
                                yield Err(LlmError::InvalidResponse(
                                    "invalid Anthropic SSE event".to_string()
                                ).into());
                                return;
                            }
                        }
                    }
                }
                match decoder.finish() {
                    Ok(None) => {}
                    Ok(Some(event)) => {
                        if parse_sse_data(&event).is_some() {
                            yield Err(LlmError::InvalidResponse(
                                "truncated Anthropic SSE event at EOF".to_string()
                            ).into());
                        }
                    }
                    Err(error) => yield Err(error),
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

/// Anthropic system field: either a plain string or array of content blocks
#[derive(Serialize)]
#[serde(untagged)]
#[allow(dead_code)]
enum AnthropicSystem {
    Text(String),
    Blocks(Vec<SystemBlock>),
}

#[derive(Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<AnthropicSystem>,
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
    content: Vec<ContentBlock>,
    stop_reason: Option<String>,
    #[serde(default)]
    usage: Option<AnthropicUsage>,
}

#[derive(Deserialize, Clone)]
#[allow(dead_code)]
struct AnthropicUsage {
    input_tokens: u32,
    output_tokens: u32,
    #[serde(default)]
    cache_creation_input_tokens: Option<u32>,
    #[serde(default)]
    cache_read_input_tokens: Option<u32>,
}

#[derive(Deserialize, Clone)]
struct AnthropicDeltaUsage {
    output_tokens: u32,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum AnthropicStreamEvent {
    #[serde(rename = "message_start")]
    MessageStart { message: MessageStartBody },
    #[serde(rename = "content_block_start")]
    ContentBlockStart {
        index: usize,
        content_block: ContentBlockStartBody,
    },
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta { index: usize, delta: ContentDelta },
    #[serde(rename = "content_block_stop")]
    ContentBlockStop { index: usize },
    #[serde(rename = "message_delta")]
    MessageDelta {
        delta: MessageDeltaBody,
        #[serde(default)]
        usage: Option<AnthropicDeltaUsage>,
    },
    #[serde(rename = "error")]
    Error { error: AnthropicStreamError },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct AnthropicStreamError {
    message: String,
}

#[derive(Deserialize)]
struct MessageStartBody {
    #[serde(default)]
    usage: Option<AnthropicUsage>,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum ContentBlockStartBody {
    #[serde(rename = "tool_use")]
    ToolUse { id: String, name: String },
    #[serde(rename = "thinking")]
    Thinking {
        #[serde(default)]
        thinking: String,
        #[serde(default)]
        signature: String,
    },
    #[serde(rename = "redacted_thinking")]
    RedactedThinking { data: String },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct ContentDelta {
    text: Option<String>,
    thinking: Option<String>,
    signature: Option<String>,
    #[serde(rename = "partial_json")]
    partial_json: Option<String>,
}

#[derive(Deserialize)]
struct MessageDeltaBody {
    stop_reason: Option<String>,
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
    fn metadata_user_id_present_when_set() -> std::result::Result<(), serde_json::Error> {
        let client = AnthropicClient::new("ds-xxx".to_string(), "deepseek-chat".to_string());
        let body =
            serde_json::to_value(client.convert_request(&chat_request_with_user(Some("user-7"))))?;
        assert_eq!(
            body.pointer("/metadata/user_id")
                .and_then(serde_json::Value::as_str),
            Some("user-7")
        );
        Ok(())
    }

    #[test]
    fn metadata_absent_when_user_id_none() -> std::result::Result<(), serde_json::Error> {
        let client = AnthropicClient::new("ds-xxx".to_string(), "deepseek-chat".to_string());
        let body = serde_json::to_value(client.convert_request(&chat_request_with_user(None)))?;
        // metadata is skip_serializing_if Option::is_none → absent (not null).
        assert!(
            body.get("metadata").is_none(),
            "metadata should be absent when user_id is None, got: {body}"
        );
        Ok(())
    }

    #[test]
    fn multiple_system_messages_are_preserved_in_order()
    -> std::result::Result<(), serde_json::Error> {
        let client = AnthropicClient::new("sk-test", "claude-sonnet-4-6");
        let request = ChatRequest {
            messages: vec![
                Message::system("base rules".to_string()),
                Message::system("restored context".to_string()),
                Message::user("hello".to_string()),
            ],
            ..ChatRequest::default()
        };
        let body = serde_json::to_value(client.convert_request(&request))?;
        assert_eq!(
            body.pointer("/system/0/text")
                .and_then(serde_json::Value::as_str),
            Some("base rules\n\nrestored context")
        );
        Ok(())
    }

    #[test]
    fn message_delta_usage_accepts_output_tokens_only() -> std::result::Result<(), String> {
        let raw = r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":17}}"#;
        let event =
            serde_json::from_str::<AnthropicStreamEvent>(raw).map_err(|error| error.to_string())?;
        let AnthropicStreamEvent::MessageDelta { usage, .. } = event else {
            return Err("expected message_delta event".to_string());
        };
        assert_eq!(usage.map(|value| value.output_tokens), Some(17));
        Ok(())
    }

    #[test]
    fn tool_start_keeps_provider_block_index() -> std::result::Result<(), String> {
        let raw = r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"call-1","name":"read_file","input":{}}}"#;
        let event =
            serde_json::from_str::<AnthropicStreamEvent>(raw).map_err(|error| error.to_string())?;
        let AnthropicStreamEvent::ContentBlockStart { index, .. } = event else {
            return Err("expected content_block_start event".to_string());
        };
        assert_eq!(index, 1);
        Ok(())
    }

    #[test]
    fn thinking_response_is_projected_to_reasoning_content()
    -> std::result::Result<(), serde_json::Error> {
        let raw = r#"{"content":[{"type":"thinking","thinking":"reason","signature":"sig"},{"type":"text","text":"answer"}],"stop_reason":"end_turn","usage":{"input_tokens":2,"output_tokens":3}}"#;
        let response = serde_json::from_str::<AnthropicResponse>(raw)?;
        let converted =
            AnthropicClient::new("sk-test", "claude-sonnet-4-6").convert_response(response);
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
    fn signed_reasoning_blocks_are_replayed_before_tool_calls() {
        let client = AnthropicClient::new("sk-test", "claude-sonnet-4-6");
        let mut assistant = Message::assistant_with_tools(vec![ToolCall {
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
        let request = client.convert_request(&ChatRequest::new(vec![assistant]));

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
    -> std::result::Result<(), serde_json::Error> {
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
        let body = serde_json::to_value(client.convert_request(&req))?;
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
    fn literal_stream_events_preserve_usage_tool_identity_and_partial_json()
    -> std::result::Result<(), serde_json::Error> {
        let start: AnthropicStreamEvent = serde_json::from_str(
            r#"{"type":"message_start","message":{"usage":{"input_tokens":13,"output_tokens":0,"cache_read_input_tokens":4}}}"#,
        )?;
        assert!(matches!(
            start,
            AnthropicStreamEvent::MessageStart {
                message: MessageStartBody {
                    usage: Some(AnthropicUsage {
                        input_tokens: 13,
                        cache_read_input_tokens: Some(4),
                        ..
                    })
                }
            }
        ));

        let tool: AnthropicStreamEvent = serde_json::from_str(
            r#"{"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"toolu-9","name":"read_file","input":{}}}"#,
        )?;
        assert!(matches!(
            tool,
            AnthropicStreamEvent::ContentBlockStart {
                index: 2,
                content_block: ContentBlockStartBody::ToolUse { id, name }
            } if id == "toolu-9" && name == "read_file"
        ));

        let delta: AnthropicStreamEvent = serde_json::from_str(
            r#"{"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"文档.md\"}"}}"#,
        )?;
        assert!(matches!(
            delta,
            AnthropicStreamEvent::ContentBlockDelta {
                index: 2,
                delta: ContentDelta { partial_json: Some(value), .. }
            } if value == "{\"path\":\"文档.md\"}"
        ));

        let terminal: AnthropicStreamEvent = serde_json::from_str(
            r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":7}}"#,
        )?;
        assert!(matches!(
            terminal,
            AnthropicStreamEvent::MessageDelta {
                delta: MessageDeltaBody { stop_reason: Some(reason) },
                usage: Some(AnthropicDeltaUsage { output_tokens: 7 })
            } if reason == "tool_use"
        ));
        Ok(())
    }
}
