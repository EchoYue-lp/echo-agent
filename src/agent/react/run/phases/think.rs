//! Per-iteration LLM call: fire `on_think_start`, run intervention
//! callbacks、原生消息块和token统计；新执行不再重复发布旧Think/Token投影。

use super::super::processor::process_stream_chunk;
use super::super::stream_macros::yield_event_or;
use super::{ThinkOutcome, ThinkOutput};
use crate::agent::AgentEvent;
use crate::agent::snapshot::AgentRunSnapshot;
use crate::error::Result;
use crate::llm::types::{ContentPart, Message, ReasoningBlock, Role, ToolDefinition};
use echo_core::tokenizer::{CalibratedTokenizer, Tokenizer};
use futures::StreamExt;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, mpsc};

/// LLM-call phase: stream chunks, derive content / tool calls / token counts.
///
/// Returns:
/// - [`ThinkOutcome::Continue`] with the assembled [`ThinkOutput`].
/// - [`ThinkOutcome::Abandoned`] when the channel was closed mid-stream.
/// - [`ThinkOutcome::TerminalSettled`] when this phase already persisted and
///   published the single terminal outcome.
pub(crate) async fn run_think(
    snap: &AgentRunSnapshot,
    context: &Arc<Mutex<crate::compression::ContextManager>>,
    tx: &mpsc::Sender<Result<AgentEvent>>,
    messages: Vec<Message>,
    final_only: bool,
    iteration: usize,
) -> Result<ThinkOutcome> {
    let projection_context = crate::compression::ProjectionContext {
        iteration,
        agent_name: snap.config.agent_name.clone(),
        session_id: snap.config.session_id.clone(),
        conversation_id: snap.config.conversation_id.clone(),
        run_id: snap.current_run_id.clone(),
        turn_id: snap.current_turn_id.clone(),
    };
    let agent = &snap.config.agent_name;
    for cb in snap.config.callbacks.iter() {
        cb.on_think_start(agent, &messages).await;
    }

    // ── Intervention callbacks for think (streaming path) ──
    for intervention in &snap.tools.intervention_callbacks {
        let result = intervention.on_think_start(agent, &messages).await;
        if result.cancel {
            super::finalize::settle_terminal_projection(
                snap,
                context,
                Some("Agent execution cancelled by intervention at think".to_string()),
                tx,
            )
            .await?;
            snap.finalize_run(
                crate::trace::RunStatus::Cancelled,
                None,
                Some("Agent execution cancelled by intervention at think"),
            )
            .await;
            let _ = tx.send(Ok(AgentEvent::Cancelled)).await;
            return Ok(ThinkOutcome::TerminalSettled {
                outcome: crate::agent::AgentSteerTurnOutcome::Cancelled,
            });
        }
        if result.block {
            let reason = result
                .block_reason
                .unwrap_or_else(|| "blocked by intervention at think".into());
            super::finalize::settle_terminal_projection(
                snap,
                context,
                Some(format!("Think blocked by intervention: {reason}")),
                tx,
            )
            .await?;
            snap.finalize_run(
                crate::trace::RunStatus::Failed,
                None,
                Some(&format!("Think blocked by intervention: {reason}")),
            )
            .await;
            let _ = tx
                .send(Ok(AgentEvent::error_message(
                    "intervention",
                    format!("Think blocked by intervention: {reason}"),
                )))
                .await;
            return Ok(ThinkOutcome::TerminalSettled {
                outcome: crate::agent::AgentSteerTurnOutcome::Failed,
            });
        }
        if let Some(injected) = result.injected_context {
            super::super::context::push_runtime_context_note(
                context,
                "Intervention:ThinkStart",
                &injected,
            )
            .await;
        }
    }

    let request = build_llm_request(snap, messages, final_only);
    if let Some(projector) = &snap.pre_model_context_projector
        && let Err(error) = projector
            .validate_model_request(
                &projection_context,
                &request,
                snap.config.tool_output_artifacts.as_ref(),
            )
            .await
    {
        if projector.is_required() {
            let reason = error.to_string();
            let terminal = super::required_context_failure_event(snap, error)?;
            super::finalize::settle_terminal_projection(snap, context, Some(reason.clone()), tx)
                .await?;
            snap.finalize_run(crate::trace::RunStatus::Failed, None, Some(&reason))
                .await;
            let _ = tx.send(Ok(terminal)).await;
            return Ok(ThinkOutcome::TerminalSettled {
                outcome: crate::agent::AgentSteerTurnOutcome::Failed,
            });
        }
        tracing::warn!(error = %error, "Optional model context qualification failed");
    }
    let request_estimate = estimate_request_prompt(&request, snap.calibrated_tokenizer.as_ref())
        .and_then(|estimate| {
            validate_request_budget(snap, &estimate)?;
            Ok(estimate)
        });
    let request_estimate = match request_estimate {
        Ok(estimate) => estimate,
        Err(error) => {
            let terminal = AgentEvent::from_error("react_loop", &error)?;
            super::finalize::settle_terminal_projection(snap, context, Some(error.to_string()), tx)
                .await?;
            snap.finalize_run(
                crate::trace::RunStatus::Failed,
                None,
                Some(&error.to_string()),
            )
            .await;
            let _ = tx.send(Ok(terminal)).await;
            return Ok(ThinkOutcome::TerminalSettled {
                outcome: crate::agent::AgentSteerTurnOutcome::Failed,
            });
        }
    };
    let estimated_context_tokens = request_estimate.message_tokens;
    let context_breakdown = crate::trace::LlmContextBreakdown::estimate(
        &request.messages,
        snap.calibrated_tokenizer.as_ref(),
    );
    let cache_fingerprint = cache_fingerprint(&request.messages, request.tools.as_deref());
    let message_count = request.messages.len();
    let (protected_message_count, protected_context_tokens) = {
        let context = context.lock().await;
        (
            context.protected_message_count(),
            context.protected_token_estimate(),
        )
    };
    let llm_started = Instant::now();
    // One request identity is shared by every native assistant block emitted
    // below. Consumers can therefore reconstruct provider order without
    // guessing from text or from the legacy ThinkStart/ThinkEnd projection.
    let request_id = uuid::Uuid::new_v4().to_string();
    let mut block_sequence = 0_u64;
    yield_event_or!(
        tx,
        super::finalize::execution_status_event(
            snap,
            echo_core::agent::ExecutionStatusKind::Waiting,
            echo_core::agent::AgentPhase::Thinking,
            Some(request_id.clone()),
            u64::try_from(iteration)
                .unwrap_or(u64::MAX)
                .saturating_mul(2)
                .saturating_add(1),
            None
        ),
        ThinkOutcome::Abandoned
    );
    let mut llm_stream = match create_llm_stream(snap, request).await {
        Ok(stream) => Box::pin(stream),
        Err(error) => {
            let cancelled_at_failure = snap
                .cancel_token
                .as_ref()
                .is_some_and(crate::agent::CancellationToken::is_cancelled);
            super::retain_provider_response_observation(
                context,
                super::incomplete_response_observation(&error),
            )
            .await;
            let terminal = AgentEvent::from_error("react_loop", &error)?;
            if cancelled_at_failure {
                let reason = "Agent execution cancelled before model response";
                super::finalize::settle_terminal_projection(
                    snap,
                    context,
                    Some(reason.to_string()),
                    tx,
                )
                .await?;
                snap.finalize_run(crate::trace::RunStatus::Cancelled, None, Some(reason))
                    .await;
                let _ = tx.send(Ok(AgentEvent::Cancelled)).await;
                return Ok(ThinkOutcome::TerminalSettled {
                    outcome: crate::agent::AgentSteerTurnOutcome::Cancelled,
                });
            }
            super::finalize::settle_terminal_projection(snap, context, Some(error.to_string()), tx)
                .await?;
            snap.finalize_run(
                crate::trace::RunStatus::Failed,
                None,
                Some(&error.to_string()),
            )
            .await;
            let _ = tx.send(Ok(terminal)).await;
            return Ok(ThinkOutcome::TerminalSettled {
                outcome: crate::agent::AgentSteerTurnOutcome::Failed,
            });
        }
    };
    let mut content_buffer = String::new();
    let mut reasoning_buffer = String::new();
    let mut reasoning_blocks = Vec::new();
    let mut assistant_blocks = Vec::new();
    let mut tool_call_map: super::super::processor::ToolCallFragments = HashMap::new();
    let mut last_usage = None;
    let mut provider_response = None;
    let mut finish_reason = None::<String>;
    let mut legacy_reasoning_open = false;

    loop {
        let next = tokio::select! {
            biased;
            _ = async {
                match snap.cancel_token.as_ref() {
                    Some(token) => token.cancelled().await,
                    None => std::future::pending().await,
                }
            } => {
                super::retain_provider_response_observation(context, provider_response.as_ref()).await;
                super::finalize::settle_terminal_projection(
                    snap,
                    context,
                    Some("Agent execution cancelled during model response".to_string()),
                    tx,
                ).await?;
                snap.finalize_run(
                    crate::trace::RunStatus::Cancelled,
                    None,
                    Some("Agent execution cancelled during model response"),
                ).await;
                let _ = tx.send(Ok(AgentEvent::Cancelled)).await;
                return Ok(ThinkOutcome::TerminalSettled {
                    outcome: crate::agent::AgentSteerTurnOutcome::Cancelled,
                });
            }
            next = llm_stream.next() => next,
        };
        let Some(cr) = next else {
            break;
        };
        let observation_error = match &cr {
            Ok(chunk) => chunk.choices.iter().find_map(|choice| {
                choice
                    .delta
                    .provider_response
                    .as_ref()
                    .and_then(|observation| {
                        observe_provider_response(&mut provider_response, observation).err()
                    })
            }),
            Err(_) => None,
        };
        let cr = observation_error.map_or(cr, Err);
        let chunk = match cr {
            Ok(chunk) => chunk,
            Err(error) => {
                let cancelled_at_failure = snap
                    .cancel_token
                    .as_ref()
                    .is_some_and(crate::agent::CancellationToken::is_cancelled);
                if let Some(observation) = super::incomplete_response_observation(&error)
                    && let Err(rejected) =
                        observe_provider_response(&mut provider_response, observation)
                {
                    // 错误本身仍是首因；foreign尾部观察拒绝覆盖本请求，诊断不带原件。
                    tracing::warn!(error = %rejected, "Rejected incomplete response metadata; original failure retained");
                }
                super::retain_provider_response_observation(context, provider_response.as_ref())
                    .await;
                let terminal = AgentEvent::from_error("react_loop", &error)?;
                if cancelled_at_failure {
                    super::finalize::settle_terminal_projection(
                        snap,
                        context,
                        Some("Agent execution cancelled during model response".to_string()),
                        tx,
                    )
                    .await?;
                    snap.finalize_run(
                        crate::trace::RunStatus::Cancelled,
                        None,
                        Some("Agent execution cancelled during model response"),
                    )
                    .await;
                    let _ = tx.send(Ok(AgentEvent::Cancelled)).await;
                    return Ok(ThinkOutcome::TerminalSettled {
                        outcome: crate::agent::AgentSteerTurnOutcome::Cancelled,
                    });
                }
                super::finalize::settle_terminal_projection(
                    snap,
                    context,
                    Some(error.to_string()),
                    tx,
                )
                .await?;
                snap.finalize_run(
                    crate::trace::RunStatus::Failed,
                    None,
                    Some(&error.to_string()),
                )
                .await;
                if !matches!(&error, crate::error::ReactError::Llm(error)
                    if matches!(error.as_ref(), crate::error::LlmError::IncompleteResponse { .. }))
                {
                    emit_partial_content_before_failure(
                        snap,
                        tx,
                        &content_buffer,
                        &assistant_blocks,
                        &request_id,
                        block_sequence,
                    )
                    .await;
                }
                let _ = tx.send(Ok(terminal)).await;
                return Ok(ThinkOutcome::TerminalSettled {
                    outcome: crate::agent::AgentSteerTurnOutcome::Failed,
                });
            }
        };
        for reason in chunk
            .choices
            .iter()
            .filter_map(|choice| choice.finish_reason.as_ref())
        {
            finish_reason = Some(reason.clone());
        }
        if chunk.usage.is_some() {
            last_usage = chunk.usage.clone();
        }
        for choice in &chunk.choices {
            let native_blocks = choice.delta.assistant_blocks.as_deref().unwrap_or_default();
            for block in native_blocks {
                // 原生块身份/阶段/顺序属于provider事实；正文在完整响应分类及校验前缓冲。
                assistant_blocks.push(block.clone());
                block_sequence = block_sequence.max(block.sequence.saturating_add(1));
                if let Some(reasoning) = &block.reasoning
                    && !reasoning_blocks.contains(reasoning)
                {
                    reasoning_blocks.push(reasoning.clone());
                }
            }
            if native_blocks.is_empty() {
                if let Some(reasoning) = choice
                    .delta
                    .reasoning_content
                    .as_deref()
                    .filter(|text| !text.is_empty())
                {
                    legacy_reasoning_open = true;
                    let block = echo_core::agent::AssistantMessageBlock {
                        block_id: format!("{request_id}:reasoning"),
                        request_id: request_id.clone(),
                        sequence: block_sequence,
                        phase: echo_core::agent::AssistantPhase::Reasoning,
                        delta: reasoning.to_string(),
                        complete: false,
                        provider: snap.config.provider.clone(),
                        provider_source: None,
                        reasoning: None,
                    };
                    assistant_blocks.push(block.clone());
                    block_sequence = block_sequence.saturating_add(1);
                }
                for reasoning in choice.delta.reasoning_blocks.as_deref().unwrap_or_default() {
                    let block = echo_core::agent::AssistantMessageBlock {
                        block_id: format!("{request_id}:reasoning:{block_sequence}"),
                        request_id: request_id.clone(),
                        sequence: block_sequence,
                        phase: echo_core::agent::AssistantPhase::Reasoning,
                        delta: String::new(),
                        complete: true,
                        provider: snap.config.provider.clone(),
                        provider_source: None,
                        reasoning: Some(reasoning.clone()),
                    };
                    assistant_blocks.push(block.clone());
                    block_sequence = block_sequence.saturating_add(1);
                }
            }
            for reasoning in choice.delta.reasoning_blocks.as_deref().unwrap_or_default() {
                if !reasoning_blocks.contains(reasoning) {
                    reasoning_blocks.push(reasoning.clone());
                }
            }
        }
        // 复用既有工具碎片组装；旧Think/Token是兼容观察，不能再次投影同一正文/推理。
        process_stream_chunk(
            &chunk,
            &mut content_buffer,
            &mut reasoning_buffer,
            &mut tool_call_map,
        );
    }

    match finish_reason.as_deref() {
        Some("stop" | "tool_calls" | "function_call" | "continue") => {}
        Some(reason) => {
            let error = incomplete_model_response(
                &content_buffer,
                &reasoning_buffer,
                &reasoning_blocks,
                &assistant_blocks,
                &tool_call_map,
                provider_response.as_ref(),
                Some(reason),
            );
            super::retain_provider_response_observation(context, provider_response.as_ref()).await;
            super::finalize::settle_terminal_projection(snap, context, Some(error.to_string()), tx)
                .await?;
            snap.finalize_run(
                crate::trace::RunStatus::Failed,
                None,
                Some(&error.to_string()),
            )
            .await;
            let _ = tx.send(AgentEvent::from_error("react_loop", &error)).await;
            return Ok(ThinkOutcome::TerminalSettled {
                outcome: crate::agent::AgentSteerTurnOutcome::Failed,
            });
        }
        None => {
            let error = incomplete_model_response(
                &content_buffer,
                &reasoning_buffer,
                &reasoning_blocks,
                &assistant_blocks,
                &tool_call_map,
                provider_response.as_ref(),
                None,
            );
            super::retain_provider_response_observation(context, provider_response.as_ref()).await;
            super::finalize::settle_terminal_projection(snap, context, Some(error.to_string()), tx)
                .await?;
            snap.finalize_run(
                crate::trace::RunStatus::Failed,
                None,
                Some(&error.to_string()),
            )
            .await;
            let _ = tx.send(AgentEvent::from_error("react_loop", &error)).await;
            return Ok(ThinkOutcome::TerminalSettled {
                outcome: crate::agent::AgentSteerTurnOutcome::Failed,
            });
        }
    }

    if legacy_reasoning_open {
        // 旧reasoning_content协议以正常response完成补最后空增量；它只结束reasoning块。
        assistant_blocks.push(echo_core::agent::AssistantMessageBlock {
            block_id: format!("{request_id}:reasoning"),
            request_id: request_id.clone(),
            sequence: block_sequence,
            phase: echo_core::agent::AssistantPhase::Reasoning,
            delta: String::new(),
            complete: true,
            provider: snap.config.provider.clone(),
            provider_source: None,
            reasoning: None,
        });
        block_sequence = block_sequence.saturating_add(1);
    }
    // 没有原生正文身份的旧协议只补Unknown，不能因同响应有工具而改成commentary。
    let has_native_text = assistant_blocks
        .iter()
        .any(|block| block.phase != echo_core::agent::AssistantPhase::Reasoning);
    if !has_native_text && !content_buffer.is_empty() {
        assistant_blocks.push(echo_core::agent::AssistantMessageBlock {
            block_id: format!("{request_id}:assistant"),
            request_id: request_id.clone(),
            sequence: block_sequence,
            phase: echo_core::agent::AssistantPhase::Unknown,
            delta: content_buffer.clone(),
            complete: true,
            provider: snap.config.provider.clone(),
            provider_source: None,
            reasoning: None,
        });
    }
    content_buffer = assistant_blocks
        .iter()
        .filter(|block| {
            super::effective_assistant_phase(&assistant_blocks, block)
                != echo_core::agent::AssistantPhase::Reasoning
        })
        .map(|block| block.delta.as_str())
        .collect();
    let continue_response = finish_reason.as_deref() == Some("continue");
    let candidate_answer = if tool_call_map.is_empty() && finish_reason.as_deref() == Some("stop") {
        let phase = if assistant_blocks.iter().any(|block| {
            super::effective_assistant_phase(&assistant_blocks, block)
                == echo_core::agent::AssistantPhase::FinalAnswer
        }) {
            echo_core::agent::AssistantPhase::FinalAnswer
        } else {
            echo_core::agent::AssistantPhase::Unknown
        };
        let selected: Vec<_> = assistant_blocks
            .iter()
            .filter(|block| super::effective_assistant_phase(&assistant_blocks, block) == phase)
            .collect();
        // 一块的complete只结束该块；最终候选还要求整个协议正常完成且同身份最后增量已结束。
        let complete = !selected.is_empty()
            && selected.iter().all(|block| {
                assistant_blocks
                    .iter()
                    .rev()
                    .find(|last| {
                        last.block_id == block.block_id && last.request_id == block.request_id
                    })
                    .is_some_and(|last| last.complete)
            });
        let answer: String = selected.iter().map(|block| block.delta.as_str()).collect();
        (complete && !answer.is_empty()).then_some(answer)
    } else {
        None
    };
    // 工具优先/纯进度会继续原执行；候选正文留到Guard/Critic/Stop/持久化之后统一发布。
    let progress_only = super::assistant_progress_only(&assistant_blocks);
    let mut approved_assistant_blocks = Vec::new();
    if !tool_call_map.is_empty() || progress_only || candidate_answer.is_none() {
        let approved_text = if content_buffer.is_empty() {
            true
        } else {
            snap.check_final_answer_guard(&content_buffer)
                .await
                .is_ok_and(|approved| approved == content_buffer)
        };
        for block in &assistant_blocks {
            let phase = super::effective_assistant_phase(&assistant_blocks, block);
            let reasoning = phase == echo_core::agent::AssistantPhase::Reasoning;
            let progress = matches!(
                phase,
                echo_core::agent::AssistantPhase::Commentary
                    | echo_core::agent::AssistantPhase::PartialAnswer
            );
            let structured_output = snap
                .config
                .response_format
                .as_ref()
                .is_some_and(crate::llm::ResponseFormat::is_json);
            // 最终Critic不阻断已过Guard的非最终正文；工具/协议继续只决定公开许可，
            // 不改原phase。最终phase（含晚到metadata）不能借工具分支提前公开。
            let ordinary_nonfinal = phase == echo_core::agent::AssistantPhase::Unknown
                && (!tool_call_map.is_empty() || continue_response)
                && !structured_output;
            if reasoning || (approved_text && (progress || ordinary_nonfinal)) {
                approved_assistant_blocks.push(block.clone());
                yield_event_or!(
                    tx,
                    AgentEvent::AssistantMessage {
                        block: block.clone()
                    },
                    {
                        super::retain_provider_response_observation(
                            context,
                            provider_response.as_ref(),
                        )
                        .await;
                        ThinkOutcome::Abandoned
                    }
                );
            }
        }
    }

    let pt = last_usage
        .as_ref()
        .map(|usage| usage.effective_prompt_tokens())
        .unwrap_or(0) as usize;
    let ct = last_usage
        .as_ref()
        .and_then(|u| u.completion_tokens)
        .unwrap_or(0) as usize;
    let total_tokens = last_usage
        .as_ref()
        .map(|usage| usage.effective_total_tokens() as usize)
        .unwrap_or_else(|| pt.saturating_add(ct));
    let cached_prompt_tokens = last_usage
        .as_ref()
        .map(|u| u.cached_prompt_tokens() as usize)
        .unwrap_or(0);
    let cache_creation_prompt_tokens = last_usage
        .as_ref()
        .map(|u| u.cache_creation_prompt_tokens() as usize)
        .unwrap_or(0);
    let usage_reported = last_usage.is_some();

    snap.record_event(crate::trace::RunEvent::LlmCall {
        messages: message_count,
        prompt_tokens: u32::try_from(pt).unwrap_or(u32::MAX),
        completion_tokens: u32::try_from(ct).unwrap_or(u32::MAX),
        cached_prompt_tokens: u32::try_from(cached_prompt_tokens).unwrap_or(u32::MAX),
        cache_creation_prompt_tokens: u32::try_from(cache_creation_prompt_tokens)
            .unwrap_or(u32::MAX),
        usage_reported,
        estimated_context_tokens,
        protected_context_tokens,
        protected_message_count,
        context_limit_tokens: snap.config.token_limit,
        context_breakdown,
        cache_fingerprint,
        duration_ms: u64::try_from(llm_started.elapsed().as_millis()).unwrap_or(u64::MAX),
    })
    .await;

    // Only comparable provider prompt usage may update the factor. Provider
    // file fallback and reasoning replay have no portable token estimate.
    if request_estimate.calibratable
        && let Some(actual) = last_usage
            .as_ref()
            .filter(|usage| usage.prompt_tokens.is_some())
            .map(|usage| usage.effective_prompt_tokens())
            .filter(|actual| *actual > 0)
    {
        snap.calibrated_tokenizer
            .calibrate(request_estimate.raw_tokens, actual);
    }

    // Record usage in the token tracker for cumulative tracking
    if let Some(ref u) = last_usage {
        snap.token_tracker.record_usage(u);
    }
    #[cfg(feature = "telemetry")]
    {
        let provider = snap.config.provider.as_deref().unwrap_or("unknown");
        let status = if content_buffer.is_empty() && tool_call_map.is_empty() {
            "empty"
        } else {
            "success"
        };
        crate::telemetry::Metrics::record_llm_call(provider, &snap.config.model_name, status);
        crate::telemetry::Metrics::record_llm_latency(
            provider,
            &snap.config.model_name,
            llm_started.elapsed().as_secs_f64() * 1000.0,
        );
        crate::telemetry::Metrics::record_llm_tokens(
            provider,
            &snap.config.model_name,
            "input",
            u64::try_from(pt).unwrap_or(u64::MAX),
        );
        crate::telemetry::Metrics::record_llm_tokens(
            provider,
            &snap.config.model_name,
            "output",
            u64::try_from(ct).unwrap_or(u64::MAX),
        );
    }
    tracing::debug!(
        target: "echo_agent::llm_usage",
        agent = %snap.config.agent_name,
        model = %snap.config.model_name,
        prompt_tokens = pt,
        completion_tokens = ct,
        total_tokens = total_tokens,
        cached_prompt_tokens = cached_prompt_tokens,
        cache_creation_prompt_tokens = cache_creation_prompt_tokens,
        usage_reported = usage_reported,
        "LLM usage recorded"
    );

    yield_event_or!(
        tx,
        AgentEvent::LlmUsage {
            model: snap.config.model_name.clone(),
            prompt_tokens: pt,
            completion_tokens: ct,
            total_tokens,
            cached_prompt_tokens,
            cache_creation_prompt_tokens,
            usage_reported,
        },
        {
            super::retain_provider_response_observation(context, provider_response.as_ref()).await;
            ThinkOutcome::Abandoned
        }
    );

    Ok(ThinkOutcome::Continue(Box::new(ThinkOutput {
        provider_response,
        content_buffer,
        candidate_answer,
        continue_response,
        reasoning_buffer,
        reasoning_blocks,
        assistant_blocks,
        approved_assistant_blocks,
        tool_call_map,
        pt,
        ct,
        usage_reported,
    })))
}

