//! ReAct loop core (think / process_steps / run_react_loop)

use super::super::{ReactAgent, StepType, TOOL_FINAL_ANSWER};
use super::execution::{ToolExecutionFailure, ToolExecutionOutcome};
use super::types::StreamMode;
use crate::agent::AgentEvent;
use crate::agent::snapshot::AgentRunSnapshot;
use crate::error::{AgentError, ReactError, Result, ToolError};
use crate::guard::GuardDirection;
use crate::llm::types::Message;
use crate::llm::{ChatRequest, chat};
use futures::future::join_all;
use serde_json::Value;
use std::time::Instant;
use tokio::sync::mpsc;
use tracing::{Instrument, debug, info, info_span, warn};

impl ReactAgent {
    /// Unified LLM call with retry, diagnostics logging, and circuit breaker.
    ///
    /// Handles both custom `llm_client` and raw HTTP paths, returning a
    /// normalized `(message, usage, finish_reason)` tuple.
    async fn call_llm_with_retry(
        &self,
        messages: &[Message],
        tools: Vec<crate::llm::types::ToolDefinition>,
    ) -> Result<(Message, Option<crate::llm::types::Usage>, String)> {
        let agent = &self.config.agent_name;
        let max_retries = self.config.llm_max_retries;
        let retry_delay = self.config.llm_retry_delay_ms;
        let circuit_breaker = self.guard.circuit_breaker.clone();
        let temperature = self.config.temperature;
        let max_tokens = self.config.max_tokens;
        let response_format = self.config.response_format.clone();

        if let Some(llm_client) = self.llm_client.clone() {
            let ms = messages.to_vec();
            let t = tools.clone();
            // Build cache hints from the stable message layout (before moving into request).
            let layout = echo_core::llm::cache::PromptCacheLayout::from_messages(&ms, &t);
            let prefix_hash = echo_core::llm::cache::diagnostic::stable_prefix_hash(
                layout.system,
                layout.canonical,
                layout.tools,
                layout.history,
            );
            let cache_hints = echo_core::llm::cache::CacheHints {
                breakpoints: vec![],
                stable_prefix_hash: Some(prefix_hash),
                segments: layout.segment_ranges(),
            };
            let request = ChatRequest {
                messages: ms,
                temperature,
                max_tokens,
                tools: Some(t),
                tool_choice: None,
                response_format: response_format.clone(),
                thinking: self.thinking.clone(),
                cancel_token: None,
                user_id: self.config.cache_user_id.clone(),
                cache_hints: Some(cache_hints),
            };
            let msg_count = request.messages.len();
            let tool_count = request.tools.as_ref().map_or(0, |t| t.len());
            let last_msg_preview = request.messages.last().map(|m| {
                let role = m.role.as_str();
                let content = m.content.as_text().unwrap_or_default();
                let preview: String = content.chars().take(200).collect();
                format!("[{role}] {preview}")
            });
            warn!(
                agent = %agent,
                msg_count,
                tool_count,
                temperature = request.temperature,
                max_tokens = request.max_tokens,
                last_msg = ?last_msg_preview,
                "📤 LLM request"
            );
            let response = super::retry::retry_llm_call(
                agent,
                max_retries,
                retry_delay,
                &circuit_breaker,
                || {
                    let llm_client = llm_client.clone();
                    let request = request.clone();
                    async move { llm_client.chat(request).await }
                },
            )
            .await?;
            warn!(
                agent = %agent,
                finish_reason = ?response.finish_reason,
                has_tool_calls = response.has_tool_calls(),
                content_preview = ?response.content().map(|c| c.chars().take(200).collect::<String>()),
                "📥 LLM response"
            );
            let usage = response.raw.usage.clone();
            let finish_reason = response.finish_reason.unwrap_or_default();
            Ok((response.message, usage, finish_reason))
        } else {
            let client = self.client.clone();
            let model_name = self.config.model_name.clone();
            let cache_user_id = self.config.cache_user_id.clone();
            let msg_count = messages.len();
            let tool_count = tools.len();
            let last_msg_preview = messages.last().map(|m| {
                let role = m.role.as_str();
                let content = m.content.as_text().unwrap_or_default();
                let preview: String = content.chars().take(200).collect();
                format!("[{role}] {preview}")
            });
            warn!(
                agent = %agent,
                msg_count,
                tool_count,
                temperature,
                max_tokens,
                last_msg = ?last_msg_preview,
                "📤 LLM request"
            );
            let response = super::retry::retry_llm_call(
                agent,
                max_retries,
                retry_delay,
                &circuit_breaker,
                || {
                    let client = client.clone();
                    let model_name = model_name.as_str();
                    let messages = messages.to_vec();
                    let tools = tools.clone();
                    let response_format = response_format.clone();
                    let user_id = cache_user_id.clone();
                    async move {
                        chat(
                            client,
                            model_name,
                            &messages,
                            temperature,
                            max_tokens,
                            Some(false),
                            Some(tools),
                            None,
                            response_format,
                            user_id,
                        )
                        .await
                    }
                },
            )
            .await?;
            let usage = response.usage.clone();
            let choice = response.choices.first().ok_or(ReactError::Agent(Box::new(
                AgentError::NoResponse {
                    model: self.config.model_name.clone(),
                    agent: self.config.agent_name.clone(),
                },
            )))?;
            let finish_reason = choice.finish_reason.clone().unwrap_or_default();
            let message = choice.message.clone();
            warn!(
                agent = %agent,
                finish_reason = ?finish_reason,
                has_tool_calls = message.tool_calls.as_ref().is_some_and(|t| !t.is_empty()),
                content_preview = ?message.content.as_text().map(|c| c.chars().take(200).collect::<String>()),
                "📥 LLM response"
            );
            Ok((message, usage, finish_reason))
        }
    }

