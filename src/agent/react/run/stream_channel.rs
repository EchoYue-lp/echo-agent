//! Channel-based streaming execution — returns `BoxStream<'static>`.
//!
//! Uses a tokio::mpsc channel + spawned task instead of `try_stream!`.
//! This is the primary streaming implementation; `run_stream_inner` (try_stream!)
//! is kept as a fallback for non-static borrow scenarios.

use super::super::{ReactAgent, StepType, TOOL_FINAL_ANSWER};
use super::stream_loop::processor::{build_tool_calls_from_map, process_stream_chunk};
use super::types::{StreamInit, StreamMode};
use crate::agent::AgentEvent;
use crate::error::{AgentError, ReactError, Result};
use crate::llm::types::Message;
use crate::tools::ToolParameters;
use futures::StreamExt;
use futures::future::join_all;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{Instrument, debug, info, info_span};

// Default stream buffer capacity
const DEFAULT_STREAM_BUFFER: usize = 256;

macro_rules! yield_event {
    ($tx:expr, $event:expr) => {
        match $tx.try_send(Ok($event)) {
            Ok(()) => {}
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                // Channel full — drop event and log
                tracing::warn!("Stream buffer full ({}), dropping event", DEFAULT_STREAM_BUFFER);
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                return Ok(());
            }
        }
    };
}
macro_rules! try_send {
    ($tx:expr, $fallible:expr) => {
        match $fallible {
            Ok(v) => v,
            Err(e) => {
                let _ = $tx.try_send(Err(e.into()));
                return Ok(());
            }
        }
    };
}

// ── ReactAgent: entry point ──────────────────────────────────────────

impl ReactAgent {
    /// Channel-based streaming entry point. Returns `BoxStream<'static>`.
    pub(crate) async fn run_stream_channel(
        &self,
        init: StreamInit,
        mode: StreamMode,
    ) -> Result<futures::stream::BoxStream<'static, Result<AgentEvent>>> {
        let (tx, rx) = mpsc::channel::<Result<AgentEvent>>(self.config.stream_buffer_size);
        let context = self.memory.context.clone();
        let text = init.text.clone();
        let message = init.message.clone();
        let label = init.label.clone();

        let recalled = if let Some(ref msg) = init.message {
            self.prepare_stream_context_with_message(mode, msg).await
        } else {
            self.prepare_stream_context(mode, &init.text).await
        };

        // Start trace run for streaming path
        self.start_trace_run(&text).await;

        let mut snap = make_snapshot(self);
        // Pass current run_id from the agent to the snapshot
        snap.current_run_id = self.current_run_id.lock().unwrap().clone();

        tokio::spawn(async move {
            if let Err(e) = snap
                .run_loop(context, text, message, label, mode, recalled, tx.clone())
                .await
            {
                let _ = tx.try_send(Err(e));
            }
        });

        Ok(Box::pin(
            tokio_stream::wrappers::ReceiverStream::new(rx),
        ))
    }
}

// ── AgentSnapshot ────────────────────────────────────────────────────

// AgentSnapshot is now AgentRunSnapshot from crate::agent::snapshot
use crate::agent::snapshot::AgentRunSnapshot as AgentSnapshot;

// Helper to create snapshot from agent (keeps the same API for rest of file)
fn make_snapshot(agent: &ReactAgent) -> AgentSnapshot {
    AgentSnapshot::from_agent(agent)
}

impl AgentSnapshot {
    // ── Main loop ────────────────────────────────────────────────────