fn observe_provider_response(
    current: &mut Option<crate::llm::types::ProviderResponseMetadata>,
    observation: &crate::llm::types::ProviderResponseMetadata,
) -> Result<()> {
    if current.as_ref().is_some_and(|previous| {
        previous.protocol != observation.protocol
            || previous.provider != observation.provider
            || previous
                .request_id
                .as_ref()
                .is_some_and(|request_id| observation.request_id.as_ref() != Some(request_id))
    }) {
        return Err(crate::error::LlmError::InvalidResponse(
            "provider response observation changed its request identity".into(),
        )
        .into());
    }
    // 协议reducer给出完整快照；这里只替换同请求观察，不重建usage或再次累计。
    *current = Some(observation.clone());
    Ok(())
}

fn incomplete_model_response(
    content: &str,
    reasoning: &str,
    reasoning_blocks: &[ReasoningBlock],
    assistant_blocks: &[echo_core::agent::AssistantMessageBlock],
    tool_calls: &super::super::processor::ToolCallFragments,
    observation: Option<&crate::llm::types::ProviderResponseMetadata>,
    finish_reason: Option<&str>,
) -> crate::error::ReactError {
    if content.is_empty()
        && reasoning.is_empty()
        && reasoning_blocks.is_empty()
        && assistant_blocks.is_empty()
        && tool_calls.is_empty()
        && observation.is_none()
    {
        return crate::error::LlmError::InvalidResponse(match finish_reason {
            Some(reason) => format!("model stream ended with non-success finish reason '{reason}'"),
            None => "model stream ended without a finish reason; response may be truncated".into(),
        })
        .into();
    }
    let mut partial = super::with_reasoning_content(
        Message::assistant(content.to_string()),
        reasoning.to_string(),
        reasoning_blocks.to_vec(),
    );
    partial.assistant_blocks = (!assistant_blocks.is_empty()).then(|| assistant_blocks.to_vec());
    partial.provider_response = observation.cloned();
    let mut ordered = tool_calls.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|(index, _)| **index);
    // 原碎片只进入失败载体，不解析/修复参数、不生成调用ID、不得交给工具执行器。
    let calls = ordered
        .into_iter()
        .map(
            |(_, (id, name, arguments, source))| crate::llm::types::ToolCall {
                provider_source: source.clone(),
                id: id.clone(),
                call_type: "function".into(),
                function: crate::llm::types::FunctionCall {
                    name: name.clone(),
                    arguments: arguments.clone(),
                },
            },
        )
        .collect::<Vec<_>>();
    partial.tool_calls = (!calls.is_empty()).then_some(calls);
    crate::error::LlmError::IncompleteResponse {
        partial: Box::new(partial),
    }
    .into()
}