    /// Process steps produced by one think round:
    /// - Tool calls → execute in parallel (approval-required tools are serialized), return answer on `final_answer`
    /// - No tool calls → plain text response treated as final answer, returned directly
    #[allow(dead_code)]
    #[tracing::instrument(skip(self, steps), fields(agent = %self.config.agent_name, tool_count = steps.iter().filter(|s| matches!(s, StepType::Call { .. })).count()))]
    pub(crate) async fn process_steps(&self, steps: Vec<StepType>) -> Result<Option<String>> {
        let agent = self.config.agent_name.clone();
        let mut tool_calls: Vec<(String, String, Value)> = Vec::new();
        let mut last_thought: Option<String> = None;

        for step in steps {
            match step {
                StepType::Call {
                    tool_call_id,
                    function_name,
                    arguments,
                } => {
                    tool_calls.push((tool_call_id, function_name, arguments));
                }
                StepType::Thought(content) => {
                    debug!(agent = %agent, "🤔 Thought: {}", content);
                    last_thought = Some(content);
                }
            }
        }

        if tool_calls.is_empty() {
            return Ok(last_thought.filter(|s| !s.is_empty()));
        }

        let max_concurrency = self.tools.tool_manager.max_concurrency();
        if tool_calls.len() > 1 {
            let tool_names: Vec<&str> = tool_calls.iter().map(|(_, n, _)| n.as_str()).collect();
            info!(
                agent = %agent,
                tools = ?tool_names,
                max_concurrency = ?max_concurrency,
                "⚡ Concurrently executing {} tool calls",
                tool_calls.len()
            );
        }

        // Separate tools into approval-required and non-approval groups.
        // Only serialize the approval-required tools; let others continue concurrently.
        #[cfg(feature = "human-loop")]
        let (approval_tools, concurrent_tools) = {
            let mut approval = Vec::new();
            let mut concurrent = Vec::new();
            for tc in tool_calls {
                if self.tool_needs_approval(&tc.1).await {
                    approval.push(tc);
                } else {
                    concurrent.push(tc);
                }
            }
            (approval, concurrent)
        };
        #[cfg(not(feature = "human-loop"))]
        let (approval_tools, concurrent_tools) =
            (Vec::<(String, String, Value)>::new(), tool_calls);

        // Extract tool names before concurrent execution so they are available
        // for PostToolBatch hook even on batch timeout.
        let batch_tool_names: Vec<String> = concurrent_tools
            .iter()
            .map(|(_, name, _)| name.clone())
            .collect();

        // Determine which tools are exempt from the parallel batch timeout.
        //
        // Long-running tools (subagent dispatch such as `agent_tool` /
        // `delegate_readonly`, web research) that internally run their own
        // multi-step ReAct declare `exempt_from_batch_timeout() -> true`. Their
        // latency is inherently far higher than typical file/shell tools and
        // would otherwise dominate the batch budget, prematurely cancelling
        // peers. Such tools are separated out and run without the outer batch
        // timeout (each carries its own per-execution timeout, e.g. the
        // subagent 600s default in `SubagentExecutor`).
        //
        // Collect exempt tool indices up front so the DashMap Ref is dropped
        // before any await below (no read lock held across tool futures).
        let exempt_indices: std::collections::HashSet<usize> = concurrent_tools
            .iter()
            .enumerate()
            .filter_map(|(i, (_, name, _))| {
                self.tools
                    .tool_manager
                    .get_tool(name)
                    .map(|entry| entry.value().exempt_from_batch_timeout())
                    .unwrap_or(false)
                    .then_some(i)
            })
            .collect();
        let timed_indices: Vec<usize> = (0..concurrent_tools.len())
            .filter(|i| !exempt_indices.contains(i))
            .collect();
        let exempt_indices_vec: Vec<usize> = exempt_indices.iter().copied().collect();
        if !exempt_indices.is_empty() {
            let names: Vec<&str> = exempt_indices_vec
                .iter()
                .map(|&i| concurrent_tools[i].1.as_str())
                .collect();
            info!(
                agent = %agent,
                tools = ?names,
                "⏳ Running {} exempt (long-running) tools without batch timeout",
                exempt_indices.len()
            );
        }

        // Execute non-approval tools concurrently, split into two groups:
        //   - timed: subject to batch_timeout (original behavior).
        //   - exempt: joined without an outer timeout.
        // Results are placed back into a Vec indexed by the ORIGINAL position
        // so downstream context push / hook / statistics (which zip
        // `concurrent_tools` with `concurrent_results`) stays aligned and
        // unchanged.
        let concurrent_results: Vec<
            std::result::Result<ToolExecutionOutcome, ToolExecutionFailure>,
        > = if concurrent_tools.is_empty() {
            Vec::new()
        } else {
            let mut results: Vec<
                Option<std::result::Result<ToolExecutionOutcome, ToolExecutionFailure>>,
            > = (0..concurrent_tools.len()).map(|_| None).collect();

            // 1) Timed batch (with batch_timeout, original logic).
            if !timed_indices.is_empty() {
                let futures: Vec<_> = timed_indices
                    .iter()
                    .map(|&i| {
                        let (_, name, args) = &concurrent_tools[i];
                        self.execute_tool_feedback_raw(name, args, self.config.tool_error_feedback)
                            .instrument(info_span!("tool_execute", tool.name = %name))
                    })
                    .collect();
                let batch_timeout = super::retry::compute_concurrent_tool_batch_timeout(
                    &self.config.tool_execution,
                    futures.len(),
                    max_concurrency,
                );
                let timed_results = if let Some(timeout) = batch_timeout {
                    match tokio::time::timeout(timeout, join_all(futures)).await {
                        Ok(results) => results,
                        Err(_) => {
                            // Fire PostToolBatch hook before returning timeout error
                            let hook_ctx = crate::skills::hooks::HookContext::for_post_tool_batch(
                                &batch_tool_names,
                                0,
                                batch_tool_names.len(),
                                self.config.session_id.as_deref().unwrap_or(""),
                                &self.config.agent_name,
                            );
                            let registry = self.tools.hook_registry.read().await.clone();
                            let batch_result = registry.run_lifecycle_hooks(&hook_ctx).await;
                            if let Some(ctx) = &batch_result.injected_context {
                                super::context::push_runtime_context_note(
                                    &self.memory.context,
                                    "Hook:PostToolBatch",
                                    ctx,
                                )
                                .await;
                            }
                            return Err(ToolError::Timeout(format!(
                                "parallel tool batch exceeded total timeout after {:?}",
                                timeout
                            ))
                            .into());
                        }
                    }
                } else {
                    join_all(futures).await
                };
                for (i, r) in timed_indices.into_iter().zip(timed_results.into_iter()) {
                    results[i] = Some(r);
                }
            }

            // 2) Exempt batch (no outer timeout — each tool has its own
            //    per-execution timeout, e.g. subagent 600s default).
            if !exempt_indices_vec.is_empty() {
                let futures: Vec<_> = exempt_indices_vec
                    .iter()
                    .map(|&i| {
                        let (_, name, args) = &concurrent_tools[i];
                        self.execute_tool_feedback_raw(name, args, self.config.tool_error_feedback)
                            .instrument(info_span!("tool_execute", tool.name = %name))
                    })
                    .collect();
                let exempt_results = join_all(futures).await;
                for (i, r) in exempt_indices_vec
                    .into_iter()
                    .zip(exempt_results.into_iter())
                {
                    results[i] = Some(r);
                }
            }

            // Every slot must be filled (each tool went into exactly one group).
            results
                .into_iter()
                .map(|r| {
                    r.unwrap_or_else(|| {
                        // Defensive: should be unreachable (every index is timed or exempt).
                        Err(ToolExecutionFailure {
                            error: ReactError::Other(
                                "tool result slot not filled (internal invariant violation)".into(),
                            ),
                            hook_messages: Default::default(),
                        })
                    })
                })
                .collect()
        };

        // Push concurrent results to context
        let mut final_answer: Option<String> = None;
        let mut batch_success_count = 0usize;
        let mut batch_failure_count = 0usize;
        let mut first_failure: Option<ReactError> = None;
        for ((tool_call_id, function_name, _), result) in
            concurrent_tools.into_iter().zip(concurrent_results)
        {
            let output = match result {
                Ok(outcome) => {
                    self.apply_hook_messages(&function_name, &outcome.hook_messages)
                        .await;
                    batch_success_count += 1;
                    // Apply truncation to tool output for token budget management
                    self.truncate_tool_output(outcome.output).await
                }
                Err(failure) => {
                    self.apply_hook_messages(&function_name, &failure.hook_messages)
                        .await;
                    batch_failure_count += 1;
                    let error_display = failure.error.to_string();
                    if first_failure.is_none() {
                        first_failure = Some(failure.error);
                    }
                    format!("[error: {}]", error_display)
                }
            };
            self.memory.context.lock().await.push(Message::tool_result(
                tool_call_id,
                function_name.clone(),
                output.clone(),
            ));
            if function_name == TOOL_FINAL_ANSWER {
                // ── Intervention callbacks for final answer ──
                let mut answer_blocked = false;
                for intervention in &self.tools.intervention_callbacks {
                    let result = intervention.on_final_answer(&agent, &output).await;
                    if result.cancel {
                        return Err(ReactError::Other(
                            "Agent execution cancelled by intervention at final answer".into(),
                        ));
                    }
                    if result.block {
                        let reason = result
                            .block_reason
                            .unwrap_or_else(|| "blocked by intervention at final answer".into());
                        warn!(agent = %agent, reason = %reason, "Intervention blocked final answer");
                        answer_blocked = true;
                        break;
                    }
                    if let Some(context) = result.injected_context {
                        super::context::push_runtime_context_note(
                            &self.memory.context,
                            "Intervention:FinalAnswer",
                            &context,
                        )
                        .await;
                    }
                }
                if !answer_blocked {
                    info!(agent = %agent, "🏁 Final answer generated");
                    final_answer = Some(output);
                }
            }
        }

        // Fire PostToolBatch hook for the concurrent tool batch
        if !batch_tool_names.is_empty() {
            let hook_ctx = crate::skills::hooks::HookContext::for_post_tool_batch(
                &batch_tool_names,
                batch_success_count,
                batch_failure_count,
                self.config.session_id.as_deref().unwrap_or(""),
                &self.config.agent_name,
            );
            let registry = self.tools.hook_registry.read().await.clone();
            let batch_result = registry.run_lifecycle_hooks(&hook_ctx).await;
            if let Some(ctx) = &batch_result.injected_context {
                super::context::push_runtime_context_note(
                    &self.memory.context,
                    "Hook:PostToolBatch",
                    ctx,
                )
                .await;
            }
        }

        // Return first failure (after PostToolBatch has fired)
        if let Some(err) = first_failure {
            return Err(err);
        }

        // Execute approval tools sequentially
        for (tool_call_id, function_name, arguments) in approval_tools {
            let result = self
                .execute_tool_feedback(&function_name, &arguments)
                .await?;
            self.memory.context.lock().await.push(Message::tool_result(
                tool_call_id,
                function_name.clone(),
                result.clone(),
            ));
            if function_name == TOOL_FINAL_ANSWER {
                info!(agent = %agent, "🏁 Final answer generated");
                return Ok(Some(result));
            }
        }

        if final_answer.is_some() {
            return Ok(final_answer);
        }

        Ok(None)
    }

