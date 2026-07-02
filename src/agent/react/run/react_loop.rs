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

    /// Call LLM for reasoning, returning the list of steps for this round.
    ///
    /// Before each call, `ContextManager::prepare` auto-compresses overflow history messages,
    /// then the compressed message list is passed to the LLM; the LLM response is appended back to context.
    #[allow(dead_code)]
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
                    super::context::push_runtime_context_note(
                        &self.memory.context,
                        "Hook:PostCompact",
                        ctx,
                    )
                    .await;
                }
                for msg in &post_result.messages {
                    super::context::push_runtime_context_note(
                        &self.memory.context,
                        "Hook:PostCompact",
                        msg,
                    )
                    .await;
                }
            }
        }

        // Inject any PreCompact hook messages into context
        if let Some(ctx) = &pre_compact_result.injected_context {
            super::context::push_runtime_context_note(&self.memory.context, "Hook:PreCompact", ctx)
                .await;
        }
        for msg in &pre_compact_result.messages {
            super::context::push_runtime_context_note(&self.memory.context, "Hook:PreCompact", msg)
                .await;
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
                    "Think blocked by intervention: {}",
                    reason
                )));
            }
            if let Some(context) = result.injected_context {
                super::context::push_runtime_context_note(
                    &self.memory.context,
                    "Intervention:ThinkStart",
                    &context,
                )
                .await;
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

        // Record usage in the token tracker for cumulative tracking
        if let Some(ref u) = usage {
            self.token_tracker.record_usage(u);
        }
        let cached_prompt_tokens = usage
            .as_ref()
            .map(|u| u.cached_prompt_tokens())
            .unwrap_or(0);
        let cache_creation_prompt_tokens = usage
            .as_ref()
            .map(|u| u.cache_creation_prompt_tokens())
            .unwrap_or(0);
        let total_tokens = usage
            .as_ref()
            .and_then(|u| u.total_tokens)
            .unwrap_or_else(|| prompt_tokens.saturating_add(completion_tokens));
        tracing::debug!(
            target: "echo_agent::llm_usage",
            agent = %agent,
            model = %self.config.model_name,
            prompt_tokens = prompt_tokens,
            completion_tokens = completion_tokens,
            total_tokens = total_tokens,
            cached_prompt_tokens = cached_prompt_tokens,
            cache_creation_prompt_tokens = cache_creation_prompt_tokens,
            usage_reported = usage.is_some(),
            "LLM usage recorded"
        );

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
        if let Some(runtime_context) = super::context::format_turn_runtime_context(
            memory_context.as_deref(),
            ws_block.as_str(),
        ) {
            context.push(super::context::runtime_context_note(
                "turn",
                &runtime_context,
            ));
        }
        context.push(Message::user(message.to_string()));

        // Start trace run recording
        self.start_trace_run(message).await;

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

        // ★ NEW: Intent routing
        if let Some(ref router) = self.intent_router {
            let messages = self.memory.context.lock().await.messages().to_vec();
            let intent = router.classify(message, &messages).await;
            match intent {
                crate::intent::Intent::DirectAnswer { confidence } => {
                    tracing::info!(
                        agent = %self.config.agent_name,
                        confidence = confidence,
                        "🎯 IntentRouter: DirectAnswer shortcut"
                    );
                    return self.direct_answer(message).await;
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
                    if let Err(e) = self.activate_skill_for_context(&skill_name).await {
                        tracing::warn!(skill = %skill_name, error = %e, "IntentRouter: failed to activate skill");
                    }
                }
                crate::intent::Intent::WorkflowRequired {
                    workflow_name,
                    confidence,
                } => {
                    tracing::info!(
                        agent = %self.config.agent_name,
                        workflow = %workflow_name,
                        confidence = confidence,
                        "🎯 IntentRouter: WorkflowRequired (fallback to ReAct for now)"
                    );
                    // TODO: execute workflow before entering ReAct
                }
                crate::intent::Intent::Fallback => {
                    tracing::debug!(agent = %self.config.agent_name, "IntentRouter: Fallback to ReAct");
                }
            }
        }

        // Create channel + snapshot
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

        let (response, _usage, _finish_reason) =
            self.call_llm_with_retry(&messages, vec![]).await?;
        let content = response.content.as_text().unwrap_or_default().to_string();

        // Record trace
        self.record_trace_event(crate::trace::RunEvent::LlmCall {
            messages: messages.len(),
            prompt_tokens: 0,
            completion_tokens: 0,
            duration_ms: 0,
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