async fn emit_partial_content_before_failure(
    snap: &AgentRunSnapshot,
    tx: &mpsc::Sender<Result<AgentEvent>>,
    content: &str,
    blocks: &[echo_core::agent::AssistantMessageBlock],
    request_id: &str,
    sequence: u64,
) {
    if snap.config.verifier_enabled
        || snap
            .config
            .response_format
            .as_ref()
            .is_some_and(crate::llm::ResponseFormat::is_json)
    {
        return;
    }
    if !content.is_empty()
        && let Ok(approved) = snap.check_final_answer_guard(content).await
    {
        if approved == content
            && blocks
                .iter()
                .any(|block| block.phase != echo_core::agent::AssistantPhase::Reasoning)
        {
            for block in blocks
                .iter()
                .filter(|block| block.phase != echo_core::agent::AssistantPhase::Reasoning)
            {
                if tx
                    .send(Ok(AgentEvent::AssistantMessage {
                        block: block.clone(),
                    }))
                    .await
                    .is_err()
                {
                    return;
                }
            }
        } else {
            let _ = tx
                .send(Ok(AgentEvent::AssistantMessage {
                    block: echo_core::agent::AssistantMessageBlock {
                        block_id: format!("{request_id}:incomplete"),
                        request_id: request_id.to_string(),
                        sequence,
                        phase: echo_core::agent::AssistantPhase::Unknown,
                        delta: approved,
                        complete: false,
                        provider: snap.config.provider.clone(),
                        provider_source: None,
                        reasoning: None,
                    },
                }))
                .await;
        }
    }
}