    /// Prepare non-streaming execution context: guard check, turn tracking,
    /// memory recall, push user message, start trace run.
    ///
    /// Returns the number of recalled long-term memories (for the `MemoryRecalled` event).
    async fn prepare_react_context(&self, message: &str) -> Result<usize> {
        let agent = self.config.agent_name.clone();

        // Begin a new agent turn (phase: ReceiveInput)
        let mut turn = crate::agent::turn::AgentTurn::new(message);
        *self.current_turn.lock().unwrap_or_else(|e| e.into_inner()) = Some(turn.clone());

        // Clear read-before-edit tracking for the new conversation turn
        self.clear_read_files();

        // Input guard check
        if let Some(gm) = &self.guard.guard_manager {
            info!(agent = %agent, direction = "input", "🛡️ Guard check started");
            let result = gm.check_all(message, GuardDirection::Input).await?;
            if let crate::guard::GuardResult::Block { reason } = &result {
                info!(agent = %agent, reason = %reason, "🛡️ Input blocked by guard");
                if let Some(al) = &self.guard.audit_logger {
                    let event = crate::audit::AuditEvent::now(
                        self.config.session_id.clone(),
                        agent.clone(),
                        crate::audit::AuditEventType::GuardBlock {
                            guard: "guard_manager".to_string(),
                            direction: GuardDirection::Input,
                            reason: reason.clone(),
                        },
                    );
                    if let Err(e) = al.log(event).await {
                        tracing::warn!(error = %e, "Failed to log guard audit event");
                    }
                }
                return Err(ReactError::Other(format!(
                    "Request blocked by safety guard: {reason}"
                )));
            }
        }

        // Trace identity is independent from the product run identity and is
        // established before the first observable execution phase.
        self.start_trace_run(message).await;

        // Phase: Recall
        turn.advance(crate::agent::turn::TurnPhase::Recall);
        self.record_trace_event(crate::trace::RunEvent::PhaseTransition {
            phase: "recall".into(),
            iteration: 0,
        })
        .await;
        *self.current_turn.lock().unwrap_or_else(|e| e.into_inner()) = Some(turn.clone());

        // Persist memory-worthy triggers before recall injection mutates context.
        self.detect_and_write_memory_triggers(message).await;

        // Inject relevant long-term memories
        let mut recalled = 0usize;
        let mut memory_context = None;
        match self.recall_long_term_memories(message).await {
            Ok(items) if !items.is_empty() => {
                recalled = items.len();
                debug!(agent = %agent, count = items.len(), "📚 Injecting relevant long-term memories");
                memory_context = Some(super::context::format_memory_context(&items));
            }
            Ok(_) => {}
            Err(e) => {
                warn!(agent = %agent, error = %e, "⚠️ Long-term memory retrieval failed, skipping injection");
            }
        }

        let wd = self.config.working_dir.lock().ok().and_then(|g| g.clone());
        let ws_block = crate::agent::react::ReactAgent::build_workspace_context_block(wd.as_ref());
        let mut context = self.memory.context.lock().await;
        context.replace_projection(
            super::context::WORKSPACE_CONTEXT_PROJECTION,
            (!ws_block.trim().is_empty())
                .then(|| super::context::runtime_context_note("workspace", &ws_block)),
        );
        context.replace_tail_projection(
            super::context::TURN_MEMORY_CONTEXT_PROJECTION,
            memory_context
                .map(|body| super::context::runtime_context_note("memory", body.as_str())),
        );
        context.push(Message::user(message.to_string()));

        Ok(recalled)
    }