    async fn run_loop(
        self,
        context: Arc<tokio::sync::Mutex<crate::compression::ContextManager>>,
        text: String,
        _message: Option<Message>,
        label: String,
        mode: StreamMode,
        recalled: usize,
        tx: mpsc::Sender<Result<AgentEvent>>,
    ) -> Result<()> {
        let agent = self.config.agent_name.clone();
        let callbacks = self.config.callbacks.clone();

        match mode {
            StreamMode::Execute => info!(agent = %agent, "Agent streaming task execution{label}"),
            StreamMode::Chat => info!(agent = %agent, "Agent streaming conversation{label}"),
        }
        if recalled > 0 {
            yield_event!(tx, AgentEvent::MemoryRecalled { count: recalled });
        }

        // Audit: user input
        if let Some(al) = &self.guard.audit_logger {
            let event = crate::audit::AuditEvent::now(
                self.config.session_id.clone(),
                self.config.agent_name.clone(),
                crate::audit::AuditEventType::UserInput {
                    content: text.clone(),
                },
            );
            let _ = al.log(event).await;
        }

        // UserPromptSubmit hook
        {
            let hook_ctx = crate::skills::hooks::HookContext::for_user_prompt_submit(
                &text,
                None,
                self.config.session_id.as_deref().unwrap_or(""),
                &self.config.agent_name,
            );
            let registry = self.tools.hook_registry.read().await.clone();
            let result = registry.run_lifecycle_hooks(&hook_ctx).await;
            if result.block {
                yield_event!(
                    tx,
                    AgentEvent::FinalAnswer(format!(
                        "Blocked by UserPromptSubmit hook: {}",
                        result.block_reason.unwrap_or_default()
                    ))
                );
                self.fire_hook(crate::skills::hooks::HookEvent::SessionEnd, Some("blocked"))
                    .await;
                return Ok(());
            }
            if let Some(ctx) = &result.injected_context {
                context.lock().await.push(Message::system(ctx.clone()));
            }
            for msg in &result.messages {
                context.lock().await.push(Message::system(msg.clone()));
            }
        }

        let mut stop_hook_continued = false;

        for iteration in 0..self.config.max_iterations {
            for cb in &callbacks {
                cb.on_iteration(&agent, iteration).await;
            }
            debug!(agent = %agent, iteration = iteration + 1, "--- Streaming iteration{label} ---");

            self.fire_hook(crate::skills::hooks::HookEvent::PreCompact, Some("auto"))
                .await;
            let prepare_result = try_send!(tx, context.lock().await.prepare(None).await);

            if let Some(ref stats) = prepare_result.compressed {
                yield_event!(
                    tx,
                    AgentEvent::ContextCompressed {
                        before_count: stats.before_count,
                        after_count: stats.after_count,
                        before_tokens: stats.before_tokens,
                        after_tokens: stats.after_tokens,
                    }
                );
                let hs = crate::skills::hooks::CompressHookStats {
                    before_count: stats.before_count,
                    after_count: stats.after_count,
                    before_tokens: stats.before_tokens,
                    after_tokens: stats.after_tokens,
                };
                let hc = crate::skills::hooks::HookContext::for_post_compact(
                    &hs,
                    "auto",
                    self.config.session_id.as_deref().unwrap_or(""),
                    &self.config.agent_name,
                );
                let reg = self.tools.hook_registry.read().await.clone();
                let r = reg.run_lifecycle_hooks(&hc).await;
                if let Some(c) = &r.injected_context {
                    context
                        .lock()
                        .await
                        .push(Message::system(format!("[Hook:PostCompact] {}", c)));
                }
                for m in &r.messages {
                    context.lock().await.push(Message::system(m.clone()));
                }
            }

            let messages = prepare_result.messages;
            for cb in &callbacks {
                cb.on_think_start(&agent, &messages).await;
            }

            let mut llm_stream = Box::pin(try_send!(
                tx,
                self.create_llm_stream(messages.clone()).await
            ));
            let mut content_buffer = String::new();
            let mut tool_call_map: HashMap<u32, (String, String, String)> = HashMap::new();
            let mut last_usage = None;
            let mut in_reasoning = false;

            while let Some(cr) = llm_stream.next().await {
                let chunk = try_send!(tx, cr);
                if chunk.usage.is_some() {
                    last_usage = chunk.usage.clone();
                }
                for event in process_stream_chunk(
                    &chunk,
                    &mut content_buffer,
                    &mut tool_call_map,
                    &mut in_reasoning,
                ) {
                    yield_event!(tx, event);
                }
            }

            let pt = last_usage
                .as_ref()
                .and_then(|u| u.prompt_tokens)
                .unwrap_or(0) as usize;
            let ct = last_usage
                .as_ref()
                .and_then(|u| u.completion_tokens)
                .unwrap_or(0) as usize;
            if in_reasoning {
                yield_event!(
                    tx,
                    AgentEvent::ThinkEnd {
                        prompt_tokens: pt,
                        completion_tokens: ct
                    }
                );
            }

            if !tool_call_map.is_empty() {
                let (msg_tc, steps) = build_tool_calls_from_map(&tool_call_map);
                for (_, name, args) in &steps {
                    yield_event!(
                        tx,
                        AgentEvent::ToolCall {
                            name: name.clone(),
                            args: args.clone()
                        }
                    );
                }
                {
                    let ts: Vec<StepType> = steps
                        .iter()
                        .map(|(id, n, a)| StepType::Call {
                            tool_call_id: id.clone(),
                            function_name: n.clone(),
                            arguments: a.clone(),
                        })
                        .collect();
                    for cb in &callbacks {
                        cb.on_think_end(&agent, &ts, pt, ct).await;
                    }
                }
                context
                    .lock()
                    .await
                    .push(Message::assistant_with_tools(msg_tc));

                #[cfg(feature = "human-loop")]
                let (appr, conc) = {
                    let mut a = vec![];
                    let mut c = vec![];
                    for s in steps {
                        if self.tool_needs_approval(&s.1).await {
                            a.push(s);
                        } else {
                            c.push(s);
                        }
                    }
                    (a, c)
                };
                #[cfg(not(feature = "human-loop"))]
                let (appr, conc): (
                    Vec<(String, String, Value)>,
                    Vec<(String, String, Value)>,
                ) = (vec![], steps);

                if !conc.is_empty() {
                    let mc = self.tools.tool_manager.max_concurrency();
                    let snapshot = self.clone();
                    let futs: Vec<_> = conc
                        .iter()
                        .map(|(_, n, a)| {
                            let snapshot = snapshot.clone();
                            let name = n.clone();
                            let args = a.clone();
                            async move {
                                let params = if let Value::Object(m) = &args {
                                    m.clone().into_iter().collect()
                                } else {
                                    HashMap::new()
                                };
                                snapshot
                                    .execute_tool_with_policy(&name, &params, &args)
                                    .await
                            }
                            .instrument(info_span!("tool", tool.name = %n))
                        })
                        .collect();
                    let bt = super::retry::compute_concurrent_tool_batch_timeout(
                        &self.config.tool_execution,
                        futs.len(),
                        mc,
                    );
                    let results: Vec<std::result::Result<String, ReactError>>;
                    if let Some(to) = bt {
                        results = try_send!(
                            tx,
                            tokio::time::timeout(to, join_all(futs)).await.map_err(|_| {
                                ReactError::from(crate::error::ToolError::Timeout(
                                    "batch timeout".into(),
                                ))
                            })
                        );
                    } else {
                        results = join_all(futs).await;
                    }

                    for ((id, fname, _), result) in conc.into_iter().zip(results) {
                        match result {
                            Ok(output) => {
                                yield_event!(
                                    tx,
                                    AgentEvent::ToolResult {
                                        name: fname.clone(),
                                        output: output.clone()
                                    }
                                );
                                context.lock().await.push(Message::tool_result(
                                    id,
                                    fname.clone(),
                                    output.clone(),
                                ));
                                if fname == TOOL_FINAL_ANSWER {
                                    return self
                                        .finish(
                                            context,
                                            agent,
                                            callbacks,
                                            label,
                                            &output,
                                            iteration,
                                            stop_hook_continued,
                                            tx,
                                        )
                                        .await;
                                }
                            }
                            Err(error) => {
                                yield_event!(
                                    tx,
                                    AgentEvent::ToolError {
                                        name: fname.clone(),
                                        error: error.to_string()
                                    }
                                );
                                context.lock().await.push(Message::tool_result(
                                    id,
                                    fname.clone(),
                                    format!("[Error] {error}"),
                                ));
                            }
                        }
                    }
                }

                for (id, fname, args) in appr {
                    let params = if let Value::Object(m) = &args {
                        m.clone().into_iter().collect()
                    } else {
                        HashMap::new()
                    };
                    match self.execute_tool_with_policy(&fname, &params, &args).await {
                        Ok(truncated) => {
                            yield_event!(
                                tx,
                                AgentEvent::ToolResult {
                                    name: fname.clone(),
                                    output: truncated.clone()
                                }
                            );
                            context.lock().await.push(Message::tool_result(
                                id,
                                fname.clone(),
                                truncated.clone(),
                            ));
                            if fname == TOOL_FINAL_ANSWER {
                                return self
                                    .finish(
                                        context,
                                        agent,
                                        callbacks,
                                        label,
                                        &truncated,
                                        iteration,
                                        stop_hook_continued,
                                        tx,
                                    )
                                    .await;
                            }
                        }
                        Err(error) => {
                            yield_event!(
                                tx,
                                AgentEvent::ToolError {
                                    name: fname.clone(),
                                    error: error.to_string()
                                }
                            );
                            context.lock().await.push(Message::tool_result(
                                id,
                                fname.clone(),
                                format!("[Error] {error}"),
                            ));
                        }
                    }
                }
                self.auto_snapshot(&context, iteration).await;
            } else if !content_buffer.is_empty() {
                let ts = vec![StepType::Thought(content_buffer.clone())];
                for cb in &callbacks {
                    cb.on_think_end(&agent, &ts, pt, ct).await;
                    cb.on_final_answer(&agent, &content_buffer).await;
                }
                context
                    .lock()
                    .await
                    .push(Message::assistant(content_buffer.clone()));
                self.auto_snapshot(&context, iteration).await;
                if let Some(al) = &self.guard.audit_logger {
                    let ev = crate::audit::AuditEvent::now(
                        self.config.session_id.clone(),
                        self.config.agent_name.clone(),
                        crate::audit::AuditEventType::FinalAnswer {
                            content: content_buffer.clone(),
                        },
                    );
                    let _ = al.log(ev).await;
                }
                self.save_checkpoint(&context).await;
                // Finalize trace before moving content_buffer into the event
                self.finalize_run(
                    crate::trace::RunStatus::Completed,
                    Some(&content_buffer),
                    None,
                )
                .await;
                yield_event!(tx, AgentEvent::FinalAnswer(content_buffer));
                let hc = crate::skills::hooks::HookContext::for_stop(
                    None,
                    self.config.session_id.as_deref().unwrap_or(""),
                    &self.config.agent_name,
                    stop_hook_continued,
                );
                let reg = self.tools.hook_registry.read().await.clone();
                let sr = reg.run_lifecycle_hooks(&hc).await;
                if let Some(reason) = &sr.continue_reason {
                    if !stop_hook_continued {
                        context
                            .lock()
                            .await
                            .push(Message::system(format!("[Hook:Stop] Continue: {}", reason)));
                        stop_hook_continued = true;
                        continue;
                    }
                }
                self.fire_hook(
                    crate::skills::hooks::HookEvent::SessionEnd,
                    Some("complete"),
                )
                .await;
                return Ok(());
            } else {
                self.finalize_run(
                    crate::trace::RunStatus::Failed,
                    None,
                    Some("No response from LLM"),
                )
                .await;
                let _ = tx.try_send(Err(ReactError::Agent(Box::new(AgentError::NoResponse {
                    model: self.config.model_name.clone(),
                    agent: self.config.agent_name.clone(),
                }))));
                return Ok(());
            }
        }

        self.fire_hook(
            crate::skills::hooks::HookEvent::SessionEnd,
            Some("max_iterations"),
        )
        .await;
        self.fire_hook(
            crate::skills::hooks::HookEvent::StopFailure,
            Some("max_iterations"),
        )
        .await;
        self.finalize_run(
            crate::trace::RunStatus::Failed,
            None,
            Some("Max iterations exceeded"),
        )
        .await;
        let _ = tx.try_send(Err(ReactError::Agent(Box::new(
            AgentError::MaxIterationsExceeded(self.config.max_iterations),
        ))));
        Ok(())
    }