fn build_llm_request(
    snap: &AgentRunSnapshot,
    messages: Vec<Message>,
    final_only: bool,
) -> crate::llm::ChatRequest {
    // 排除整条失败观察；普通消息保D1到适配器核对native来源后再从HTTP wire剥离。
    // 不改原Context，也不提前抹掉来源一致性证据；工具及签名reasoning仍走原协议回传。
    let messages: Vec<_> = messages
        .into_iter()
        .filter(|message| !message.is_provider_response_observation_only())
        .collect();
    let tools = tools_for_request(snap, final_only);
    let layout = echo_core::llm::cache::PromptCacheLayout::from_messages(
        &messages,
        tools.as_deref().unwrap_or(&[]),
    );
    let prefix_hash = echo_core::llm::cache::diagnostic::stable_prefix_hash(
        layout.system,
        layout.canonical,
        layout.tools,
        layout.history,
    );
    let segments = layout.segment_ranges();
    let response_format = match snap.config.response_format.as_ref() {
        Some(crate::llm::ResponseFormat::Text) | None => None,
        format => format.cloned(),
    };
    crate::llm::ChatRequest {
        messages,
        temperature: snap.config.temperature,
        max_tokens: snap.config.max_tokens,
        tools,
        tool_choice: (final_only && snap.config.supports_tool_choice_none)
            .then_some(crate::llm::ToolChoice::None),
        response_format,
        thinking: snap.thinking.clone(),
        cancel_token: snap.cancel_token.clone(),
        timeouts: None,
        user_id: snap.config.cache_user_id.clone(),
        cache_hints: Some(echo_core::llm::cache::CacheHints {
            breakpoints: vec![],
            stable_prefix_hash: Some(prefix_hash),
            segments,
        }),
    }
}

/// Create a streaming LLM call wrapped in retry / circuit-breaker policy.
pub(crate) async fn create_llm_stream(
    snap: &AgentRunSnapshot,
    request: crate::llm::ChatRequest,
) -> Result<
    std::pin::Pin<
        Box<dyn futures::Stream<Item = Result<crate::llm::types::ChatCompletionChunk>> + Send>,
    >,