    /// Core ReAct loop — thin wrapper that delegates to the shared `run_core_loop`.
    ///
    /// Creates a snapshot + channel, runs the unified core loop in a spawned task,
    /// then collects `FinalAnswer` from the event stream.
    #[tracing::instrument(skip(self, message), fields(agent = %self.config.agent_name, model = %self.config.model_name))]
    pub(crate) async fn run_react_loop(&self, message: &str) -> Result<String> {
        // ★ Serialize all execution on this agent — only one run at a time.
        let _execution_guard = self.execution_mutex.lock().await;

        // Prepare context (guard check, memory recall, push message, start trace)
        let recalled = match self.prepare_react_context(message).await {
            Ok(n) => n,
            Err(e) => {
                // Guard blocked — return the message directly (not an error)
                let msg = e.to_string();
                if msg.starts_with("Request blocked by safety guard:") {
                    return Ok(msg);
                }
                return Err(e);
            }
        };
        let turn_id = self
            .current_run_id
            .lock()
            .ok()
            .and_then(|value| value.clone())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let active_turn_lease = self.turn_steer_mailbox.begin(turn_id);

        // ★ NEW: Intent routing
        if let Some(ref router) = self.intent_router {
            let messages = self.memory.context.lock().await.messages().to_vec();
            let intent = router.classify(message, &messages).await;
            match intent {
                crate::intent::Intent::DirectAnswer { confidence }
                    if self.allows_direct_answer_shortcut() =>
                {
                    tracing::info!(
                        agent = %self.config.agent_name,
                        confidence = confidence,
                        "🎯 IntentRouter: DirectAnswer shortcut"
                    );
                    return match self.direct_answer(message).await {
                        Ok(answer) => {
                            self.finalize_trace_run(
                                crate::trace::RunStatus::Completed,
                                Some(&answer),
                                None,
                            )
                            .await;
                            Ok(answer)
                        }
                        Err(error) => {
                            let error_text = error.to_string();
                            self.finalize_trace_run(
                                crate::trace::RunStatus::Failed,
                                None,
                                Some(error_text.as_str()),
                            )
                            .await;
                            Err(error)
                        }
                    };
                }
                crate::intent::Intent::DirectAnswer { confidence } => {
                    tracing::debug!(
                        agent = %self.config.agent_name,
                        confidence,
                        "DirectAnswer routed through ReAct for pre-model projection"
                    );
                }
                crate::intent::Intent::SkillRequired {
                    skill_name,
                    confidence,
                } => {
                    tracing::info!(
                        agent = %self.config.agent_name,
                        skill = %skill_name,
                        confidence = confidence,
                        "🎯 IntentRouter: activating skill"
                    );
                    if let Err(e) = self.activate_skill(&skill_name).await {
                        tracing::warn!(skill = %skill_name, error = %e, "IntentRouter: failed to activate skill");
                    }
                }
                crate::intent::Intent::Fallback => {
                    tracing::debug!(agent = %self.config.agent_name, "IntentRouter: Fallback to ReAct");
                }
            }
        }

        // Create channel + snapshot
        active_turn_lease.set_steerable(true);
        let (tx, mut rx) = mpsc::channel::<Result<AgentEvent>>(self.config.stream_buffer_size);
        let mut snap = AgentRunSnapshot::from_agent(self);
        snap.current_run_id = self
            .current_run_id
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        snap.external_cancel = self
            .external_cancel
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        snap.external_trace_sink = self
            .external_trace_sink
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        snap.external_delegation_policy = *self
            .external_delegation_policy
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        // Run the shared core loop in a spawned task
        let context = self.memory.context.clone();
        let text = message.to_string();
        tokio::spawn(async move {
            if let Err(e) = snap
                .run_core_loop(
                    context,
                    text,
                    None,
                    String::new(),
                    StreamMode::Chat,
                    recalled,
                    tx,
                )
                .await
            {
                // Error already sent via tx in most cases; log as fallback
                tracing::warn!(error = %e, "Core loop error (already sent via channel)");
            }
        });

        // Collect events, extract FinalAnswer
        let mut answer = String::new();
        while let Some(event) = rx.recv().await {
            match event {
                Ok(AgentEvent::FinalAnswer(a)) => {
                    answer = a;
                    break;
                }
                Ok(AgentEvent::Cancelled) => {
                    answer = "Cancelled.".to_string();
                    break;
                }
                Ok(AgentEvent::Error { message, .. }) => {
                    return Err(ReactError::Other(message));
                }
                Err(e) => return Err(e),
                _ => {} // Ignore intermediate events (Token, ToolCall, etc.)
            }
        }

        drop(active_turn_lease);
        Ok(answer)
    }