    // ── Helpers ──────────────────────────────────────────────────────

    async fn create_llm_stream(
        &self,
        messages: Vec<Message>,
    ) -> Result<impl futures::Stream<Item = Result<crate::llm::types::ChatCompletionChunk>>> {
        let tools = if self.config.enable_tool {
            let t = self.tools.tool_manager.get_openai_tools();
            if t.is_empty() { None } else { Some(t) }
        } else {
            None
        };
        let cancel = self.cancel_token.clone();
        super::retry::retry_llm_call(
            &self.config.agent_name,
            self.config.llm_max_retries,
            self.config.llm_retry_delay_ms,
            &self.guard.circuit_breaker,
            || {
                let c = self.client.clone();
                let m = self.config.model_name.clone();
                let ms = messages.clone();
                let t = tools.clone();
                let ct = cancel.clone();
                async move {
                    crate::llm::stream_chat(
                        c,
                        &m,
                        ms,
                        self.config.temperature,
                        self.config.max_tokens,
                        t,
                        None,
                        None,
                        ct,
                    )
                    .await
                }
            },
        )
        .await
    }

    async fn truncate_output(&self, output: String) -> String {
        let Some(mt) = self.config.max_tool_output_tokens else {
            return output;
        };
        if output.chars().count() / 3 <= mt {
            return output;
        }
        let ratio = mt as f64 / (output.chars().count() as f64 / 3.0);
        format!(
            "{}\n[Output truncated]",
            output
                .chars()
                .take((output.len() as f64 * ratio * 0.95) as usize)
                .collect::<String>()
        )
    }

