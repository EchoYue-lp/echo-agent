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
use echo_core::circuit_breaker::CircuitBreaker;
use echo_core::tools::permission::PermissionPolicy;
use futures::future::join_all;
use futures::StreamExt;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{Instrument, debug, info, info_span, warn};

macro_rules! yield_event {
    ($tx:expr, $event:expr) => { if $tx.send(Ok($event)).is_err() { return Ok(()); } };
}
macro_rules! try_send {
    ($tx:expr, $fallible:expr) => {
        match $fallible { Ok(v) => v, Err(e) => { let _ = $tx.send(Err(e.into())); return Ok(()); } }
    };
}

// ── ReactAgent: entry point ──────────────────────────────────────────

impl ReactAgent {
    /// Channel-based streaming entry point. Returns `BoxStream<'static>`.
    pub(crate) async fn run_stream_channel(
        &self, init: StreamInit, mode: StreamMode,
    ) -> Result<futures::stream::BoxStream<'static, Result<AgentEvent>>> {
        let (tx, rx) = mpsc::unbounded_channel::<Result<AgentEvent>>();
        let context = self.memory.context.clone();
        let text = init.text.clone();
        let message = init.message.clone();
        let label = init.label.clone();

        let recalled = if let Some(ref msg) = init.message {
            self.prepare_stream_context_with_message(mode, msg).await
        } else { self.prepare_stream_context(mode, &init.text).await };

        let snap = AgentSnapshot::from_agent(self);

        tokio::spawn(async move {
            if let Err(e) = snap.run_loop(context, text, message, label, mode, recalled, tx.clone()).await {
                let _ = tx.send(Err(e));
            }
        });

        Ok(Box::pin(tokio_stream::wrappers::UnboundedReceiverStream::new(rx)))
    }
}

// ── AgentSnapshot ────────────────────────────────────────────────────

struct AgentSnapshot {
    agent_name: String,
    callbacks: Vec<Arc<dyn crate::agent::AgentCallback>>,
    max_iterations: usize,
    session_id: Option<String>,
    model_name: String,
    temperature: Option<f32>,
    max_tokens: Option<u32>,
    tool_error_feedback: bool,
    enable_tool: bool,
    llm_max_retries: usize,
    llm_retry_delay_ms: u64,
    max_tool_output_tokens: Option<usize>,
    tool_execution: crate::tools::ToolExecutionConfig,
    hook_registry: Arc<tokio::sync::RwLock<crate::skills::hooks::HookRegistry>>,
    tool_manager: Arc<crate::tools::ToolManager>,
    permission_policy: Option<Arc<dyn PermissionPolicy>>,
    audit_logger: Option<Arc<dyn crate::audit::AuditLogger>>,
    circuit_breaker: Option<Arc<CircuitBreaker>>,
    checkpointer: Option<Arc<dyn crate::memory::checkpointer::Checkpointer>>,
    snapshot_manager: Arc<std::sync::RwLock<Option<crate::memory::snapshot::SnapshotManager>>>,
    client: Arc<reqwest::Client>,
    cancel_token: Option<crate::agent::CancellationToken>,
    #[cfg(feature = "human-loop")]
    permission_service: Option<Arc<crate::human_loop::PermissionService>>,
}

impl AgentSnapshot {
    fn from_agent(agent: &ReactAgent) -> Self {
        Self {
            agent_name: agent.config.agent_name.clone(),
            callbacks: agent.config.callbacks.to_vec(),
            max_iterations: agent.config.max_iterations,
            session_id: agent.config.session_id.clone(),
            model_name: agent.config.model_name.clone(),
            temperature: agent.config.temperature,
            max_tokens: agent.config.max_tokens,
            tool_error_feedback: agent.config.tool_error_feedback,
            enable_tool: agent.config.enable_tool,
            llm_max_retries: agent.config.llm_max_retries,
            llm_retry_delay_ms: agent.config.llm_retry_delay_ms,
            max_tool_output_tokens: agent.config.max_tool_output_tokens,
            tool_execution: agent.config.tool_execution.clone(),
            hook_registry: agent.tools.hook_registry.clone(),
            tool_manager: Arc::clone(&agent.tools.tool_manager),
            permission_policy: agent.guard.permission_policy.clone(),
            audit_logger: agent.guard.audit_logger.clone(),
            circuit_breaker: agent.guard.circuit_breaker.clone(),
            checkpointer: agent.memory.checkpointer.clone(),
            snapshot_manager: agent.memory.snapshot_manager.clone(),
            client: agent.client().clone(),
            cancel_token: None,
            #[cfg(feature = "human-loop")]
            permission_service: agent.approval.permission_service.clone(),
        }
    }

