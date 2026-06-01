//! ReAct loop core (think / process_steps / run_react_loop)

use super::super::{ReactAgent, StepType, TOOL_FINAL_ANSWER};
use super::execution::{ToolExecutionFailure, ToolExecutionOutcome};
use crate::error::{AgentError, ReactError, Result, ToolError};
use crate::guard::GuardDirection;
use crate::llm::types::Message;
use crate::llm::{ChatRequest, chat};
use futures::future::join_all;
use serde_json::Value;
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
            let request = ChatRequest {
                messages: messages.to_vec(),
                temperature,
                max_tokens,
                tools: Some(tools.clone()),
                tool_choice: None,
                response_format: response_format.clone(),
                cancel_token: None,
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

    /// Call LLM for reasoning, returning the list of steps for this round.
    ///
    /// Before each call, `ContextManager::prepare` auto-compresses overflow history messages,
    /// then the compressed message list is passed to the LLM; the LLM response is appended back to context.
    #[tracing::instrument(skip(self), fields(agent = %self.config.agent_name, model = %self.config.model_name))]
    pub(crate) async fn think(&self) -> Result<Vec<StepType>> {
        let agent = self.config.agent_name.clone();
        let callbacks = self.config.callbacks.clone();
        let mut res = Vec::new();

        debug!(agent = %agent, model = %self.config.model_name, "🧠 LLM thinking...");

        // ContextManager::prepare handles compression internally — no need for duplicate pre-check here.
        // Fire PreCompact hooks before compression
        let pre_compact_result = self
            .fire_lifecycle_hook(crate::skills::hooks::HookEvent::PreCompact, Some("auto"))
            .await;

        let prepare_result = self.memory.context.lock().await.prepare(None).await?;

        if let Some(ref stats) = prepare_result.compressed {
            tracing::info!(
                agent = %agent,
                before = stats.before_count,
                after = stats.after_count,
                before_tokens = stats.before_tokens,
                after_tokens = stats.after_tokens,
                "📦 Context auto-compressed"
            );
            // Fire PostCompact hooks with actual compression stats
            {
                let hook_stats = crate::skills::hooks::CompressHookStats {
                    before_count: stats.before_count,
                    after_count: stats.after_count,
                    before_tokens: stats.before_tokens,
                    after_tokens: stats.after_tokens,
                };
                let hook_ctx = crate::skills::hooks::HookContext::for_post_compact(
                    &hook_stats,
                    "auto",
                    self.config.session_id.as_deref().unwrap_or(""),
                    &self.config.agent_name,
                );
                let registry = self.tools.hook_registry.read().await.clone();
                let post_result = registry.run_lifecycle_hooks(&hook_ctx).await;
                if let Some(ctx) = &post_result.injected_context {
                    self.memory
                        .context
                        .lock()
                        .await
                        .push(crate::llm::types::Message::system(format!(
                            "[Hook:PostCompact] {}",
                            ctx
                        )));
                }
                for msg in &post_result.messages {
                    self.memory
                        .context
                        .lock()
                        .await
                        .push(crate::llm::types::Message::system(msg.clone()));
                }
            }
        }

        // Inject any PreCompact hook messages into context
        if let Some(ctx) = &pre_compact_result.injected_context {
            self.memory
                .context
                .lock()
                .await
                .push(crate::llm::types::Message::system(ctx.clone()));
        }
        for msg in &pre_compact_result.messages {
            self.memory
                .context
                .lock()
                .await
                .push(crate::llm::types::Message::system(msg.clone()));
        }

        let messages = prepare_result.messages;

        for cb in &callbacks {
            cb.on_think_start(&agent, &messages).await;
        }

        // ── Intervention callbacks for think ──
        for intervention in &self.tools.intervention_callbacks {
            let result = intervention.on_think_start(&agent, &messages).await;
            if result.cancel {
                return Err(ReactError::Other(
                    "Agent execution cancelled by intervention at think".into(),
                ));
            }
            if result.block {
                let reason = result
                    .block_reason
                    .unwrap_or_else(|| "blocked by intervention at think".into());
                warn!(agent = %agent, reason = %reason, "Intervention blocked think");
                return Err(ReactError::Other(format!(
                    "Think blocked by intervention: {}", reason
                )));
            }
            if let Some(context) = result.injected_context {
                self.memory
                    .context
                    .lock()
                    .await
                    .push(Message::system(context));
            }
        }

        let tools = self.tools.tool_manager.get_openai_tools();

        // Circuit breaker check
        let circuit_breaker = self.guard.circuit_breaker.clone();
        if let Some(cb) = &circuit_breaker
            && cb.is_open()
        {
            warn!(agent = %agent, "🔴 Circuit breaker open, skip LLM request");
            // Fire StopFailure hook for circuit breaker
            let sf_result = self
                .fire_lifecycle_hook(
                    crate::skills::hooks::HookEvent::StopFailure,
                    Some("circuit_breaker_open"),
                )
                .await;
            if !sf_result.messages.is_empty() || sf_result.injected_context.is_some() {
                warn!(agent = %agent, "StopFailure hook (circuit_breaker) produced output that cannot be injected (terminal path)");
            }
            return Err(ReactError::Agent(Box::new(
                AgentError::InitializationFailed(
                    "LLM service unavailable (circuit breaker open)".to_string(),
                ),
            )));
        }

        let (message, usage, finish_reason) = self.call_llm_with_retry(&messages, tools).await?;

        let has_tool_calls = message.tool_calls.is_some();
        let tool_calls_count = message.tool_calls.as_ref().map_or(0, |tc| tc.len());
        let has_content = message.content.as_text_ref().is_some();
        let has_reasoning = message.reasoning_content.is_some();
        warn!(
            agent = %agent,
            has_tool_calls,
            tool_calls_count,
            has_content,
            has_reasoning,
            finish_reason = ?finish_reason,
            content_debug = ?message.content,
            reasoning_preview = ?message.reasoning_content.as_ref().map(|r| r.chars().take(200).collect::<String>()),
            "🔍 LLM response diagnostics"
        );

        if let Some(tool_calls) = &message.tool_calls
            && !tool_calls.is_empty()
        {
            self.memory.context.lock().await.push(message.clone());
            let tool_names: Vec<&str> = tool_calls
                .iter()
                .map(|c| c.function.name.as_str())
                .collect();
            info!(
                agent = %agent,
                tools = ?tool_names,
                "🧠 LLM decided to call {} tools",
                tool_calls.len()
            );
            for call in tool_calls {
                res.push(StepType::Call {
                    tool_call_id: call.id.clone(),
                    function_name: call.function.name.clone(),
                    arguments: serde_json::from_str(&call.function.arguments)?,
                });
            }
        } else if let Some(content) = message.content.as_text_ref() {
            self.memory.context.lock().await.push(message.clone());
            debug!(agent = %agent, "🧠 LLM returned text response");
            res.push(StepType::Thought(content.to_string()));
        } else if message.reasoning_content.is_some() || message.content.as_text_ref().is_none() {
            // Don't push to context: messages with empty content + no tool_calls sent to the API
            // cause "content field is required" errors; reasoning_content is the model's internal
            // thought process and doesn't need to be passed back to the next round.
            debug!(agent = %agent, "🧠 LLM returned only reasoning content or empty response, continue iterating");
        }

        let prompt_tokens = usage.as_ref().and_then(|u| u.prompt_tokens).unwrap_or(0);
        let completion_tokens = usage
            .as_ref()
            .and_then(|u| u.completion_tokens)
            .unwrap_or(0);

        // Record trace event
        self.record_trace_event(crate::trace::RunEvent::LlmCall {
            messages: messages.len(),
            prompt_tokens,
            completion_tokens,
            duration_ms: 0, // duration tracked by caller
        })
        .await;

        for cb in &callbacks {
            cb.on_think_end(
                &agent,
                &res,
                prompt_tokens as usize,
                completion_tokens as usize,
            )
            .await;
        }

        Ok(res)
    }

    /// Process steps produced by one think round:
    /// - Tool calls → execute in parallel (approval-required tools are serialized), return answer on `final_answer`
    /// - No tool calls → plain text response treated as final answer, returned directly
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

        // Execute non-approval tools concurrently
        let concurrent_results: Vec<
            std::result::Result<ToolExecutionOutcome, ToolExecutionFailure>,
        > = if concurrent_tools.is_empty() {
            Vec::new()
        } else {
            let futures: Vec<_> = concurrent_tools
                .iter()
                .map(|(_, name, args)| {
                    self.execute_tool_feedback_raw(name, args, self.config.tool_error_feedback)
                        .instrument(info_span!("tool_execute", tool.name = %name))
                })
                .collect();
            let batch_timeout = super::retry::compute_concurrent_tool_batch_timeout(
                &self.config.tool_execution,
                futures.len(),
                max_concurrency,
            );

            if let Some(timeout) = batch_timeout {
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
                            self.memory.context.lock().await.push(
                                crate::llm::types::Message::system(format!(
                                    "[Hook:PostToolBatch] {}",
                                    ctx
                                )),
                            );
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
            }
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
                    outcome.output
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
                    let result = intervention
                        .on_final_answer(&agent, &output)
                        .await;
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
                        self.memory
                            .context
                            .lock()
                            .await
                            .push(Message::system(context));
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
                self.memory
                    .context
                    .lock()
                    .await
                    .push(Message::system(format!("[Hook:PostToolBatch] {}", ctx)));
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

    /// Core ReAct loop (inject memories → append message → think/act iteration).
    /// `run_direct` and `run_chat_direct` share this implementation.
    #[tracing::instrument(skip(self, message), fields(agent = %self.config.agent_name, model = %self.config.model_name))]
    pub(crate) async fn run_react_loop(&self, message: &str) -> Result<String> {
        let agent = self.config.agent_name.clone();
        let callbacks = self.config.callbacks.clone();

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
                return Ok(format!("Request blocked by safety guard: {reason}"));
            }
        }

        self.log_user_input_audit(message).await;

        // Phase: Recall
        turn.advance(crate::agent::turn::TurnPhase::Recall);
        self.record_trace_event(crate::trace::RunEvent::PhaseTransition {
            phase: "recall".into(),
            iteration: 0,
        })
        .await;
        *self.current_turn.lock().unwrap_or_else(|e| e.into_inner()) = Some(turn.clone());

        // Fire UserPromptSubmit hook
        {
            let hook_ctx = crate::skills::hooks::HookContext::for_user_prompt_submit(
                message,
                None,
                self.config.session_id.as_deref().unwrap_or(""),
                &self.config.agent_name,
            );
            let registry = self.tools.hook_registry.read().await.clone();
            let prompt_result = registry.run_lifecycle_hooks(&hook_ctx).await;
            if prompt_result.block {
                return Ok(format!(
                    "Blocked by UserPromptSubmit hook: {}",
                    prompt_result.block_reason.unwrap_or_default()
                ));
            }
            if let Some(ctx) = &prompt_result.injected_context {
                self.memory
                    .context
                    .lock()
                    .await
                    .push(crate::llm::types::Message::system(ctx.clone()));
            }
            for msg in &prompt_result.messages {
                self.memory
                    .context
                    .lock()
                    .await
                    .push(crate::llm::types::Message::system(msg.clone()));
            }
        }

        match self.recall_long_term_memories(message).await {
            Ok(items) if !items.is_empty() => {
                debug!(agent = %agent, count = items.len(), "📚 Injecting relevant long-term memories");
                let mut lines = vec!["[Related historical memories]".to_string()];
                for (i, item) in items.iter().enumerate() {
                    let content_str = item
                        .value
                        .get("content")
                        .and_then(|v| v.as_str())
                        .map(String::from)
                        .unwrap_or_else(|| item.value.to_string());
                    lines.push(format!("{}. {}", i + 1, content_str));
                }
                lines.push("[Above memories are for reference, please answer based on the current question]".to_string());
                self.memory
                    .context
                    .lock()
                    .await
                    .push(Message::user(lines.join("\n")));
            }
            Ok(_) => {}
            Err(e) => {
                warn!(agent = %agent, error = %e, "⚠️ Long-term memory retrieval failed, skipping injection");
            }
        }

        self.memory
            .context
            .lock()
            .await
            .push(Message::user(message.to_string()));

        // Start trace run recording
        self.start_trace_run(message).await;

        /// Maximum times a Stop hook can request continuation before being ignored.
        const MAX_STOP_CONTINUE: usize = 3;
        let mut stop_continue_count = 0usize;

        let max_iters = self.config.max_iterations;
        let unlimited = max_iters == 0;

        let mut iteration = 0usize;
        loop {
            // Iteration limit check (0 = unlimited)
            if !unlimited && iteration >= max_iters {
                break;
            }

            // Cancel check: if a cancellation token is set and triggered, stop the loop
            if let Some(ref cancel) = *self.cancel_token.lock().await
                && cancel.is_cancelled()
            {
                info!(agent = %agent, "Cancellation requested, stopping ReAct loop");
                self.finalize_trace_run(
                    crate::trace::RunStatus::Cancelled,
                    None,
                    Some("Cancelled"),
                )
                .await;
                return Ok("Cancelled.".to_string());
            }

            info!(agent = %agent, iteration = iteration + 1, "🔄 ReAct iteration starting");

            for cb in &callbacks {
                cb.on_iteration(&agent, iteration).await;
            }

            debug!(agent = %agent, iteration = iteration + 1, "--- Iteration ---");

            // Phase: Think
            turn.advance(crate::agent::turn::TurnPhase::Think);
            self.record_trace_event(crate::trace::RunEvent::PhaseTransition {
                phase: "think".into(),
                iteration: iteration + 1,
            })
            .await;
            *self.current_turn.lock().unwrap_or_else(|e| e.into_inner()) = Some(turn.clone());

            let think_model = self.config.model_name.clone();
            let steps = self
                .think()
                .instrument(info_span!("llm_think", model = %think_model))
                .await?;
            if steps.is_empty() {
                warn!(
                    agent = %agent,
                    model = %think_model,
                    iteration = iteration + 1,
                    max_iterations = self.config.max_iterations,
                    "⚠️ LLM returned empty response, continue to next iteration"
                );
                continue;
            }

            // Phase: Act
            turn.advance(crate::agent::turn::TurnPhase::Act);
            turn.record_iteration();
            self.record_trace_event(crate::trace::RunEvent::PhaseTransition {
                phase: "act".into(),
                iteration: iteration + 1,
            })
            .await;
            *self.current_turn.lock().unwrap_or_else(|e| e.into_inner()) = Some(turn.clone());

            if let Some(mut answer) = self.process_steps(steps).await? {
                // Output guard check
                if let Some(gm) = &self.guard.guard_manager {
                    let result = gm.check_all(&answer, GuardDirection::Output).await?;
                    if let crate::guard::GuardResult::Block { reason } = &result {
                        info!(agent = %agent, reason = %reason, "🛡️ Output blocked by guard");
                        if let Some(al) = &self.guard.audit_logger {
                            let event = crate::audit::AuditEvent::now(
                                self.config.session_id.clone(),
                                agent.clone(),
                                crate::audit::AuditEventType::GuardBlock {
                                    guard: "guard_manager".to_string(),
                                    direction: GuardDirection::Output,
                                    reason: reason.clone(),
                                },
                            );
                            if let Err(e) = al.log(event).await {
                                tracing::warn!(error = %e, "Failed to log output guard audit event");
                            }
                        }
                        answer = format!("Response content filtered by safety guard: {reason}");
                    }
                }

                // Final snapshot
                self.auto_snapshot(iteration).await;

                for cb in &callbacks {
                    cb.on_final_answer(&agent, &answer).await;
                }
                info!(agent = %agent, "🏁 Execution complete");

                self.log_final_answer_audit(&answer).await;

                // Fire Stop hook — if it returns a continue_reason, inject it and keep going
                {
                    let hook_ctx = crate::skills::hooks::HookContext::for_stop(
                        None,
                        self.config.session_id.as_deref().unwrap_or(""),
                        &self.config.agent_name,
                        stop_continue_count > 0,
                    );
                    let registry = self.tools.hook_registry.read().await.clone();
                    let stop_result = registry.run_lifecycle_hooks(&hook_ctx).await;
                    // Block takes priority: suppress the final answer
                    if stop_result.block {
                        warn!(agent = %agent, reason = ?stop_result.block_reason, "Stop hook blocked final answer");
                        answer = stop_result
                            .block_reason
                            .unwrap_or_else(|| "Response blocked by Stop hook".to_string());
                    }
                    if let Some(reason) = &stop_result.continue_reason {
                        if stop_continue_count >= MAX_STOP_CONTINUE {
                            warn!(
                                agent = %agent,
                                count = stop_continue_count,
                                max = MAX_STOP_CONTINUE,
                                "Stop hook continue limit reached, ignoring continuation request"
                            );
                        } else {
                            info!(agent = %agent, reason = %reason, "Stop hook requested continuation");
                            self.memory.context.lock().await.push(
                                crate::llm::types::Message::system(format!(
                                    "[Hook:Stop] Continue: {}",
                                    reason
                                )),
                            );
                            stop_continue_count += 1;
                            self.auto_snapshot(iteration).await;
                            continue;
                        }
                    }
                    // Inject any hook messages
                    for msg in &stop_result.messages {
                        self.memory
                            .context
                            .lock()
                            .await
                            .push(crate::llm::types::Message::system(msg.clone()));
                    }
                }

                self.persist_runtime_state().await;

                // Phase: Finalize
                turn.advance(crate::agent::turn::TurnPhase::Finalize);
                self.record_trace_event(crate::trace::RunEvent::PhaseTransition {
                    phase: "finalize".into(),
                    iteration: iteration + 1,
                })
                .await;
                *self.current_turn.lock().unwrap_or_else(|e| e.into_inner()) = Some(turn.clone());

                // Finalize trace run as completed
                self.finalize_trace_run(crate::trace::RunStatus::Completed, Some(&answer), None)
                    .await;

                return Ok(answer);
            }

            // Intermediate iteration snapshot (final answer not yet produced)
            self.auto_snapshot(iteration).await;

            iteration += 1;
        }

        warn!(agent = %agent, max = self.config.max_iterations, "Maximum iterations reached");

        // Auto-save checkpoint so the conversation can be resumed
        if let Some(checkpointer) = self.checkpointer()
            && let Some(ref session_id) = self.config.session_id
        {
            // Best-effort checkpoint save — don't fail the run if this errors
            if let Ok(Some(state)) = checkpointer.get_state(session_id).await {
                if let Err(e) = checkpointer.put_state(session_id, state).await {
                    tracing::warn!(error = %e, session_id = %session_id, "Failed to save checkpoint state");
                }
            }
            info!(agent = %agent, session = %session_id, "Saved checkpoint on max_iterations exceeded");
        }

        // Fire StopFailure hook before returning error
        {
            let hook_ctx = crate::skills::hooks::HookContext::for_stop_failure(
                "MaxIterationsExceeded",
                "max_iterations",
                self.config.session_id.as_deref().unwrap_or(""),
                &self.config.agent_name,
            );
            let registry = self.tools.hook_registry.read().await.clone();
            let stop_failure_result = registry.run_lifecycle_hooks(&hook_ctx).await;
            // StopFailure result cannot change the terminal outcome, but log any
            // messages or injected_context for audit visibility.
            if !stop_failure_result.messages.is_empty()
                || stop_failure_result.injected_context.is_some()
            {
                warn!(agent = %agent, "StopFailure hook produced output that cannot be injected (terminal path)");
            }
        }

        // Finalize trace run as failed
        self.finalize_trace_run(
            crate::trace::RunStatus::Failed,
            None,
            Some("Max iterations exceeded"),
        )
        .await;

        Err(ReactError::from(AgentError::MaxIterationsExceeded(
            self.config.max_iterations,
        )))
    }
}