    async fn fire_hook(&self, event: crate::skills::hooks::HookEvent, matcher: Option<&str>) {
        let sid = self.config.session_id.clone().unwrap_or_default();
        let hc = match event {
            crate::skills::hooks::HookEvent::SessionEnd => {
                crate::skills::hooks::HookContext::for_session_end(
                    matcher.unwrap_or("other"),
                    &sid,
                    &self.config.agent_name,
                )
            }
            crate::skills::hooks::HookEvent::PreCompact => {
                crate::skills::hooks::HookContext::for_pre_compact(
                    &Default::default(),
                    matcher.unwrap_or("auto"),
                    &sid,
                    &self.config.agent_name,
                )
            }
            crate::skills::hooks::HookEvent::StopFailure => {
                crate::skills::hooks::HookContext::for_stop_failure(
                    "",
                    matcher.unwrap_or(""),
                    &sid,
                    &self.config.agent_name,
                )
            }
            _ => return,
        };
        let reg = self.tools.hook_registry.read().await.clone();
        let _ = reg.run_lifecycle_hooks(&hc).await;
    }

    async fn auto_snapshot(
        &self,
        context: &Arc<tokio::sync::Mutex<crate::compression::ContextManager>>,
        iteration: usize,
    ) {
        let should_capture = {
            let mgr = self.snapshot_manager.read().unwrap();
            mgr.as_ref().is_some_and(|m| m.should_capture(iteration))
            // RwLockReadGuard dropped here — before any await
        };
        if should_capture {
            let ctx = context.lock().await;
            let ms = ctx.messages().to_vec();
            drop(ctx);
            if let Some(ref mut m) = *self.snapshot_manager.write().unwrap() {
                m.capture(iteration, &ms);
            }
        }
    }