    // ── Main loop ────────────────────────────────────────────────────

    async fn run_loop(
        self, context: Arc<tokio::sync::Mutex<crate::compression::ContextManager>>,
        text: String, _message: Option<Message>, label: String,
        mode: StreamMode, recalled: usize, tx: mpsc::UnboundedSender<Result<AgentEvent>>,
    ) -> Result<()> {
        let agent = self.agent_name.clone();
        let callbacks = self.callbacks.clone();

        match mode {
            StreamMode::Execute => info!(agent = %agent, "Agent streaming task execution{label}"),
            StreamMode::Chat => info!(agent = %agent, "Agent streaming conversation{label}"),
        }
        if recalled > 0 { yield_event!(tx, AgentEvent::MemoryRecalled { count: recalled }); }

        // Audit: user input
        if let Some(al) = &self.audit_logger {
            let event = crate::audit::AuditEvent::now(self.session_id.clone(), self.agent_name.clone(),
                crate::audit::AuditEventType::UserInput { content: text.clone() });
            let _ = al.log(event).await;
        }

        // UserPromptSubmit hook
        {
            let hook_ctx = crate::skills::hooks::HookContext::for_user_prompt_submit(
                &text, None, self.session_id.as_deref().unwrap_or(""), &self.agent_name);
            let registry = self.hook_registry.read().await.clone();
            let result = registry.run_lifecycle_hooks(&hook_ctx).await;
            if result.block {
                yield_event!(tx, AgentEvent::FinalAnswer(format!("Blocked by UserPromptSubmit hook: {}",
                    result.block_reason.unwrap_or_default())));
                self.fire_hook(crate::skills::hooks::HookEvent::SessionEnd, Some("blocked")).await;
                return Ok(());
            }
            if let Some(ctx) = &result.injected_context { context.lock().await.push(Message::system(ctx.clone())); }
            for msg in &result.messages { context.lock().await.push(Message::system(msg.clone())); }
        }

        let mut stop_hook_continued = false;

        for iteration in 0..self.max_iterations {
            for cb in &callbacks { cb.on_iteration(&agent, iteration).await; }
            debug!(agent = %agent, iteration = iteration + 1, "--- Streaming iteration{label} ---");

            self.fire_hook(crate::skills::hooks::HookEvent::PreCompact, Some("auto")).await;
            let prepare_result = try_send!(tx, context.lock().await.prepare(None).await);

            if let Some(ref stats) = prepare_result.compressed {
                yield_event!(tx, AgentEvent::ContextCompressed {
                    before_count: stats.before_count, after_count: stats.after_count,
                    before_tokens: stats.before_tokens, after_tokens: stats.after_tokens,
                });
                let hs = crate::skills::hooks::CompressHookStats {
                    before_count: stats.before_count, after_count: stats.after_count,
                    before_tokens: stats.before_tokens, after_tokens: stats.after_tokens,
                };
                let hc = crate::skills::hooks::HookContext::for_post_compact(
                    &hs, "auto", self.session_id.as_deref().unwrap_or(""), &self.agent_name);
                let reg = self.hook_registry.read().await.clone();
                let r = reg.run_lifecycle_hooks(&hc).await;
                if let Some(c) = &r.injected_context { context.lock().await.push(Message::system(format!("[Hook:PostCompact] {}", c))); }
                for m in &r.messages { context.lock().await.push(Message::system(m.clone())); }
            }

            let messages = prepare_result.messages;
            for cb in &callbacks { cb.on_think_start(&agent, &messages).await; }

            let mut llm_stream = Box::pin(try_send!(tx, self.create_llm_stream(messages.clone()).await));
            let mut content_buffer = String::new();
            let mut tool_call_map: HashMap<u32, (String, String, String)> = HashMap::new();
            let mut last_usage = None;
            let mut in_reasoning = false;

            while let Some(cr) = llm_stream.next().await {
                let chunk = try_send!(tx, cr);
                if chunk.usage.is_some() { last_usage = chunk.usage.clone(); }
                for event in process_stream_chunk(&chunk, &mut content_buffer, &mut tool_call_map, &mut in_reasoning) {
                    yield_event!(tx, event);
                }
            }

            let pt = last_usage.as_ref().and_then(|u| u.prompt_tokens).unwrap_or(0) as usize;
            let ct = last_usage.as_ref().and_then(|u| u.completion_tokens).unwrap_or(0) as usize;
            if in_reasoning { yield_event!(tx, AgentEvent::ThinkEnd { prompt_tokens: pt, completion_tokens: ct }); }

            if !tool_call_map.is_empty() {
                let (msg_tc, steps) = build_tool_calls_from_map(&tool_call_map);
                for (_, name, args) in &steps { yield_event!(tx, AgentEvent::ToolCall { name: name.clone(), args: args.clone() }); }
                {
                    let ts: Vec<StepType> = steps.iter().map(|(id, n, a)| StepType::Call {
                        tool_call_id: id.clone(), function_name: n.clone(), arguments: a.clone() }).collect();
                    for cb in &callbacks { cb.on_think_end(&agent, &ts, pt, ct).await; }
                }
                context.lock().await.push(Message::assistant_with_tools(msg_tc));

                #[cfg(feature = "human-loop")]
                let (appr, conc) = {
                    let mut a = vec![]; let mut c = vec![];
                    for s in steps { if self.tool_needs_approval(&s.1).await { a.push(s); } else { c.push(s); } }
                    (a, c)
                };
                #[cfg(not(feature = "human-loop"))]
                let (appr, conc): (Vec<_>, Vec<_>) = (vec![], steps);

                if !conc.is_empty() {
                    let mc = self.tool_manager.max_concurrency();
                    let futs: Vec<_> = conc.iter().map(|(_, n, a)| {
                        let tm = Arc::clone(&self.tool_manager);
                        let name = n.clone(); let args = a.clone(); let soften = self.tool_error_feedback;
                        async move {
                            let params = if let Value::Object(m) = &args { m.clone().into_iter().collect() } else { HashMap::new() };
                            match tm.execute_tool(&name, params).await {
                                Ok(o) => Ok(o.output),
                                Err(e) if soften && name != TOOL_FINAL_ANSWER => Ok(format!("[Tool error] {e}\nTry adjusting parameters or using another tool.")),
                                Err(e) => Err(ReactError::from(e)),
                            }
                        }.instrument(info_span!("tool", tool.name = %n))
                    }).collect();
                    let bt = super::retry::compute_concurrent_tool_batch_timeout(&self.tool_execution, futs.len(), mc);
                    let results: Vec<std::result::Result<String, ReactError>>;
                    if let Some(to) = bt {
                        results = try_send!(tx, tokio::time::timeout(to, join_all(futs)).await
                            .map_err(|_| ReactError::from(crate::error::ToolError::Timeout("batch timeout".into()))));
                    } else { results = join_all(futs).await; }

                    for ((id, fname, _), result) in conc.into_iter().zip(results) {
                        match result {
                            Ok(output) => {
                                let truncated = self.truncate_output(output).await;
                                yield_event!(tx, AgentEvent::ToolResult { name: fname.clone(), output: truncated.clone() });
                                context.lock().await.push(Message::tool_result(id, fname.clone(), truncated.clone()));
                                if fname == TOOL_FINAL_ANSWER {
                                    return self.finish(context, agent, callbacks, label, &truncated, iteration, stop_hook_continued, tx).await;
                                }
                            }
                            Err(error) => {
                                yield_event!(tx, AgentEvent::ToolError { name: fname.clone(), error: error.to_string() });
                                context.lock().await.push(Message::tool_result(id, fname.clone(), format!("[Error] {error}")));
                            }
                        }
                    }
                }

                for (id, fname, args) in appr {
                    let params = if let Value::Object(m) = &args { m.clone().into_iter().collect() } else { HashMap::new() };
                    match self.tool_manager.execute_tool(&fname, params).await {
                        Ok(result) => {
                            let truncated = self.truncate_output(result.output).await;
                            yield_event!(tx, AgentEvent::ToolResult { name: fname.clone(), output: truncated.clone() });
                            context.lock().await.push(Message::tool_result(id, fname.clone(), truncated.clone()));
                            if fname == TOOL_FINAL_ANSWER {
                                return self.finish(context, agent, callbacks, label, &truncated, iteration, stop_hook_continued, tx).await;
                            }
                        }
                        Err(error) => {
                            yield_event!(tx, AgentEvent::ToolError { name: fname.clone(), error: error.to_string() });
                            context.lock().await.push(Message::tool_result(id, fname.clone(), format!("[Error] {error}")));
                        }
                    }
                }
                self.auto_snapshot(&context, iteration).await;
            } else if !content_buffer.is_empty() {
                let ts = vec![StepType::Thought(content_buffer.clone())];
                for cb in &callbacks { cb.on_think_end(&agent, &ts, pt, ct).await; cb.on_final_answer(&agent, &content_buffer).await; }
                context.lock().await.push(Message::assistant(content_buffer.clone()));
                self.auto_snapshot(&context, iteration).await;
                if let Some(al) = &self.audit_logger {
                    let ev = crate::audit::AuditEvent::now(self.session_id.clone(), self.agent_name.clone(),
                        crate::audit::AuditEventType::FinalAnswer { content: content_buffer.clone() });
                    let _ = al.log(ev).await;
                }
                self.save_checkpoint(&context).await;
                yield_event!(tx, AgentEvent::FinalAnswer(content_buffer));
                let hc = crate::skills::hooks::HookContext::for_stop(None, self.session_id.as_deref().unwrap_or(""), &self.agent_name, stop_hook_continued);
                let reg = self.hook_registry.read().await.clone();
                let sr = reg.run_lifecycle_hooks(&hc).await;
                if let Some(reason) = &sr.continue_reason { if !stop_hook_continued {
                    context.lock().await.push(Message::system(format!("[Hook:Stop] Continue: {}", reason)));
                    stop_hook_continued = true; continue;
                }}
                self.fire_hook(crate::skills::hooks::HookEvent::SessionEnd, Some("complete")).await;
                return Ok(());
            } else {
                let _ = tx.send(Err(ReactError::Agent(Box::new(AgentError::NoResponse { model: self.model_name.clone(), agent: self.agent_name.clone() }))));
                return Ok(());
            }
        }

        self.fire_hook(crate::skills::hooks::HookEvent::SessionEnd, Some("max_iterations")).await;
        self.fire_hook(crate::skills::hooks::HookEvent::StopFailure, Some("max_iterations")).await;
        let _ = tx.send(Err(ReactError::Agent(Box::new(AgentError::MaxIterationsExceeded(self.max_iterations)))));
        Ok(())
    }