> {
    if request
        .response_format
        .as_ref()
        .is_some_and(|format| matches!(format, crate::llm::ResponseFormat::JsonSchema { .. }))
        && !snap.config.supports_structured_output
    {
        return Err(crate::error::ConfigError::UnMatchConfigError(
            snap.config.model_name.clone(),
            "configured response_format requires a fresh structured-output model capability"
                .to_string(),
        )
        .into());
    }
    log_prompt_cache_shape(&request.messages, request.tools.as_deref());

    // ── Trait path: when an LlmClient trait object is attached (production
    // OpenAiClient / test MockLlmClient), route through it. This avoids the
    // per-call model resolution, which keeps the core loop testable with a
    // mock and avoids coupling execution to a global model registry.
    // tracing::info!(
    //     agent = %snap.config.agent_name,
    //     model = %snap.config.model_name,
    //     has_llm_client = snap.llm_client.is_some(),
    //     "think: LLM call path selection"
    // );
    if let Some(llm_client) = snap.llm_client.clone() {
        type ChunkStream = std::pin::Pin<
            Box<dyn futures::Stream<Item = Result<crate::llm::types::ChatCompletionChunk>> + Send>,
        >;
        let stream: ChunkStream = super::super::retry::retry_llm_call(
            &snap.config.agent_name,
            snap.config.llm_max_retries,
            snap.config.llm_retry_delay_ms,
            &snap.guard.circuit_breaker,
            snap.cancel_token.as_ref(),
            || {
                let llm_client = llm_client.clone();
                let request = request.clone();
                async move {
                    // 公开Agent返回形式不决定HTTP传输。默认保持原流式调用；显式
                    // 非流式仅把同provider完整响应适配成一个输入块，后续校验/工具/结算共用。
                    let inner: futures::stream::BoxStream<'static, Result<crate::llm::ChatChunk>> =
                        if snap.config.model_streaming {
                            llm_client.chat_stream(request).await?
                        } else {
                            let response = llm_client.chat(request).await?;
                            let message = response.message;
                            let tool_calls = message
                                .tool_calls
                                .map(|calls| {
                                    calls
                                        .into_iter()
                                        .enumerate()
                                        .map(|(index, call)| {
                                            let index = u32::try_from(index).map_err(|_| {
                                                crate::error::ReactError::Other(
                                                    "provider returned too many tool calls".into(),
                                                )
                                            })?;
                                            Ok(crate::llm::types::DeltaToolCall {
                                                provider_source: call.provider_source,
                                                index,
                                                id: Some(call.id),
                                                call_type: Some(call.call_type),
                                                function: Some(
                                                    crate::llm::types::DeltaFunctionCall {
                                                        name: Some(call.function.name),
                                                        arguments: Some(call.function.arguments),
                                                    },
                                                ),
                                            })
                                        })
                                        .collect::<Result<Vec<_>>>()
                                })
                                .transpose()?;
                            let chunk = crate::llm::ChatChunk {
                                delta: crate::llm::types::DeltaMessage {
                                    provider_response: message.provider_response,
                                    role: Some("assistant".into()),
                                    content: message.content.as_text(),
                                    reasoning_content: message.reasoning_content,
                                    reasoning_blocks: message.reasoning_blocks,
                                    assistant_blocks: message.assistant_blocks,
                                    tool_calls,
                                },
                                finish_reason: response.finish_reason,
                                usage: response.usage,
                            };
                            Box::pin(futures::stream::iter([Ok(chunk)]))
                        };
                    // Adapt the trait's flattened ChatChunk back into the
                    // ChatCompletionChunk shape consumed by the think phase.
                    // Both originate from the same stream, so no information is lost.
                    let mapped = inner.map(|chunk_result| {
                        chunk_result.map(|c| crate::llm::types::ChatCompletionChunk {
                            id: String::new(),
                            choices: vec![crate::llm::types::ChunkChoice {
                                delta: c.delta,
                                finish_reason: c.finish_reason,
                                index: 0,
                            }],
                            usage: c.usage,
                        })
                    });
                    Ok(Box::pin(mapped) as ChunkStream)
                }
            },
        )
        .await?;
        return Ok(stream);
    }

    Err(crate::error::ReactError::Other(format!(
        "agent '{}' has no LlmClient; inject an explicit LlmConfig or LlmClient",
        snap.config.agent_name
    )))
}

struct PromptTokenEstimate {
    raw_tokens: usize,
    message_tokens: usize,
    tool_tokens: usize,
    format_tokens: usize,
    calibratable: bool,
}

struct RequestFactorTokenizer<'a> {
    base: &'a dyn Tokenizer,
    factor: f64,
}

impl Tokenizer for RequestFactorTokenizer<'_> {
    fn count_tokens(&self, text: &str) -> usize {
        (self.base.count_tokens(text) as f64 * self.factor).round() as usize
    }
}

fn serialized_base_tokens<T: serde::Serialize + ?Sized>(
    value: &T,
    base: &dyn Tokenizer,
) -> Result<usize> {
    let encoded = serde_json::to_string(value)
        .map_err(|error| crate::error::ReactError::Other(error.to_string()))?;
    Ok(base.count_tokens(&encoded))
}

fn estimate_request_prompt(
    request: &crate::llm::ChatRequest,
    tokenizer: &CalibratedTokenizer,
) -> Result<PromptTokenEstimate> {
    let base = tokenizer.base_tokenizer();
    let factor = tokenizer.calibration_factor();
    let adjusted = RequestFactorTokenizer { base, factor };
    let adjust = |tokens: usize| (tokens as f64 * factor).round() as usize;
    let mut raw_messages = 0usize;
    let mut message_tokens = 0usize;
    let mut calibratable = true;
    for message in &request.messages {
        if message.content.parts().is_some_and(|parts| {
            parts.iter().any(|part| {
                matches!(
                    part,
                    ContentPart::ImageUrl { .. } | ContentPart::File { .. }
                )
            })
        }) {
            // Image prices are provider-specific and file bytes may be
            // replaced with extracted text or a name-only fallback.
            calibratable = false;
        }
        let mut tokens = message
            .content
            .estimated_tokens(base)
            .saturating_add(base.count_tokens(message.role.as_str()));
        let mut adjusted_tokens = message
            .content
            .estimated_tokens(&adjusted)
            .saturating_add(adjusted.count_tokens(message.role.as_str()));
        if let Some(tool_calls) = &message.tool_calls {
            let tool_call_tokens = serialized_base_tokens(tool_calls, base)?;
            tokens = tokens.saturating_add(tool_call_tokens);
            adjusted_tokens = adjusted_tokens.saturating_add(adjust(tool_call_tokens));
        }
        for text in [message.name.as_deref(), message.tool_call_id.as_deref()]
            .into_iter()
            .flatten()
        {
            tokens = tokens.saturating_add(base.count_tokens(text));
            adjusted_tokens = adjusted_tokens.saturating_add(adjusted.count_tokens(text));
        }
        if message.reasoning_content.is_some() {
            calibratable = false;
        }
        if let Some(blocks) = &message.reasoning_blocks {
            // Some providers replay signed text while others drop the blocks
            // and send reasoning_content instead. Do not learn across them.
            calibratable = false;
            for block in blocks {
                match block {
                    ReasoningBlock::Signed { thinking, .. } => {
                        tokens = tokens.saturating_add(base.count_tokens(thinking));
                        adjusted_tokens =
                            adjusted_tokens.saturating_add(adjusted.count_tokens(thinking));
                    }
                    ReasoningBlock::Redacted { .. } | ReasoningBlock::Opaque { .. } => {}
                }
            }
        } else if let Some(reasoning_content) = &message.reasoning_content {
            tokens = tokens.saturating_add(base.count_tokens(reasoning_content));
            adjusted_tokens =
                adjusted_tokens.saturating_add(adjusted.count_tokens(reasoning_content));
        }
        raw_messages = raw_messages.saturating_add(tokens);
        message_tokens = message_tokens.saturating_add(adjusted_tokens);
    }
    let raw_tools = request
        .tools
        .as_ref()
        .map(|tools| serialized_base_tokens(tools, base))
        .transpose()?
        .unwrap_or(0);
    let raw_format = request
        .response_format
        .as_ref()
        .map(|format| serialized_base_tokens(format, base))
        .transpose()?
        .unwrap_or(0);
    let raw_tokens = raw_messages
        .saturating_add(raw_tools)
        .saturating_add(raw_format);

    Ok(PromptTokenEstimate {
        raw_tokens,
        message_tokens,
        tool_tokens: adjust(raw_tools),
        format_tokens: adjust(raw_format),
        calibratable,
    })
}