    async fn save_checkpoint(
        &self,
        context: &Arc<tokio::sync::Mutex<crate::compression::ContextManager>>,
    ) {
        if let (Some(cp), Some(sid)) = (&self.checkpointer, &self.config.session_id) {
            let ctx = context.lock().await;
            let state = crate::memory::ThreadState {
                messages: ctx.messages().to_vec(),
                summary: None,
                metadata: None,
            };
            drop(ctx);
            let _ = cp.put_state(sid, state).await;
        }
    }

    #[cfg(feature = "human-loop")]
    async fn tool_needs_approval(&self, tool_name: &str) -> bool {
        use crate::tools::permission::{PermissionDecision, PermissionMode};
        if let Some(svc) = &self.permission_service {
            let mode = svc.mode().await;
            if matches!(
                mode,
                PermissionMode::BypassPermissions | PermissionMode::DontAsk | PermissionMode::Plan
            ) {
                return false;
            }
            let perms = self
                .tools
                .tool_manager
                .get_tool(tool_name)
                .map(|t| t.permissions())
                .unwrap_or_default();
            return svc
                .check_with_permissions(tool_name, &serde_json::json!({}), &perms)
                .await
                .unwrap_or(PermissionDecision::RequireApproval)
                .requires_approval();
        }
        if let Some(pol) = &self.guard.permission_policy {
            let perms = self
                .tools
                .tool_manager
                .get_tool(tool_name)
                .map(|t| t.permissions())
                .unwrap_or_default();
            if !perms.is_empty() {
                let d = pol.check(tool_name, &perms).await;
                return matches!(
                    d,
                    PermissionDecision::RequireApproval | PermissionDecision::Ask { .. }
                );
            }
        }
        false
    }
    #[cfg(not(feature = "human-loop"))]
    async fn tool_needs_approval(&self, _: &str) -> bool {
        false
    }