    // ── Helpers ──────────────────────────────────────────────────────

    async fn create_llm_stream(&self, messages: Vec<Message>) -> Result<impl futures::Stream<Item = Result<crate::llm::types::ChatCompletionChunk>>> {
        let tools = if self.enable_tool {
            let t = self.tool_manager.get_openai_tools();
            if t.is_empty() { None } else { Some(t) }
        } else { None };
        let cancel = self.cancel_token.clone();
        super::retry::retry_llm_call(&self.agent_name, self.llm_max_retries, self.llm_retry_delay_ms, &self.circuit_breaker, || {
            let c = self.client.clone(); let m = self.model_name.clone(); let ms = messages.clone();
            let t = tools.clone(); let ct = cancel.clone();
            async move { crate::llm::stream_chat(c, &m, ms, self.temperature, self.max_tokens, t, None, None, ct).await }
        }).await
    }

    async fn truncate_output(&self, output: String) -> String {
        let Some(mt) = self.max_tool_output_tokens else { return output; };
        if output.chars().count() / 3 <= mt { return output; }
        let ratio = mt as f64 / (output.chars().count() as f64 / 3.0);
        format!("{}\n[Output truncated]", output.chars().take((output.len() as f64 * ratio * 0.95) as usize).collect::<String>())
    }

