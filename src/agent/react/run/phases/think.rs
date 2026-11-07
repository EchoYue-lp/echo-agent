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
    // One request identity is shared by every native assistant block emitted
    // below. Consumers can therefore reconstruct provider order without
    // guessing from text or from the legacy ThinkStart/ThinkEnd projection.
    let request_id = uuid::Uuid::new_v4().to_string();
    let mut output_timing = ModelOutputTiming::default();
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
    let observed = match create_llm_stream(snap, request, tx, &request_id).await {
        Ok(observed) => observed,
        Err(error) => {
            let cancelled_at_failure = snap
                .cancel_token
                .as_ref()
                .is_some_and(crate::agent::CancellationToken::is_cancelled);
            // 原 call_fn 已逐次记录实际尝试；最终重试错误不再重复记账。
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
    let llm_started = observed.started;
    output_timing.attempt_id = Some(observed.attempt_id);
    let mut llm_stream = observed.stream;
    let mut content_buffer = String::new();
    let mut reasoning_buffer = String::new();
    let mut reasoning_blocks = Vec::new();
    let mut assistant_blocks = Vec::new();
    let mut tool_call_map: super::super::processor::ToolCallFragments = HashMap::new();
    let mut last_usage = None;
    let mut provider_response = None;
    let mut reported_usage = None;
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
                output_timing.finished = Some(elapsed_millis(llm_started));
                observe_failed_model_usage(
                    snap, tx, last_usage.as_ref().or(reported_usage.as_ref()), provider_response.as_ref(), &request_id, (&output_timing, llm_started), crate::agent::ModelCallOutcome::Cancelled,
                ).await;
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
            output_timing.finished = Some(elapsed_millis(llm_started));
            break;
        };
        let observation_error = match &cr {
            Ok(chunk) => chunk.choices.iter().find_map(|choice| {
                choice
                    .delta
                    .provider_response
                    .as_ref()
                    .and_then(|observation| {
                        if let Err(error) =
                            observe_provider_response(&mut provider_response, observation)
                        {
                            return Some(error);
                        }
                        update_reported_usage(snap, observation, &mut reported_usage);
                        None
                    })
            }),
            Err(_) => None,
        };
        let cr = observation_error.map_or(cr, Err);
        let chunk = match cr {
            Ok(chunk) => chunk,
            Err(error) => {
                output_timing.finished = Some(elapsed_millis(llm_started));
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
                if let Some(observation) = provider_response.as_ref() {
                    update_reported_usage(snap, observation, &mut reported_usage);
                }
                observe_failed_model_usage(
                    snap,
                    tx,
                    last_usage.as_ref().or(reported_usage.as_ref()),
                    provider_response.as_ref(),
                    &request_id,
                    (&output_timing, llm_started),
                    if cancelled_at_failure {
                        crate::agent::ModelCallOutcome::Cancelled
                    } else {
                        crate::agent::ModelCallOutcome::Failed
                    },
                )
                .await;
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
        output_timing.observe(&chunk, elapsed_millis(llm_started));
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
            observe_failed_model_usage(
                snap,
                tx,
                last_usage.as_ref().or(reported_usage.as_ref()),
                provider_response.as_ref(),
                &request_id,
                (&output_timing, llm_started),
                crate::agent::ModelCallOutcome::Failed,
            )
            .await;
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
            observe_failed_model_usage(
                snap,
                tx,
                last_usage.as_ref().or(reported_usage.as_ref()),
                provider_response.as_ref(),
                &request_id,
                (&output_timing, llm_started),
                crate::agent::ModelCallOutcome::Failed,
            )
            .await;
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
            observation: Some(output_timing.observation(
                &request_id,
                last_usage.as_ref(),
                llm_started,
                crate::agent::ModelCallOutcome::Completed,
            )),
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

fn elapsed_millis(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[derive(Default)]
struct ModelOutputTiming {
    first: Option<u64>,
    last: Option<u64>,
    samples: u64,
    reasoning_seen: bool,
    incomparable: bool,
    attempt_id: Option<String>,
    finished: Option<u64>,
}

impl ModelOutputTiming {
    fn observe(&mut self, chunk: &crate::llm::types::ChatCompletionChunk, elapsed_ms: u64) {
        let mut output = false;
        if chunk.choices.len() > 1 {
            self.incomparable = true;
        }
        for choice in &chunk.choices {
            let delta = &choice.delta;
            let reasoning = delta
                .reasoning_content
                .as_ref()
                .is_some_and(|text| !text.is_empty());
            self.reasoning_seen |= reasoning;
            output |= reasoning || delta.content.as_ref().is_some_and(|text| !text.is_empty());
            output |= delta
                .tool_calls
                .as_deref()
                .unwrap_or_default()
                .iter()
                .any(|call| {
                    call.function.as_ref().is_some_and(|function| {
                        function.name.as_ref().is_some_and(|text| !text.is_empty())
                            || function
                                .arguments
                                .as_ref()
                                .is_some_and(|text| !text.is_empty())
                    })
                });
            for block in delta.assistant_blocks.as_deref().unwrap_or_default() {
                if self.attempt_id.is_none() && !block.request_id.trim().is_empty() {
                    self.attempt_id = Some(block.request_id.clone());
                }
                if block.complete
                    && let Some(reasoning) = &block.reasoning
                {
                    self.observe_reasoning(reasoning);
                }
                output |= !block.delta.is_empty();
                if block.phase == echo_core::agent::AssistantPhase::Reasoning
                    && !block.delta.is_empty()
                {
                    self.reasoning_seen = true;
                }
            }
            for reasoning in delta.reasoning_blocks.as_deref().unwrap_or_default() {
                self.observe_reasoning(reasoning);
            }
        }
        if output {
            self.first.get_or_insert(elapsed_ms);
            self.last = Some(elapsed_ms);
            self.samples = self.samples.saturating_add(1);
        }
    }

    fn observe_reasoning(&mut self, reasoning: &ReasoningBlock) {
        match reasoning {
            ReasoningBlock::Redacted { .. } | ReasoningBlock::Opaque { .. } => {
                self.incomparable = true
            }
            ReasoningBlock::Signed { thinking, .. }
                if !thinking.is_empty() && !self.reasoning_seen =>
            {
                // 完整签名块可能只在结尾到达，不能把它当作整个推理的生成计时。
                self.incomparable = true;
            }
            _ => {}
        }
    }

    fn observation(
        &self,
        call_id: &str,
        usage: Option<&crate::llm::types::Usage>,
        started: Instant,
        outcome: crate::agent::ModelCallOutcome,
    ) -> crate::agent::ModelCallObservation {
        let hidden_reasoning = usage
            .and_then(|usage| usage.output_tokens_details.as_ref())
            .and_then(|details| details.reasoning_tokens)
            .is_some_and(|tokens| tokens > 0)
            && !self.reasoning_seen;
        crate::agent::ModelCallObservation {
            call_id: call_id.to_string(),
            attempt_id: self
                .attempt_id
                .clone()
                .unwrap_or_else(|| call_id.to_string()),
            outcome,
            request_duration_ms: self.finished.unwrap_or_else(|| elapsed_millis(started)),
            first_output_ms: self.first,
            last_output_ms: self.last,
            output_samples: self.samples,
            timing_comparable: usage.is_some() && !self.incomparable && !hidden_reasoning,
        }
    }
}

fn update_reported_usage(
    snap: &AgentRunSnapshot,
    observation: &crate::llm::types::ProviderResponseMetadata,
    previous: &mut Option<crate::llm::types::Usage>,
) {
    let Some(client) = snap.llm_client.as_ref() else {
        return;
    };
    match client.reported_usage(observation, previous.as_ref()) {
        Ok(Some(usage)) => *previous = Some(usage),
        Ok(None) => {}
        Err(_) => tracing::warn!("Rejected reported usage snapshot; original source retained"),
    }
}

async fn observe_failed_model_usage(
    snap: &AgentRunSnapshot,
    tx: &mpsc::Sender<Result<AgentEvent>>,
    usage: Option<&crate::llm::types::Usage>,
    metadata: Option<&crate::llm::types::ProviderResponseMetadata>,
    call_id: &str,
    clock: (&ModelOutputTiming, Instant),
    outcome: crate::agent::ModelCallOutcome,
) {
    let (timing, started) = clock;
    let reported = usage.cloned().or_else(|| {
        let client = snap.llm_client.as_ref()?;
        match client.reported_usage(metadata?, None) {
            Ok(usage) => usage,
            Err(_) => {
                // 观测失败不能替换原 provider 错误或在日志中回显原件。
                tracing::warn!("Rejected reported usage observation; original outcome retained");
                None
            }
        }
    });
    let usage = reported.as_ref();
    // 失败不抹去已报告用量；只追加原usage观察，原错误及运行结算仍沿原路径。
    if let Some(usage) = usage {
        snap.token_tracker.record_usage(usage);
    }
    let prompt_tokens = usage
        .map(|usage| usage.effective_prompt_tokens() as usize)
        .unwrap_or(0);
    let completion_tokens = usage.and_then(|usage| usage.completion_tokens).unwrap_or(0) as usize;
    let _ = tx
        .send(Ok(AgentEvent::LlmUsage {
            model: snap.config.model_name.clone(),
            prompt_tokens,
            completion_tokens,
            total_tokens: usage
                .map(|usage| usage.effective_total_tokens() as usize)
                .unwrap_or(0),
            cached_prompt_tokens: usage
                .map(|usage| usage.cached_prompt_tokens() as usize)
                .unwrap_or(0),
            cache_creation_prompt_tokens: usage
                .map(|usage| usage.cache_creation_prompt_tokens() as usize)
                .unwrap_or(0),
            usage_reported: usage.is_some(),
            observation: Some(timing.observation(call_id, usage, started, outcome)),
        }))
        .await;
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
pub(crate) struct ObservedModelStream {
    stream: futures::stream::BoxStream<'static, Result<crate::llm::types::ChatCompletionChunk>>,
    attempt_id: String,
    started: Instant,
}

pub(crate) async fn create_llm_stream(
    snap: &AgentRunSnapshot,
    request: crate::llm::ChatRequest,
    tx: &mpsc::Sender<Result<AgentEvent>>,
    call_id: &str,
) -> Result<ObservedModelStream> {
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
        let stream = super::super::retry::retry_llm_call(
            &snap.config.agent_name,
            snap.config.llm_max_retries,
            snap.config.llm_retry_delay_ms,
            &snap.guard.circuit_breaker,
            snap.cancel_token.as_ref(),
            || {
                let llm_client = llm_client.clone();
                let request = request.clone();
                let call_id = call_id.to_string();
                async move {
                    let started = Instant::now();
                    let attempt_id = uuid::Uuid::new_v4().to_string();
                    let mut obtained_usage = None;
                    let result: Result<ChunkStream> = async {
                        // 公开Agent返回形式不决定HTTP传输。默认保持原流式调用；显式
                        // 非流式仅把同provider完整响应适配成一个输入块，后续校验/工具/结算共用。
                        let inner: futures::stream::BoxStream<
                            'static,
                            Result<crate::llm::ChatChunk>,
                        > = if snap.config.model_streaming {
                            llm_client.chat_stream(request).await?
                        } else {
                            let response = llm_client.chat(request).await?;
                            obtained_usage = response.usage.clone();
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
                    .await;
                    match result {
                        Ok(stream) => Ok(ObservedModelStream {
                            stream,
                            attempt_id,
                            started,
                        }),
                        Err(error) => {
                            let timing = ModelOutputTiming {
                                attempt_id: Some(attempt_id),
                                finished: Some(elapsed_millis(started)),
                                ..ModelOutputTiming::default()
                            };
                            let outcome = if snap
                                .cancel_token
                                .as_ref()
                                .is_some_and(crate::agent::CancellationToken::is_cancelled)
                            {
                                crate::agent::ModelCallOutcome::Cancelled
                            } else {
                                crate::agent::ModelCallOutcome::Failed
                            };
                            observe_failed_model_usage(
                                snap,
                                tx,
                                obtained_usage.as_ref(),
                                super::incomplete_response_observation(&error),
                                &call_id,
                                (&timing, started),
                                outcome,
                            )
                            .await;
                            Err(error)
                        }
                    }
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

    async fn reported_usage_http_fixture(
        responses: Vec<(u16, String)>,
        hold_last: bool,
    ) -> Result<(
        String,
        crate::agent::CancellationToken,
        tokio::task::JoinHandle<Result<usize>>,
    )> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let endpoint = format!("http://{}", listener.local_addr()?);
        let release = crate::agent::CancellationToken::new();
        let server_release = release.clone();
        let total = responses.len();
        let server = tokio::spawn(async move {
            tokio::time::timeout(std::time::Duration::from_secs(8), async move {
                let mut count = 0_usize;
                for (status, body) in responses {
                    let (mut socket, _) = listener.accept().await?;
                    let mut bytes = Vec::new();
                    let mut chunk = [0_u8; 4096];
                    loop {
                        let read = socket.read(&mut chunk).await?;
                        if read == 0 || bytes.len() > 2_000_000 {
                            return Err(crate::error::ReactError::Other("usage fixture incomplete HTTP input".into()));
                        }
                        bytes.extend_from_slice(chunk.get(..read).unwrap_or_default());
                        let Some(end) = bytes.windows(4).position(|value| value == b"\r\n\r\n") else { continue; };
                        let headers = String::from_utf8_lossy(bytes.get(..end).unwrap_or_default());
                        let length = headers.lines().filter_map(|line| line.split_once(':'))
                            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                            .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                            .ok_or_else(|| crate::error::ReactError::Other("usage fixture missing content length".into()))?;
                        if bytes.len() >= end.saturating_add(4).saturating_add(length) { break; }
                    }
                    count = count.saturating_add(1);
                    let hold = hold_last && count == total;
                    let content_type = if status == 200 { "text/event-stream" } else { "application/json" };
                    let length = if hold { String::new() } else { format!("Content-Length: {}\r\n", body.len()) };
                    let response = format!("HTTP/1.1 {status} Fixture\r\nContent-Type: {content_type}\r\n{length}Connection: close\r\n\r\n{body}");
                    socket.write_all(response.as_bytes()).await?;
                    socket.flush().await?;
                    if hold { server_release.cancelled().await; }
                }
                Ok(count)
            }).await.map_err(|_| crate::error::ReactError::Other("usage fixture HTTP watchdog".into()))?
        });
        Ok((endpoint, release, server))
    }

    fn reported_usage_agent(
        endpoint: String,
        protocol: crate::llm::LlmApiProtocol,
    ) -> Result<crate::agent::ReactAgent> {
        let path = match protocol {
            crate::llm::LlmApiProtocol::Anthropic => "messages",
            crate::llm::LlmApiProtocol::Responses => "responses",
            crate::llm::LlmApiProtocol::ChatCompletions => "chat/completions",
        };
        let config = crate::llm::LlmConfig::for_provider(
            "usage-fixture",
            format!("{endpoint}/v1/{path}"),
            "fixture-key",
            "fixture-model",
            protocol,
        )?;
        let mut agent = ReactAgentBuilder::new()
            .llm_config(config)
            .auto_project_rules(false)
            .max_iterations(2)
            .build()?;
        agent.config_mut().llm_max_retries = 0;
        agent.config_mut().llm_retry_delay_ms = 0;
        Ok(agent)
    }

    async fn reported_usage_collect(agent: &crate::agent::ReactAgent) -> Result<Vec<AgentEvent>> {
        use crate::agent::Agent;
        let stream = agent.chat_stream("核对用量观测").await?;
        let events: Vec<Result<AgentEvent>> =
            tokio::time::timeout(std::time::Duration::from_secs(6), stream.collect())
                .await
                .map_err(|_| {
                    crate::error::ReactError::Other("usage fixture model watchdog".into())
                })?;
        events.into_iter().collect()
    }

    fn failed_usage_fact(events: &[AgentEvent]) -> Result<(usize, usize, usize, bool)> {
        let usages = events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::LlmUsage {
                    prompt_tokens,
                    completion_tokens,
                    cached_prompt_tokens,
                    usage_reported,
                    observation: Some(observation),
                    ..
                } => Some((
                    *prompt_tokens,
                    *completion_tokens,
                    *cached_prompt_tokens,
                    *usage_reported,
                    observation,
                )),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(usages.len(), 1, "一次真实失败/取消只能有一笔消费观察");
        let (input, output, cached, reported, observation) = usages
            .first()
            .ok_or_else(|| crate::error::ReactError::Other("usage fact missing".into()))?;
        assert_ne!(
            observation.outcome,
            crate::agent::ModelCallOutcome::Completed
        );
        assert_eq!(
            observation
                .observed_output_tokens_per_second(u64::try_from(*output).unwrap_or(u64::MAX)),
            None
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, AgentEvent::FinalAnswer(_)))
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, AgentEvent::ExecutionStatus(status)
            if status.status == echo_core::agent::ExecutionStatusKind::Settled))
        );
        Ok((*input, *output, *cached, *reported))
    }

    #[tokio::test]
    async fn model_call_reported_usage_anthropic_eof_preserves_missing_and_reported_zero()
    -> Result<()> {
        for usage in [
            None,
            Some(serde_json::json!({"input_tokens":0,"output_tokens":0})),
            Some(
                serde_json::json!({"input_tokens":10,"output_tokens":2,"cache_read_input_tokens":7,"cache_creation_input_tokens":3}),
            ),
        ] {
            let mut message = serde_json::json!({"id":"native-anthropic","type":"message","role":"assistant","model":"fixture-model","content":[]});
            if let Some(usage) = &usage {
                message["usage"] = usage.clone();
            }
            let body = format!(
                "data: {}\n\n",
                serde_json::json!({"type":"message_start","message":message})
            );
            let (endpoint, _, server) =
                reported_usage_http_fixture(vec![(200, body)], false).await?;
            let agent = reported_usage_agent(endpoint, crate::llm::LlmApiProtocol::Anthropic)?;
            let events = reported_usage_collect(&agent).await?;
            let value = failed_usage_fact(&events)?;
            assert_eq!(
                value,
                if usage.is_none() {
                    (0, 0, 0, false)
                } else if usage
                    .as_ref()
                    .and_then(|v| v.get("input_tokens"))
                    .and_then(serde_json::Value::as_u64)
                    == Some(0)
                {
                    (0, 0, 0, true)
                } else {
                    (20, 2, 7, true)
                }
            );
            assert!(
                events
                    .iter()
                    .any(|event| matches!(event, AgentEvent::Error { .. }))
            );
            assert_eq!(
                server
                    .await
                    .map_err(|error| crate::error::ReactError::Other(error.to_string()))??,
                1
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn model_call_reported_usage_responses_incomplete_and_chat_eof_keep_cost() -> Result<()> {
        let scenarios = [
            (
                crate::llm::LlmApiProtocol::Responses,
                format!(
                    "data: {}\n\n",
                    serde_json::json!({"type":"response.incomplete","response":{"id":"native-response","status":"incomplete","output":[],"usage":{"input_tokens":30,"output_tokens":4,"total_tokens":34,"input_tokens_details":{"cached_tokens":5}}}})
                ),
                (30, 4, 5, true),
            ),
            (
                crate::llm::LlmApiProtocol::ChatCompletions,
                format!(
                    "data: {}\n\n",
                    serde_json::json!({"id":"native-chat","choices":[],"usage":{"prompt_tokens":12,"completion_tokens":7,"total_tokens":19,"prompt_tokens_details":{"cached_tokens":3}}})
                ),
                (12, 7, 3, true),
            ),
        ];
        for (protocol, body, expected) in scenarios {
            let (endpoint, _, server) =
                reported_usage_http_fixture(vec![(200, body)], false).await?;
            let agent = reported_usage_agent(endpoint, protocol)?;
            let events = reported_usage_collect(&agent).await?;
            assert_eq!(failed_usage_fact(&events)?, expected);
            assert!(
                events
                    .iter()
                    .any(|event| matches!(event, AgentEvent::Error { .. }))
            );
            assert_eq!(
                server
                    .await
                    .map_err(|error| crate::error::ReactError::Other(error.to_string()))??,
                1
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn model_call_reported_usage_anthropic_provider_error_keeps_original_failure()
    -> Result<()> {
        let body = format!(
            "data: {}\n\ndata: {}\n\n",
            serde_json::json!({"type":"message_start","message":{"id":"native-error","type":"message","role":"assistant","model":"fixture-model","content":[],"usage":{"input_tokens":10,"output_tokens":2}}}),
            serde_json::json!({"type":"error","error":{"type":"overloaded_error","message":"usage fixture original error"}})
        );
        let (endpoint, _, server) = reported_usage_http_fixture(vec![(200, body)], false).await?;
        let agent = reported_usage_agent(endpoint, crate::llm::LlmApiProtocol::Anthropic)?;
        let events = reported_usage_collect(&agent).await?;
        assert_eq!(failed_usage_fact(&events)?, (10, 2, 0, true));
        assert!(events.iter().any(|event| matches!(event,AgentEvent::Error {message,..} if message.contains("usage fixture original error"))));
        assert_eq!(
            server
                .await
                .map_err(|error| crate::error::ReactError::Other(error.to_string()))??,
            1
        );
        Ok(())
    }

    #[tokio::test]
    async fn model_call_reported_usage_cancel_retains_received_snapshot() -> Result<()> {
        use crate::agent::Agent;
        for with_null_delta in [false, true] {
            let mut body = format!(
                "data: {}\n\n",
                serde_json::json!({"type":"message_start","message":{"id":"native-cancel","type":"message","role":"assistant","model":"fixture-model","content":[],"usage":{"input_tokens":10,"output_tokens":0}}})
            );
            if with_null_delta {
                body.push_str(&format!("data: {}\n\n",
                serde_json::json!({"type":"message_delta","delta":{"stop_reason":null},"usage":{"input_tokens":null,"output_tokens":7}})));
            }
            let (endpoint, release, server) =
                reported_usage_http_fixture(vec![(200, body)], true).await?;
            let config = crate::llm::LlmConfig::for_provider(
                "usage-fixture",
                format!("{endpoint}/v1/messages"),
                "fixture-key",
                "fixture-model",
                crate::llm::LlmApiProtocol::Anthropic,
            )?;
            let inner: Arc<dyn crate::llm::LlmClient> = Arc::from(config.build_client()?);
            let cancel = crate::agent::CancellationToken::new();
            let mut agent = ReactAgentBuilder::new()
                .configured_llm_client(
                    Arc::new(UsageSourceReadBarrier {
                        inner,
                        cancel: cancel.clone(),
                        after_metadata: if with_null_delta { 2 } else { 1 },
                    }),
                    config,
                )
                .auto_project_rules(false)
                .max_iterations(2)
                .build()?;
            agent.config_mut().llm_max_retries = 0;
            let stream = agent.chat_stream_with_cancel("取消观测", cancel).await?;
            let results: Vec<Result<AgentEvent>> =
                tokio::time::timeout(std::time::Duration::from_secs(6), stream.collect())
                    .await
                    .map_err(|_| crate::error::ReactError::Other("usage cancel watchdog".into()))?;
            release.cancel();
            let events = results.into_iter().collect::<Result<Vec<_>>>()?;
            assert_eq!(
                failed_usage_fact(&events)?,
                if with_null_delta {
                    (10, 7, 0, true)
                } else {
                    (10, 0, 0, true)
                }
            );
            assert!(
                events
                    .iter()
                    .any(|event| matches!(event, AgentEvent::Cancelled))
            );
            assert_eq!(
                server
                    .await
                    .map_err(|error| crate::error::ReactError::Other(error.to_string()))??,
                1
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn model_call_reported_usage_retry_records_one_call_three_attempts_and_final_once()
    -> Result<()> {
        let body = format!(
            "data: {}\n\ndata: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
            serde_json::json!({"id":"native-retry","choices":[{"index":0,"delta":{"role":"assistant","content":"重试完成"},"finish_reason":null}]}),
            serde_json::json!({"id":"native-retry","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}),
            serde_json::json!({"id":"native-retry","choices":[],"usage":{"prompt_tokens":30,"completion_tokens":5,"total_tokens":35}})
        );
        let failure =
            serde_json::json!({"error":{"type":"overloaded_error","message":"retry fixture"}})
                .to_string();
        let (endpoint, _, server) = reported_usage_http_fixture(
            vec![(503, failure.clone()), (503, failure), (200, body)],
            false,
        )
        .await?;
        let mut agent =
            reported_usage_agent(endpoint, crate::llm::LlmApiProtocol::ChatCompletions)?;
        agent.config_mut().llm_max_retries = 2;
        let events = reported_usage_collect(&agent).await?;
        let usages = events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::LlmUsage {
                    prompt_tokens,
                    completion_tokens,
                    usage_reported,
                    observation: Some(observation),
                    ..
                } => Some((
                    *prompt_tokens,
                    *completion_tokens,
                    *usage_reported,
                    observation,
                )),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(usages.len(), 3);
        assert_eq!(
            usages
                .iter()
                .map(|(_, _, _, v)| &v.call_id)
                .collect::<std::collections::HashSet<_>>()
                .len(),
            1
        );
        assert_eq!(
            usages
                .iter()
                .map(|(_, _, _, v)| &v.attempt_id)
                .collect::<std::collections::HashSet<_>>()
                .len(),
            3
        );
        assert_eq!(
            usages
                .iter()
                .filter(|(_, _, reported, _)| *reported)
                .count(),
            1
        );
        assert_eq!(
            usages
                .iter()
                .map(|(input, output, _, _)| input.saturating_add(*output))
                .sum::<usize>(),
            35
        );
        assert_eq!(
            usages.last().map(|(_, _, _, v)| v.outcome),
            Some(crate::agent::ModelCallOutcome::Completed)
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, AgentEvent::FinalAnswer(_)))
                .count(),
            1
        );
        assert_eq!(
            server
                .await
                .map_err(|error| crate::error::ReactError::Other(error.to_string()))??,
            3
        );
        Ok(())
    }

    struct UsageSourceReadBarrier {
        inner: Arc<dyn crate::llm::LlmClient>,
        cancel: crate::agent::CancellationToken,
        after_metadata: usize,
    }

    impl crate::llm::LlmClient for UsageSourceReadBarrier {
        fn chat(
            &self,
            request: crate::llm::ChatRequest,
        ) -> futures::future::BoxFuture<'_, Result<crate::llm::ChatResponse>> {
            self.inner.chat(request)
        }
        fn chat_stream(
            &self,
            request: crate::llm::ChatRequest,
        ) -> futures::future::BoxFuture<
            '_,
            Result<futures::stream::BoxStream<'static, Result<crate::llm::ChatChunk>>>,
        > {
            let cancel = self.cancel.clone();
            let after_metadata = self.after_metadata;
            Box::pin(async move {
                let inner = self.inner.chat_stream(request).await?;
                let mut observed = 0_usize;
                let stream = inner.inspect(move |sample| {
                    if sample
                        .as_ref()
                        .ok()
                        .and_then(|chunk| chunk.delta.provider_response.as_ref())
                        .is_some_and(|metadata| metadata.usage.is_some())
                    {
                        observed = observed.saturating_add(1);
                        // 只在真实源poll已经Ready的metadata上触发；不等待Guard后公开正文。
                        if observed == after_metadata {
                            cancel.cancel();
                        }
                    }
                });
                Ok(Box::pin(stream)
                    as futures::stream::BoxStream<
                        'static,
                        Result<crate::llm::ChatChunk>,
                    >)
            })
        }
        fn model_name(&self) -> &str {
            self.inner.model_name()
        }
        fn provider_name(&self) -> Option<&str> {
            self.inner.provider_name()
        }
        fn reported_usage(
            &self,
            observation: &crate::llm::types::ProviderResponseMetadata,
            previous: Option<&Usage>,
        ) -> Result<Option<Usage>> {
            self.inner.reported_usage(observation, previous)
        }
        fn protocol_capabilities(
            &self,
        ) -> echo_core::llm::capabilities::ProviderCapabilityOverride {
            self.inner.protocol_capabilities()
        }
        fn model_profile_resolution(&self) -> echo_core::llm::capabilities::ModelProfileResolution {
            self.inner.model_profile_resolution()
        }
        fn capabilities(&self) -> echo_core::llm::capabilities::ProviderCapabilities {
            self.inner.capabilities()
        }
    }

    #[tokio::test]
    async fn model_call_reported_usage_anthropic_null_snapshots_keep_original_accumulation()
    -> Result<()> {
        for (input, cache, expected) in [
            (
                serde_json::Value::Null,
                serde_json::Value::Null,
                (10, 7, 5, true),
            ),
            (serde_json::json!(0), serde_json::json!(0), (0, 7, 0, true)),
        ] {
            let body = format!(
                "data: {}\n\ndata: {}\n\n",
                serde_json::json!({"type":"message_start","message":{"id":"native-null","type":"message","role":"assistant","model":"fixture-model","content":[],"usage":{"input_tokens":2,"output_tokens":0,"cache_read_input_tokens":5,"cache_creation_input_tokens":3}}}),
                serde_json::json!({"type":"message_delta","delta":{"stop_reason":null},"usage":{"input_tokens":input,"output_tokens":7,"cache_read_input_tokens":cache,"cache_creation_input_tokens":cache}})
            );
            let (endpoint, _, server) =
                reported_usage_http_fixture(vec![(200, body)], false).await?;
            let agent = reported_usage_agent(endpoint, crate::llm::LlmApiProtocol::Anthropic)?;
            let events = reported_usage_collect(&agent).await?;
            assert_eq!(failed_usage_fact(&events)?, expected);
            assert_eq!(
                server
                    .await
                    .map_err(|error| crate::error::ReactError::Other(error.to_string()))??,
                1
            );
        }
        Ok(())
    }

    fn timing_chunk(
        delta: serde_json::Value,
    ) -> std::result::Result<crate::llm::types::ChatCompletionChunk, serde_json::Error> {
        serde_json::from_value(serde_json::json!({"choices":[{"index":0,"delta":delta}]}))
    }

    #[test]
    fn model_output_timing_ignores_metadata_and_uses_original_stream_interval()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut timing = ModelOutputTiming::default();
        timing.observe(&timing_chunk(serde_json::json!({"role":"assistant"}))?, 10);
        assert_eq!(timing.samples, 0);
        timing.observe(&timing_chunk(serde_json::json!({"content":"先检查"}))?, 200);
        timing.observe(
            &timing_chunk(serde_json::json!({"content":"后验证"}))?,
            1200,
        );
        timing.observe(&timing_chunk(serde_json::json!({}))?, 1250);
        timing.finished = Some(1250);
        let usage = Usage {
            prompt_tokens: Some(50),
            completion_tokens: Some(100),
            ..Default::default()
        };
        let observation = timing.observation(
            "logical-call",
            Some(&usage),
            Instant::now(),
            crate::agent::ModelCallOutcome::Completed,
        );
        assert_eq!(observation.first_output_ms, Some(200));
        assert_eq!(observation.last_output_ms, Some(1200));
        assert_eq!(observation.request_duration_ms, 1250);
        assert_eq!(observation.output_samples, 2);
        assert_eq!(
            observation.observed_output_tokens_per_second(100),
            Some(100.0)
        );
        assert_eq!(
            timing
                .observation(
                    "logical-call",
                    None,
                    Instant::now(),
                    crate::agent::ModelCallOutcome::Completed
                )
                .observed_output_tokens_per_second(100),
            None
        );
        Ok(())
    }

    #[test]
    fn model_output_timing_keeps_tool_output_and_refuses_unobserved_reasoning()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut timing = ModelOutputTiming::default();
        timing.observe(
            &timing_chunk(serde_json::json!({"tool_calls":[{"index":0,"id":"native-call"}]}))?,
            10,
        );
        assert_eq!(timing.samples, 0);
        timing.observe(
            &timing_chunk(
                serde_json::json!({"tool_calls":[{"index":0,"function":{"arguments":"{"}}]}),
            )?,
            100,
        );
        timing.observe(
            &timing_chunk(
                serde_json::json!({"tool_calls":[{"index":0,"function":{"arguments":"}"}}]}),
            )?,
            1100,
        );
        timing.finished = Some(1150);
        let usage = Usage {
            completion_tokens: Some(100),
            ..Default::default()
        };
        assert_eq!(
            timing
                .observation(
                    "call",
                    Some(&usage),
                    Instant::now(),
                    crate::agent::ModelCallOutcome::Completed
                )
                .observed_output_tokens_per_second(100),
            Some(100.0)
        );
        timing.observe(
            &timing_chunk(serde_json::json!({"reasoning_blocks":[{
                "type":"opaque","provider":"responses","id":"original","data":"opaque","summary":[]
            }]}))?,
            1150,
        );
        assert_eq!(
            timing
                .observation(
                    "call",
                    Some(&usage),
                    Instant::now(),
                    crate::agent::ModelCallOutcome::Completed
                )
                .observed_output_tokens_per_second(100),
            None
        );
        Ok(())
    }

    #[tokio::test]
    async fn failed_model_usage_preserves_reported_consumption_without_publishing_terminal()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let agent =
            crate::agent::ReactAgent::new(crate::agent::AgentConfig::new("model", "usage", "sys"));
        let snapshot = AgentRunSnapshot::from_agent(&agent);
        let (tx, mut rx) = mpsc::channel(4);
        let usage = Usage {
            prompt_tokens: Some(50),
            completion_tokens: Some(5),
            total_tokens: Some(55),
            ..Default::default()
        };
        observe_failed_model_usage(
            &snapshot,
            &tx,
            Some(&usage),
            None,
            "logical-call",
            (&ModelOutputTiming::default(), Instant::now()),
            crate::agent::ModelCallOutcome::Failed,
        )
        .await;
        let event = rx.try_recv()??;
        assert_eq!(event.tokens_used(), Some(55));
        assert!(!event.is_execution_terminal());
        assert!(matches!(
            event,
            AgentEvent::LlmUsage {
                usage_reported: true,
                observation: Some(crate::agent::ModelCallObservation {
                    outcome: crate::agent::ModelCallOutcome::Failed,
                    ..
                }),
                ..
            }
        ));
        assert!(rx.try_recv().is_err());
        Ok(())
    }

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