    #[allow(clippy::too_many_arguments)]
    async fn finish(
        &self,
        context: Arc<tokio::sync::Mutex<crate::compression::ContextManager>>,
        agent: String,
        callbacks: Vec<Arc<dyn crate::agent::AgentCallback>>,
        label: String,
        output: &str,
        _iteration: usize,
        stop_hook_continued: bool,
        tx: mpsc::Sender<Result<AgentEvent>>,
    ) -> Result<()> {
        for cb in &callbacks {
            cb.on_final_answer(&agent, output).await;
        }
        info!(agent = %agent, "Streaming execution completed{label}");
        if let Some(al) = &self.guard.audit_logger {
            let ev = crate::audit::AuditEvent::now(
                self.config.session_id.clone(),
                self.config.agent_name.clone(),
                crate::audit::AuditEventType::FinalAnswer {
                    content: output.to_string(),
                },
            );
            let _ = al.log(ev).await;
        }
        self.save_checkpoint(&context).await;
        yield_event!(tx, AgentEvent::FinalAnswer(output.to_string()));
        let hc = crate::skills::hooks::HookContext::for_stop(
            None,
            self.config.session_id.as_deref().unwrap_or(""),
            &self.config.agent_name,
            stop_hook_continued,
        );
        let reg = self.tools.hook_registry.read().await.clone();
        let sr = reg.run_lifecycle_hooks(&hc).await;
        if let Some(reason) = &sr.continue_reason {
            if !stop_hook_continued {
                context
                    .lock()
                    .await
                    .push(Message::system(format!("[Hook:Stop] Continue: {}", reason)));
            }
        }
        self.fire_hook(
            crate::skills::hooks::HookEvent::SessionEnd,
            Some("complete"),
        )
        .await;
        Ok(())
    }

    /// Execute a single tool call with the full policy pipeline:
    /// PreToolUse hooks → read-before-edit guard → execute → PostToolUse hooks → audit.
    ///
    /// Mirrors `ReactAgent::execute_tool_feedback_raw` but uses the
    /// snapshot's owned state so it can run inside a `'static` future.
    async fn execute_tool_with_policy(
        &self,
        tool_name: &str,
        params: &ToolParameters,
        input: &Value,
    ) -> std::result::Result<String, crate::error::ReactError> {
        // ── PreToolUse hooks ──
        let hook_reg = self.tools.hook_registry.read().await.clone();
        let pre_result = hook_reg
            .run_pre_tool_use(tool_name, input, self.config.session_id.as_deref().unwrap_or(""))
            .await;
        if pre_result.block {
            let reason = pre_result
                .block_reason
                .unwrap_or_else(|| "blocked by skill hook".into());
            return Ok(format!("Tool {} blocked by hook: {}", tool_name, reason));
        }

        let mut effective_params = params.clone();
        let mut effective_input = input.clone();
        if let Some(updated) = pre_result.updated_input
            && let Value::Object(map) = &updated
        {
            effective_params = map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            effective_input = updated;
        }

        self.validate_read_before_edit(tool_name, &effective_params)?;
        // ── Business audit: tool start ──
        if let Some(al) = &self.guard.audit_logger {
            let ev = crate::audit::AuditEvent::now(
                self.config.session_id.clone(),
                self.config.agent_name.clone(),
                crate::audit::AuditEventType::ToolCall {
                    tool: tool_name.to_string(),
                    input: effective_input.clone(),
                    output: String::new(),
                    success: true,
                    duration_ms: 0,
                },
            );
            let _ = al.log(ev).await;
        }

        // ── Execute ──
        let call_id = format!("call_{}", uuid::Uuid::new_v4());
        // Redact secrets from args before recording
        let safe_args = Some(serde_json::from_str(
            &crate::security::redact_secrets(&serde_json::to_string(&effective_input).unwrap_or_default())
        ).unwrap_or(effective_input.clone()));
        // Record ToolCall trace event
        self.record_event(crate::trace::RunEvent::ToolCall {
            call_id: call_id.clone(),
            name: tool_name.to_string(),
            args: safe_args,
            risk: None,
            duration_ms: 0,
        })
        .await;

        let result = match self
            .tools
            .tool_manager
            .execute_tool(tool_name, effective_params.clone())
            .await
        {
            Ok(r) => r,
            Err(e) => {
                let err_msg = e.to_string();
                // Record ToolError trace event
                self.record_event(crate::trace::RunEvent::ToolError {
                    call_id: call_id.clone(),
                    name: tool_name.to_string(),
                    message: err_msg.clone(),
                })
                .await;
                // Audit failure
                if let Some(al) = &self.guard.audit_logger {
                    let ev = crate::audit::AuditEvent::now(
                        self.config.session_id.clone(),
                        self.config.agent_name.clone(),
                        crate::audit::AuditEventType::ToolCall {
                            tool: tool_name.to_string(),
                            input: effective_input.clone(),
                            output: err_msg.clone(),
                            success: false,
                            duration_ms: 0,
                        },
                    );
                    let _ = al.log(ev).await;
                }
                // Error softening
                if self.config.tool_error_feedback && tool_name != TOOL_FINAL_ANSWER {
                    return Ok(format!(
                        "[Tool error] {e}\nTry adjusting parameters or using another tool."
                    ));
                }
                return Err(crate::error::ReactError::from(e));
            }
        };

        // Record ToolResult trace event
        self.record_event(crate::trace::RunEvent::ToolResult {
            call_id: call_id.clone(),
            name: tool_name.to_string(),
            success: result.success,
            output_preview: Some(result.output.chars().take(200).collect()),
            output_truncated: false,
            duration_ms: 0,
        })
        .await;

        if result.success {
            self.record_file_read_if_needed(tool_name, &effective_params);
        }

        // ── PostToolUse hooks ──
        let post_result = hook_reg
            .run_post_tool_use(
                tool_name,
                &effective_input,
                &result.output,
                self.config.session_id.as_deref().unwrap_or(""),
            )
            .await;
        if post_result.block {
            let reason = post_result
                .block_reason
                .unwrap_or_else(|| format!("Tool {} output blocked by hook", tool_name));
            return Ok(reason);
        }

        // ── Truncate ──
        let truncated = self.truncate_output(result.output).await;
        Ok(truncated)
    }