/// Reserve the current tool and format schema before ContextManager prepares
/// messages. The later immutable ChatRequest remains the final budget authority
/// if intervention or tool visibility changes after this estimate.
pub(super) fn estimate_pre_compaction_overhead(snap: &AgentRunSnapshot) -> Result<usize> {
    let request = build_llm_request(snap, Vec::new(), false);
    let estimate = estimate_request_prompt(&request, snap.calibrated_tokenizer.as_ref())?;
    Ok(estimate.tool_tokens.saturating_add(estimate.format_tokens))
}

fn tools_for_request(snap: &AgentRunSnapshot, final_only: bool) -> Option<Vec<ToolDefinition>> {
    if !snap.config.enable_tool || final_only {
        return None;
    }
    let tools = snap.tools.tools_for_llm();
    if tools.is_empty() {
        return None;
    }
    if let Ok(stats) = echo_execution::tools::ToolManager::schema_stats_for(&tools) {
        snap.tools.tool_manager.record_schema_stats(&stats);
        tracing::info!(
            target: "echo_agent::tool_budget",
            tool_count = stats.tool_count,
            schema_bytes = stats.schema_bytes,
            schema_estimated_tokens = stats.estimated_tokens,
            "model tool schema budget"
        );
    }
    Some(tools)
}

fn validate_request_budget(snap: &AgentRunSnapshot, estimate: &PromptTokenEstimate) -> Result<()> {
    if let Some(error) = &snap.config.token_budget_error {
        return Err(crate::error::ReactError::Other(error.clone()));
    }
    let window = snap.config.token_limit;
    if window == usize::MAX {
        return Ok(());
    }
    let message_tokens = estimate.message_tokens;
    let tool_tokens = estimate.tool_tokens;
    let format_tokens = estimate.format_tokens;
    let default_output = window / 10;
    let output_tokens = snap
        .config
        .max_tokens
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(default_output);
    let safety_tokens = window / 20;
    let required = message_tokens
        .saturating_add(tool_tokens)
        .saturating_add(format_tokens)
        .saturating_add(output_tokens)
        .saturating_add(safety_tokens);
    if required > window {
        return Err(crate::error::AgentError::ContextLimitExceeded(format!(
            "request requires approximately {required} tokens (messages {message_tokens}, tools {tool_tokens}, format {format_tokens}, output {output_tokens}, safety {safety_tokens}) but the model window is {window}"
        ))
        .into());
    }
    Ok(())
}

pub(crate) fn cache_fingerprint(
    messages: &[Message],
    tools: Option<&[ToolDefinition]>,
) -> echo_core::llm::cache::PromptCacheFingerprint {
    let layout =
        echo_core::llm::cache::PromptCacheLayout::from_messages(messages, tools.unwrap_or(&[]));
    echo_core::llm::cache::prompt_cache_fingerprint(
        layout.system,
        layout.canonical,
        layout.tools,
        layout.history,
    )
}