    async fn fire_hook(&self, event: crate::skills::hooks::HookEvent, matcher: Option<&str>) {
        let sid = self.session_id.clone().unwrap_or_default();
        let hc = match event {
            crate::skills::hooks::HookEvent::SessionEnd =>
                crate::skills::hooks::HookContext::for_session_end(matcher.unwrap_or("other"), &sid, &self.agent_name),
            crate::skills::hooks::HookEvent::PreCompact =>
                crate::skills::hooks::HookContext::for_pre_compact(&Default::default(), matcher.unwrap_or("auto"), &sid, &self.agent_name),
            crate::skills::hooks::HookEvent::StopFailure =>
                crate::skills::hooks::HookContext::for_stop_failure("", matcher.unwrap_or(""), &sid, &self.agent_name),
            _ => return,
        };
        let reg = self.hook_registry.read().await.clone();
        let _ = reg.run_lifecycle_hooks(&hc).await;
    }

    async fn auto_snapshot(&self, context: &Arc<tokio::sync::Mutex<crate::compression::ContextManager>>, iteration: usize) {
        let should_capture = {
            let mgr = self.snapshot_manager.read().unwrap();
            mgr.as_ref().is_some_and(|m| m.should_capture(iteration))
            // RwLockReadGuard dropped here — before any await
        };
        if should_capture {
            let ctx = context.lock().await;
            let ms = ctx.messages().to_vec();
            drop(ctx);
            if let Some(ref mut m) = *self.snapshot_manager.write().unwrap() { m.capture(iteration, &ms); }
        }
    }

