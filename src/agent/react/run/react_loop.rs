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
        }

        let messages = prepare_result.messages;

        for cb in &callbacks {
            cb.on_think_start(&agent, &messages).await;
        }

        let tools = self.tools.tool_manager.get_openai_tools();
        let max_retries = self.config.llm_max_retries;
        let retry_delay = self.config.llm_retry_delay_ms;
        let client = self.client.clone();
        let model_name = self.config.model_name.clone();
        let response_format = self.config.response_format.clone();
        let temperature = self.config.temperature;
        let max_tokens = self.config.max_tokens;

        // Circuit breaker check
        let circuit_breaker = self.guard.circuit_breaker.clone();
        if let Some(cb) = &circuit_breaker
            && cb.is_open()
        {
            warn!(agent = %agent, "🔴 Circuit breaker open, skip LLM request");
            return Err(ReactError::Agent(AgentError::InitializationFailed(
                "LLM service unavailable (circuit breaker open)".to_string(),
            )));
        }

        let (message, usage, finish_reason) = if let Some(llm_client) = self.llm_client.clone() {
            let request = ChatRequest {
                messages: messages.clone(),
                temperature: self.config.temperature,
                max_tokens: self.config.max_tokens,
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
                &agent,
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
            let finish_reason = response.finish_reason.clone();
            (response.message, usage, finish_reason)
        } else {
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
                &agent,
                max_retries,
                retry_delay,
                &circuit_breaker,
                || {
                    let client = client.clone();
                    let model_name = model_name.as_str();
                    let messages = &messages;
                    let tools = tools.clone();
                    let response_format = response_format.clone();
                    async move {
                        chat(
                            client,
                            model_name,
                            messages,
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
            let choice =
                response
                    .choices
                    .first()
                    .ok_or(ReactError::Agent(AgentError::NoResponse {
                        model: self.config.model_name.clone(),
                        agent: self.config.agent_name.clone(),
                    }))?;
            let finish_reason = choice.finish_reason.clone();
            let message = choice.message.clone();
            warn!(
                agent = %agent,
                finish_reason = ?finish_reason,
                has_tool_calls = message.tool_calls.as_ref().is_some_and(|t| !t.is_empty()),
                content_preview = ?message.content.as_text().map(|c| c.chars().take(200).collect::<String>()),
                "📥 LLM response"
            );
            (message, usage, finish_reason)
        };

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

        let prompt_tokens = usage.as_ref().and_then(|u| u.prompt_tokens).unwrap_or(0) as usize;
        let completion_tokens = usage
            .as_ref()
            .and_then(|u| u.completion_tokens)
            .unwrap_or(0) as usize;
        for cb in &callbacks {
            cb.on_think_end(&agent, &res, prompt_tokens, completion_tokens)
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
        for ((tool_call_id, function_name, _), result) in
            concurrent_tools.into_iter().zip(concurrent_results)
        {
            let result = match result {
                Ok(outcome) => {
                    self.apply_hook_messages(&function_name, &outcome.hook_messages)
                        .await;
                    outcome.output
                }
                Err(failure) => {
                    self.apply_hook_messages(&function_name, &failure.hook_messages)
                        .await;
                    return Err(failure.error);
                }
            };
            self.memory.context.lock().await.push(Message::tool_result(
                tool_call_id,
                function_name.clone(),
                result.clone(),
            ));
            if function_name == TOOL_FINAL_ANSWER {
                info!(agent = %agent, "🏁 Final answer generated");
                final_answer = Some(result);
            }
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
                    let _ = al.log(event).await;
                }
                return Ok(format!("Request blocked by safety guard: {reason}"));
            }
        }

        self.log_user_input_audit(message).await;

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

        for iteration in 0..self.config.max_iterations {
            info!(agent = %agent, iteration = iteration + 1, "🔄 ReAct iteration starting");

            for cb in &callbacks {
                cb.on_iteration(&agent, iteration).await;
            }

            debug!(agent = %agent, iteration = iteration + 1, "--- Iteration ---");

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
                            let _ = al.log(event).await;
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
                self.persist_runtime_state().await;

                return Ok(answer);
            }

            // Intermediate iteration snapshot (final answer not yet produced)
            self.auto_snapshot(iteration).await;
        }

        warn!(agent = %agent, max = self.config.max_iterations, "Maximum iterations reached");
        Err(ReactError::from(AgentError::MaxIterationsExceeded(
            self.config.max_iterations,
        )))
    }
}