fn log_prompt_cache_shape(messages: &[Message], tools: Option<&[ToolDefinition]>) {
    // 完整模型可见 prompt 指纹只在调试级别计算：它要序列化全部 parts/tool calls/
    // reasoning，生产默认不付这份开销；记录的内容只有摘要、数量与稳定段标记。
    if tracing::enabled!(target: "echo_agent::prompt_cache", tracing::Level::DEBUG) {
        let tools = tools.unwrap_or(&[]);
        let full = echo_core::llm::cache::full_prompt_fingerprint(messages, tools);
        let layout = echo_core::llm::cache::PromptCacheLayout::from_messages(messages, tools);
        tracing::debug!(
            target: "echo_agent::prompt_cache",
            prompt_hash = %full.prompt_hash,
            tools_schema_hash = %full.tools_schema_hash,
            system_hash = %full.system_hash,
            history_hash = %full.history_hash,
            dynamic_hash = %full.dynamic_hash,
            message_count = full.message_count,
            tool_count = full.tool_count,
            history_messages = layout.history.len(),
            dynamic_messages = layout.dynamic.len(),
            "LLM full prompt cache fingerprint"
        );
    }
    let fingerprint = cache_fingerprint(messages, tools);
    let leading_system_messages = messages
        .iter()
        .take_while(|message| matches!(message.role, Role::System))
        .count();
    let cwd_system_messages = messages
        .iter()
        .filter(|message| {
            matches!(message.role, Role::System)
                && message
                    .text_content()
                    .is_some_and(|text| text.contains("Current working directory:"))
        })
        .count();
    let memory_system_messages = messages
        .iter()
        .filter(|message| {
            matches!(message.role, Role::System)
                && message
                    .text_content()
                    .is_some_and(|text| text.contains("[memory_context]"))
        })
        .count();
    tracing::debug!(
        target: "echo_agent::prompt_cache",
        prefix_hash = %fingerprint.stable_prefix_hash,
        system_prefix_hash = %fingerprint.system_prefix_hash,
        tools_schema_hash = %fingerprint.tools_schema_hash,
        message_count = messages.len(),
        leading_system_messages,
        cwd_system_messages,
        memory_system_messages,
        tool_count = fingerprint.tool_count,
        "LLM prompt cache shape"
    );
    if cwd_system_messages > 1 {
        tracing::warn!(
            target: "echo_agent::prompt_cache",
            cwd_system_messages,
            "Multiple cwd system messages found; prompt-cache prefix is likely unstable"
        );
    }
    if memory_system_messages > 0 {
        tracing::warn!(
            target: "echo_agent::prompt_cache",
            memory_system_messages,
            "Dynamic memory context is present as a system message; prompt-cache prefix is likely unstable"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::react::builder::ReactAgentBuilder;
    use crate::llm::types::{ImageUrl, ResponseFormat, Usage};
    use crate::testing::{MockLlmClient, MockTool};
    use echo_core::tokenizer::HeuristicTokenizer;

    struct BlockPartialOutput;

    impl crate::guard::Guard for BlockPartialOutput {
        fn name(&self) -> &str {
            "block_partial_output"
        }

        fn check<'a>(
            &'a self,
            _content: &'a str,
            direction: crate::guard::GuardDirection,
        ) -> futures::future::BoxFuture<'a, crate::error::Result<crate::guard::GuardResult>>
        {
            Box::pin(async move {
                if direction == crate::guard::GuardDirection::Output {
                    Ok(crate::guard::GuardResult::Block {
                        reason: "partial content rejected".to_string(),
                    })
                } else {
                    Ok(crate::guard::GuardResult::Pass)
                }
            })
        }
    }

    #[tokio::test]
    async fn failed_provider_partial_output_cannot_bypass_output_guard() {
        let mut agent = crate::agent::ReactAgent::new(crate::agent::AgentConfig::new(
            "test-model",
            "guarded",
            "sys",
        ));
        agent.set_guard_manager(echo_core::guard::GuardManager::from_guards(vec![
            std::sync::Arc::new(BlockPartialOutput),
        ]));
        let snapshot = AgentRunSnapshot::from_agent(&agent);
        let (tx, mut rx) = mpsc::channel::<Result<AgentEvent>>(4);
        emit_partial_content_before_failure(
            &snapshot,
            &tx,
            "unfiltered partial",
            &[],
            "failed-request",
            0,
        )
        .await;
        assert!(rx.try_recv().is_err());
    }

    struct FinalRequestQualificationProbe {
        required: bool,
        observed: std::sync::Mutex<Vec<usize>>,
    }

    impl crate::compression::PreModelContextProjector for FinalRequestQualificationProbe {
        fn is_required(&self) -> bool {
            self.required
        }
        fn project<'a>(
            &'a self,
            _: &'a crate::compression::ProjectionContext,
        ) -> futures::future::BoxFuture<'a, Result<Vec<crate::compression::ContextProjection>>>
        {
            Box::pin(async { Ok(Vec::new()) })
        }
        fn validate_model_request<'a>(
            &'a self,
            _: &'a crate::compression::ProjectionContext,
            request: &'a crate::llm::ChatRequest,
            _: Option<&'a crate::tools::artifact::ToolOutputArtifactConfig>,
        ) -> futures::future::BoxFuture<'a, Result<()>> {
            Box::pin(async move {
                self.observed
                    .lock()
                    .map_err(|_| {
                        crate::error::ReactError::Other("observation lock failed".to_owned())
                    })?
                    .push(request.tools.as_ref().map_or(0, Vec::len));
                Err(crate::error::ReactError::Other(
                    "fixture requires a reader in final request".to_owned(),
                ))
            })
        }
    }

    #[tokio::test]
    async fn final_request_qualification_observes_final_only_and_optional_policy() -> Result<()> {
        for required in [false, true] {
            let llm = Arc::new(crate::testing::MockLlmClient::new().with_response("normal answer"));
            let agent = crate::agent::ReactAgentBuilder::new()
                .enable_tools()
                .llm_client(llm.clone())
                .build()?;
            let audit = Arc::new(FinalRequestQualificationProbe {
                required,
                observed: std::sync::Mutex::new(Vec::new()),
            });
            agent.set_pre_model_context_projector(Some(audit.clone()));
            let snapshot = AgentRunSnapshot::from_agent(&agent);
            let (tx, mut rx) = mpsc::channel(16);
            let result = run_think(
                &snapshot,
                agent.context(),
                &tx,
                vec![Message::user("bounded input".to_owned())],
                true,
                3,
            )
            .await?;
            assert_eq!(
                audit
                    .observed
                    .lock()
                    .map_err(|_| crate::error::ReactError::Other(
                        "observation lock failed".to_owned()
                    ))?
                    .as_slice(),
                &[0]
            );
            if required {
                assert!(matches!(
                    result,
                    ThinkOutcome::TerminalSettled {
                        outcome: crate::agent::AgentSteerTurnOutcome::Failed
                    }
                ));
                assert_eq!(llm.call_count(), 0);
                let event = rx.recv().await.ok_or_else(|| {
                    crate::error::ReactError::Other("required failure missing".to_owned())
                })??;
                assert!(
                    matches!(event, AgentEvent::Error { failure, .. } if failure.code == "required_context_unavailable")
                );
            } else {
                assert!(matches!(result, ThinkOutcome::Continue(_)));
                assert_eq!(llm.call_count(), 1);
            }
        }
        Ok(())
    }

    #[test]
    fn prompt_cache_fingerprint_isolates_stable_system_from_history() {
        let first = vec![
            Message::system("stable system".to_string()),
            Message::user("first request".to_string()),
        ];
        let second = vec![
            Message::system("stable system".to_string()),
            Message::user("second request".to_string()),
        ];

        let first_shape = cache_fingerprint(&first, None);
        let second_shape = cache_fingerprint(&second, None);

        assert_eq!(
            first_shape.system_prefix_hash,
            second_shape.system_prefix_hash
        );
        assert_ne!(
            first_shape.stable_prefix_hash,
            second_shape.stable_prefix_hash
        );
    }

    #[test]
    fn duplicate_system_messages_change_system_component_hash() {
        let duplicate = vec![
            Message::system("Current working directory: /tmp/a".to_string()),
            Message::system("Current working directory: /tmp/a".to_string()),
            Message::user("hello".to_string()),
        ];
        let single = vec![
            Message::system("Current working directory: /tmp/a".to_string()),
            Message::user("hello".to_string()),
        ];

        let duplicate = cache_fingerprint(&duplicate, None);
        let single = cache_fingerprint(&single, None);

        assert_ne!(duplicate.system_prefix_hash, single.system_prefix_hash);
    }

    #[tokio::test]
    async fn production_feedback_converges_from_entire_request_base_estimate() -> Result<()> {
        let mut agent = ReactAgentBuilder::new()
            .llm_client(Arc::new(MockLlmClient::new()))
            .tool(Box::new(MockTool::new("calibration_probe")))
            .build()?;
        let messages = vec![Message::user(
            "measure this text prompt across repeated requests".repeat(20),
        )];
        let tools = AgentRunSnapshot::from_agent(&agent).tools.tools_for_llm();
        assert!(!tools.is_empty());
        let base = HeuristicTokenizer;
        let raw_message_tokens = messages.iter().fold(0usize, |total, message| {
            total
                .saturating_add(message.content.estimated_tokens(&base))
                .saturating_add(base.count_tokens(message.role.as_str()))
        });
        let tool_schema = serde_json::to_string(&tools)
            .map_err(|error| crate::error::ReactError::Other(error.to_string()))?;
        let raw_prompt_tokens = raw_message_tokens.saturating_add(base.count_tokens(&tool_schema));
        let actual_prompt_tokens =
            u32::try_from(raw_prompt_tokens.saturating_mul(2)).unwrap_or(u32::MAX);
        let usage = Usage {
            prompt_tokens: Some(actual_prompt_tokens.saturating_sub(20)),
            cache_read_input_tokens: Some(20),
            ..Usage::default()
        };
        let llm = Arc::new((0..12).fold(MockLlmClient::new(), |llm, _| {
            llm.with_response_usage("done", usage.clone())
        }));
        agent.set_llm_client(llm.clone());
        let snapshot = AgentRunSnapshot::from_agent(&agent);

        for _ in 0..12 {
            let (tx, _rx) = mpsc::channel(16);
            let outcome =
                run_think(&snapshot, agent.context(), &tx, messages.clone(), false, 0).await?;
            assert!(matches!(outcome, ThinkOutcome::Continue(_)));
        }

        assert_eq!(snapshot.calibrated_tokenizer.sample_count(), 12);
        assert_eq!(llm.all_tool_counts(), vec![tools.len(); 12]);
        let factor = snapshot.calibrated_tokenizer.calibration_factor();
        assert!(
            (factor - 2.0).abs() < 0.08,
            "factor {factor} did not converge"
        );
        Ok(())
    }

    #[tokio::test]
    async fn missing_prompt_usage_does_not_change_calibration() -> Result<()> {
        let llm = MockLlmClient::new()
            .with_response("usage absent")
            .with_response_usage(
                "completion only",
                Usage {
                    completion_tokens: Some(10),
                    total_tokens: Some(10),
                    ..Usage::default()
                },
            )
            .with_response_usage(
                "cache only",
                Usage {
                    cache_read_input_tokens: Some(40),
                    ..Usage::default()
                },
            );
        let agent = ReactAgentBuilder::new().llm_client(Arc::new(llm)).build()?;
        let snapshot = AgentRunSnapshot::from_agent(&agent);
        let messages = vec![Message::user(
            "prompt without reported input usage".to_string(),
        )];

        for _ in 0..3 {
            let (tx, _rx) = mpsc::channel(16);
            let outcome =
                run_think(&snapshot, agent.context(), &tx, messages.clone(), false, 0).await?;
            assert!(matches!(outcome, ThinkOutcome::Continue(_)));
        }

        assert_eq!(snapshot.calibrated_tokenizer.sample_count(), 0);
        assert_eq!(snapshot.calibrated_tokenizer.calibration_factor(), 1.0);
        Ok(())
    }

    #[test]
    fn response_format_schema_is_counted_in_request_budget() -> Result<()> {
        let agent = ReactAgentBuilder::new()
            .llm_client(Arc::new(MockLlmClient::new()))
            .build()?;
        let mut snapshot = AgentRunSnapshot::from_agent(&agent);
        Arc::make_mut(&mut snapshot.config).token_limit = 200;
        let mut request = crate::llm::ChatRequest::new(vec![Message::user("short".to_string())]);
        let without_format = estimate_request_prompt(&request, &snapshot.calibrated_tokenizer)?;
        assert!(validate_request_budget(&snapshot, &without_format).is_ok());

        request.response_format = Some(ResponseFormat::json_schema(
            "large",
            serde_json::json!({"type":"string","enum":vec!["option"; 300]}),
        ));
        let with_format = estimate_request_prompt(&request, &snapshot.calibrated_tokenizer)?;
        assert!(with_format.format_tokens > 0);
        assert_eq!(
            with_format.raw_tokens,
            without_format
                .raw_tokens
                .saturating_add(with_format.format_tokens)
        );
        assert!(validate_request_budget(&snapshot, &with_format).is_err());
        Ok(())
    }

    #[tokio::test]
    async fn provider_specific_replay_is_not_used_as_calibration_sample() -> Result<()> {
        let usage = Usage {
            prompt_tokens: Some(200),
            ..Usage::default()
        };
        let llm = MockLlmClient::new()
            .with_response_usage("file", usage.clone())
            .with_response_usage("reasoning blocks", usage.clone())
            .with_response_usage("reasoning text", usage);
        let agent = ReactAgentBuilder::new().llm_client(Arc::new(llm)).build()?;
        let snapshot = AgentRunSnapshot::from_agent(&agent);
        let file = Message::user_multimodal(vec![ContentPart::File {
            name: "report.pdf".to_string(),
            content: "base64-data".to_string(),
        }]);
        let mut reasoning = Message::assistant("answer".to_string());
        reasoning.reasoning_blocks = Some(vec![ReasoningBlock::Signed {
            thinking: "private reasoning".to_string(),
            signature: "signature".to_string(),
        }]);
        let mut reasoning_text = Message::assistant("answer".to_string());
        reasoning_text.reasoning_content = Some("private reasoning".to_string());

        for message in [file, reasoning, reasoning_text] {
            let (tx, _rx) = mpsc::channel(16);
            let outcome =
                run_think(&snapshot, agent.context(), &tx, vec![message], false, 0).await?;
            assert!(matches!(outcome, ThinkOutcome::Continue(_)));
        }

        assert_eq!(snapshot.calibrated_tokenizer.sample_count(), 0);
        assert_eq!(snapshot.calibrated_tokenizer.calibration_factor(), 1.0);
        Ok(())
    }

    #[tokio::test]
    async fn image_usage_does_not_distort_text_feedback_across_requests() -> Result<()> {
        let text = "plain text whose provider token ratio is two ".repeat(20);
        let base = HeuristicTokenizer;
        let raw_text = base
            .count_tokens(&text)
            .saturating_add(base.count_tokens("user"));
        let text_usage = Usage {
            prompt_tokens: Some(u32::try_from(raw_text.saturating_mul(2)).unwrap_or(u32::MAX)),
            ..Usage::default()
        };
        let image_usage = Usage {
            // A provider may charge a fixed vision cost independent of text
            // tokenization; this is not twice the local 1,100-token estimate.
            prompt_tokens: Some(
                u32::try_from(raw_text.saturating_mul(2).saturating_add(300)).unwrap_or(u32::MAX),
            ),
            ..Usage::default()
        };
        let llm = MockLlmClient::new()
            .with_response_usage("text first", text_usage.clone())
            .with_response_usage("image", image_usage)
            .with_response_usage("text again", text_usage);
        let agent = ReactAgentBuilder::new().llm_client(Arc::new(llm)).build()?;
        let snapshot = AgentRunSnapshot::from_agent(&agent);
        let text_message = Message::user(text.clone());
        let image_message = Message::user_multimodal(vec![
            ContentPart::Text { text },
            ContentPart::ImageUrl {
                image_url: ImageUrl {
                    url: "https://example.invalid/image.png".to_string(),
                    detail: Some("high".to_string()),
                },
            },
        ]);

        let (tx, _rx) = mpsc::channel(16);
        assert!(matches!(
            run_think(
                &snapshot,
                agent.context(),
                &tx,
                vec![text_message.clone()],
                false,
                0,
            )
            .await?,
            ThinkOutcome::Continue(_)
        ));
        let first_factor = snapshot.calibrated_tokenizer.calibration_factor();
        assert_eq!(snapshot.calibrated_tokenizer.sample_count(), 1);

        let (tx, _rx) = mpsc::channel(16);
        assert!(matches!(
            run_think(
                &snapshot,
                agent.context(),
                &tx,
                vec![image_message],
                false,
                0
            )
            .await?,
            ThinkOutcome::Continue(_)
        ));
        assert_eq!(snapshot.calibrated_tokenizer.sample_count(), 1);
        assert_eq!(
            snapshot.calibrated_tokenizer.calibration_factor(),
            first_factor
        );

        let (tx, _rx) = mpsc::channel(16);
        assert!(matches!(
            run_think(
                &snapshot,
                agent.context(),
                &tx,
                vec![text_message],
                false,
                0
            )
            .await?,
            ThinkOutcome::Continue(_)
        ));
        assert_eq!(snapshot.calibrated_tokenizer.sample_count(), 2);
        assert!(snapshot.calibrated_tokenizer.calibration_factor() > first_factor);
        Ok(())
    }

    #[test]
    fn image_budget_keeps_fixed_cost_after_text_calibration() -> Result<()> {
        let tokenizer = CalibratedTokenizer::new(Arc::new(HeuristicTokenizer));
        for _ in 0..12 {
            tokenizer.calibrate(100, 200);
        }
        let text = "model-visible text";
        let request = crate::llm::ChatRequest::new(vec![Message::user_multimodal(vec![
            ContentPart::Text {
                text: text.to_string(),
            },
            ContentPart::ImageUrl {
                image_url: ImageUrl {
                    url: "https://example.invalid/image.png".to_string(),
                    detail: Some("high".to_string()),
                },
            },
        ])]);
        let estimate = estimate_request_prompt(&request, &tokenizer)?;
        let expected = tokenizer
            .count_tokens(text)
            .saturating_add(tokenizer.count_tokens("user"))
            .saturating_add(1_100);
        assert_eq!(estimate.message_tokens, expected);
        assert!(!estimate.calibratable);
        Ok(())
    }

    #[test]
    fn pre_compaction_tool_reservation_does_not_freeze_final_visibility() -> Result<()> {
        let agent = ReactAgentBuilder::new()
            .llm_client(Arc::new(MockLlmClient::new()))
            .tool(Box::new(MockTool::new("late_visible_tool")))
            .build()?;
        let mut snapshot = AgentRunSnapshot::from_agent(&agent);
        let tool_name = "late_visible_tool".to_string();
        let visibility = Arc::new(echo_core::tools::ToolVisibilityState::new(
            std::collections::HashSet::from([tool_name.clone()]),
            std::collections::HashSet::new(),
        ));
        Arc::make_mut(&mut snapshot.tools).visibility = Some(visibility.clone());

        let reserved = estimate_pre_compaction_overhead(&snapshot)?;
        assert!(
            build_llm_request(&snapshot, Vec::new(), false)
                .tools
                .is_none()
        );
        visibility.activate([tool_name]);
        let final_request = build_llm_request(&snapshot, Vec::new(), false);
        let final_estimate =
            estimate_request_prompt(&final_request, snapshot.calibrated_tokenizer.as_ref())?;
        assert!(final_estimate.tool_tokens > reserved);
        assert_eq!(final_request.tools.as_ref().map(Vec::len), Some(1));
        Ok(())
    }
}