    async fn save_checkpoint(&self, context: &Arc<tokio::sync::Mutex<crate::compression::ContextManager>>) {
        if let (Some(cp), Some(sid)) = (&self.checkpointer, &self.session_id) {
            let ctx = context.lock().await;
            let state = crate::memory::ThreadState { messages: ctx.messages().to_vec(), summary: None, metadata: None };
            drop(ctx);
            let _ = cp.put_state(sid, state).await;
        }
    }

    #[cfg(feature = "human-loop")]
    async fn tool_needs_approval(&self, tool_name: &str) -> bool {
        use crate::tools::permission::{PermissionDecision, PermissionMode};
        if let Some(svc) = &self.permission_service {
            let mode = svc.mode().await;
            if matches!(mode, PermissionMode::BypassPermissions | PermissionMode::DontAsk | PermissionMode::Plan) { return false; }
            let perms = self.tool_manager.get_tool(tool_name).map(|t| t.permissions()).unwrap_or_default();
            return svc.check_with_permissions(tool_name, &serde_json::json!({}), &perms).await
                .unwrap_or(PermissionDecision::RequireApproval).requires_approval();
        }
        if let Some(pol) = &self.permission_policy {
            let perms = self.tool_manager.get_tool(tool_name).map(|t| t.permissions()).unwrap_or_default();
            if !perms.is_empty() {
                let d = pol.check(tool_name, &perms).await;
                return matches!(d, PermissionDecision::RequireApproval | PermissionDecision::Ask { .. });
            }
        }
        false
    }
    #[cfg(not(feature = "human-loop"))]
    async fn tool_needs_approval(&self, _: &str) -> bool { false }

    #[allow(clippy::too_many_arguments)]
    async fn finish(
        &self, context: Arc<tokio::sync::Mutex<crate::compression::ContextManager>>,
        agent: String, callbacks: Vec<Arc<dyn crate::agent::AgentCallback>>,
        label: String, output: &str, iteration: usize,
        stop_hook_continued: bool, tx: mpsc::UnboundedSender<Result<AgentEvent>>,
    ) -> Result<()> {
        for cb in &callbacks { cb.on_final_answer(&agent, output).await; }
        info!(agent = %agent, "Streaming execution completed{label}");
        if let Some(al) = &self.audit_logger {
            let ev = crate::audit::AuditEvent::now(self.session_id.clone(), self.agent_name.clone(),
                crate::audit::AuditEventType::FinalAnswer { content: output.to_string() });
            let _ = al.log(ev).await;
        }
        self.save_checkpoint(&context).await;
        yield_event!(tx, AgentEvent::FinalAnswer(output.to_string()));
        let hc = crate::skills::hooks::HookContext::for_stop(None, self.session_id.as_deref().unwrap_or(""), &self.agent_name, stop_hook_continued);
        let reg = self.hook_registry.read().await.clone();
        let sr = reg.run_lifecycle_hooks(&hc).await;
        if let Some(reason) = &sr.continue_reason { if !stop_hook_continued {
            context.lock().await.push(Message::system(format!("[Hook:Stop] Continue: {}", reason)));
        }}
        self.fire_hook(crate::skills::hooks::HookEvent::SessionEnd, Some("complete")).await;
        Ok(())
    }
}