    /// Direct answer: bypass ReAct loop and call LLM directly.
    /// Used by IntentRouter for simple intents (greetings, weather, etc.).
    async fn direct_answer(&self, message: &str) -> Result<String> {
        let system_prompt = self.config.system_prompt.clone();
        let messages = vec![
            crate::llm::types::Message::system(system_prompt),
            crate::llm::types::Message::user(message.to_string()),
        ];

        let llm_started = Instant::now();
        let (response, usage, _finish_reason) = self.call_llm_with_retry(&messages, vec![]).await?;
        let content = response.content.as_text().unwrap_or_default().to_string();
        let prompt_tokens = usage
            .as_ref()
            .map(|value| value.effective_prompt_tokens())
            .unwrap_or(0);
        let completion_tokens = usage
            .as_ref()
            .and_then(|value| value.completion_tokens)
            .unwrap_or(0);
        let cached_prompt_tokens = usage
            .as_ref()
            .map(|value| value.cached_prompt_tokens())
            .unwrap_or(0);
        let cache_creation_prompt_tokens = usage
            .as_ref()
            .map(|value| value.cache_creation_prompt_tokens())
            .unwrap_or(0);
        if let Some(ref usage) = usage {
            self.token_tracker.record_usage(usage);
        }
        let estimated_context_tokens = {
            use echo_core::tokenizer::Tokenizer;
            messages
                .iter()
                .filter_map(|value| value.text_content())
                .fold(0usize, |total, text| {
                    total.saturating_add(self.calibrated_tokenizer.count_tokens(&text))
                })
        };

        // Record trace
        self.record_trace_event(crate::trace::RunEvent::LlmCall {
            messages: messages.len(),
            prompt_tokens,
            completion_tokens,
            cached_prompt_tokens,
            cache_creation_prompt_tokens,
            usage_reported: usage.is_some(),
            estimated_context_tokens,
            protected_context_tokens: 0,
            protected_message_count: 0,
            context_limit_tokens: self.config.token_limit,
            context_breakdown: crate::trace::LlmContextBreakdown::estimate(
                &messages,
                self.calibrated_tokenizer.as_ref(),
            ),
            cache_fingerprint: super::phases::think::cache_fingerprint(&messages, None),
            duration_ms: u64::try_from(llm_started.elapsed().as_millis()).unwrap_or(u64::MAX),
        })
        .await;

        // Push to context so the agent remembers this turn
        self.memory
            .context
            .lock()
            .await
            .push(crate::llm::types::Message::assistant(content.clone()));

        Ok(content)
    }
}