    fn validate_read_before_edit(
        &self,
        tool_name: &str,
        params: &ToolParameters,
    ) -> std::result::Result<(), crate::error::ReactError> {
        if !self.config.force_read_before_edit || !is_write_tool(tool_name) {
            return Ok(());
        }

        if let Some(path) = extract_path_param(params) {
            let canonical = canonicalize_for_read_tracking(&path);
            let ttl = std::time::Duration::from_secs(30 * 60);
            let mut files = self.recently_read_files.lock().unwrap();
            let read = match files.get(&canonical) {
                Some(instant) if instant.elapsed() < ttl => true,
                Some(_) => { files.remove(&canonical); false }
                None => false,
            };
            drop(files);
            if !read {
                return Err(crate::error::ReactError::Other(format!(
                    "Read-before-edit is enabled. File '{}' has not been read in this conversation turn. Use read_file to read it first, then retry this operation.",
                    path
                )));
            }
        }

        Ok(())
    }

    fn record_file_read_if_needed(&self, tool_name: &str, params: &ToolParameters) {
        if !self.config.force_read_before_edit || !is_read_tool(tool_name) {
            return;
        }

        if let Some(path) = extract_path_param(params) {
            let canonical = canonicalize_for_read_tracking(&path);
            self.recently_read_files.lock().unwrap().insert(canonical, std::time::Instant::now());
        }
    }
}

// ── Read-before-edit helpers ─────────────────────────────────────────────────

const WRITE_TOOLS: &[&str] = &[
    "edit_file",
    "write_file",
    "append_file",
    "create_file",
    "delete_file",
    "update_file",
    "move_file",
];

const READ_TOOLS: &[&str] = &["read_file", "read_text"];

fn is_write_tool(name: &str) -> bool {
    WRITE_TOOLS.contains(&name)
}

fn is_read_tool(name: &str) -> bool {
    READ_TOOLS.contains(&name)
}

fn extract_path_param(params: &ToolParameters) -> Option<String> {
    params
        .get("path")
        .or_else(|| params.get("file_path"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn canonicalize_for_read_tracking(path: &str) -> String {
    std::fs::canonicalize(path)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| path.to_string())
}
