//! Channel-based streaming execution
//!
//! This module provides an alternative to the try_stream!-based streaming in
//! stream_loop.rs. Instead of generating a large async state machine via the
//! async_stream crate, we use a tokio::mpsc channel + spawned task.
//!
//! This avoids the inline generator storage in async_stream::AsyncStream that
//! can overflow the stack in debug builds (opt-level=0) when the try_stream!
//! body spans 500+ lines with 30+ yield/await points.

use super::super::{ReactAgent, StepType, TOOL_FINAL_ANSWER};
use super::execution::ToolExecutionFailure;
use super::{StreamInit, StreamMode};
use crate::agent::AgentEvent;
use crate::error::{AgentError, ReactError, Result};
use crate::llm::types::{FunctionCall, Message, ToolCall as LlmToolCall};
use futures::future::join_all;
use futures::StreamExt;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{Instrument, debug, info, info_span, warn};

// ── Convenience macros for channel-based control flow ──────────────
// These mirror try_stream!'s `yield` and `?` semantics using explicit
// channel sends instead of generated state machine variants.

/// Send an event through the channel. Return from the function if the
/// receiver has been dropped (cancellation).
macro_rules! yield_event {
    ($tx:expr, $event:expr) => {
        if $tx.send(Ok($event)).is_err() {
            return Ok(());
        }
    };
}

/// Equivalent to the `?` operator inside try_stream!: on error, sends
/// `Err(e.into())` through the channel and returns.
macro_rules! try_send {
    ($tx:expr, $fallible:expr) => {
        match $fallible {
            Ok(v) => v,
            Err(e) => {
                let _ = $tx.send(Err(e.into()));
                return Ok(());
            }
        }
    };
}

impl ReactAgent {
    /// Channel-based streaming entry point.
    ///
    /// Unlike `run_stream_inner` which uses `try_stream!` and can produce
    /// a very large stack-allocated state machine in debug builds, this
    /// method spawns a dedicated task that processes the main loop and
    /// sends events through an mpsc channel. The returned stream is a
    /// lightweight `UnboundedReceiverStream`.
    pub(crate) async fn run_stream_channel(
        &self,
        init: StreamInit,
        mode: StreamMode,
    ) -> Result<futures::stream::BoxStream<'static, Result<AgentEvent>>> {
        let (tx, rx) = mpsc::unbounded_channel::<Result<AgentEvent>>();

        // Clone everything the spawned task needs from &self.
        let context = self.memory.context.clone();
        let text = init.text.clone();
        let message = init.message.clone();
        let label = init.label.clone();

        // Prepare context synchronously (relative to the caller) before spawning.
        let recalled = if let Some(ref msg) = init.message {
            self.prepare_stream_context_with_message(mode, msg).await
        } else {
            self.prepare_stream_context(mode, &init.text).await
        };

        // Build a "snapshot" of &self state for the spawned task.
        // We clone Arcs and copy/config values; the spawned task holds
        // these independently of the borrow.
        let snapshot = AgentSnapshot {
            agent_name: self.config.agent_name.clone(),
            callbacks: self.config.callbacks.clone(),
            max_iterations: self.config.max_iterations,
            session_id: self.config.session_id.clone(),
            model_name: self.config.model_name.clone(),
            temperature: self.config.temperature,
            max_tokens: self.config.max_tokens,
            response_format: self.config.response_format.clone(),
            tool_error_feedback: self.config.tool_error_feedback,
            enable_tool: self.config.enable_tool,
            llm_max_retries: self.config.llm_max_retries,
            llm_retry_delay_ms: self.config.llm_retry_delay_ms,
            max_tool_output_tokens: self.config.max_tool_output_tokens,
            tool_execution: self.config.tool_execution.clone(),
            hook_registry: self.tools.hook_registry.clone(),
            tool_manager: self.tools.tool_manager.clone_arc(),
            skill_registry: self.tools.skill_registry.clone(),
            mcp_manager: self.tools.mcp_manager_arc(),
            sandbox_manager: self.tools.sandbox_manager.clone(),
            guard_manager: self.guard.guard_manager.clone(),
            permission_policy: self.guard.permission_policy.clone(),
            audit_logger: self.guard.audit_logger.clone(),
            circuit_breaker: self.guard.circuit_breaker.clone(),
            checkpointer: self.memory.checkpointer.clone(),
            snapshot_manager: self.memory.snapshot_manager.clone(),
            store: self.memory.store.clone(),
            client: self.client.clone(),
            llm_client: self.llm_client.clone(),
            llm_config: self.llm_config.clone(),
            approval_provider: self.approval.provider(),
            approval_policy: self.approval.policy(),
            subagent_registry: self.tools.subagent_registry(),
            subagent_executor: self.tools.subagent_executor(),
            task_manager: self.tools.task_manager(),
            progressive_skill_registry: self.tools.progressive_skill_registry(),
        };

        let recalled_val = recalled;

        tokio::spawn(async move {
            if let Err(e) = Self::run_stream_loop_impl(
                snapshot,
                context,
                text,
                message,
                label,
                mode,
                recalled_val,
                tx.clone(),
            )
            .await
            {
                let _ = tx.send(Err(e));
            }
        });

        Ok(Box::pin(
            tokio_stream::wrappers::UnboundedReceiverStream::new(rx),
        ))
    }
}

// ── State snapshot for the spawned task ────────────────────────────
//
// We clone/copy every field the main loop needs so the spawned task
// can run without borrowing from the agent's RwLockReadGuard.

#[allow(dead_code)]
struct AgentSnapshot {
    agent_name: String,
    callbacks: Arc<Vec<Arc<dyn crate::agent::AgentCallback>>>,
    max_iterations: usize,
    session_id: Option<String>,
    model_name: String,
    temperature: Option<f64>,
    max_tokens: Option<u32>,
    response_format: Option<crate::llm::config::ResponseFormat>,
    tool_error_feedback: bool,
    enable_tool: bool,
    llm_max_retries: u32,
    llm_retry_delay_ms: u64,
    max_tool_output_tokens: Option<usize>,
    tool_execution: crate::tools::ToolExecutionConfig,
    hook_registry: Arc<tokio::sync::RwLock<crate::skills::hooks::HookRegistry>>,
    tool_manager: Arc<crate::tools::ToolManager>,
    skill_registry: Arc<crate::skills::SkillRegistry>,
    mcp_manager: Option<Arc<crate::mcp::McpManager>>,
    sandbox_manager: Option<Arc<crate::sandbox::SandboxManager>>,
    guard_manager: Option<Arc<crate::guard::GuardManager>>,
    permission_policy: Option<Arc<crate::guard::PermissionPolicy>>,
    audit_logger: Option<Arc<dyn crate::audit::AuditLogger>>,
    circuit_breaker: Option<Arc<crate::circuit_breaker::CircuitBreaker>>,
    checkpointer: Option<Arc<dyn crate::memory::checkpointer::Checkpointer>>,
    snapshot_manager: Option<Arc<crate::memory::snapshot::SnapshotManager>>,
    store: Option<Arc<dyn crate::memory::store::Store>>,
    client: Arc<reqwest::Client>,
    llm_client: Option<Arc<dyn crate::llm::LlmClient>>,
    llm_config: Option<crate::llm::config::LlmConfig>,
    approval_provider: Option<Arc<dyn crate::human_loop::HumanLoopProvider>>,
    approval_policy: Option<Arc<dyn crate::human_loop::PermissionPolicy>>,
    #[cfg(feature = "subagent")]
    subagent_registry: Option<Arc<crate::agent::subagent::SubagentRegistry>>,
    #[cfg(feature = "subagent")]
    subagent_executor: Option<Arc<crate::agent::subagent::executor::SubagentExecutor>>,
    #[cfg(feature = "tasks")]
    task_manager: Option<Arc<crate::tasks::TaskManager>>,
    progressive_skill_registry: Option<Arc<crate::skills::ProgressiveSkillRegistry>>,
}

impl ReactAgent {
    /// The main streaming loop, running inside a spawned task.
    /// Sends events through `tx` instead of yielding from a try_stream! block.
    async fn run_stream_loop_impl(
        snap: AgentSnapshot,
        context: Arc<tokio::sync::Mutex<crate::compression::ContextManager>>,
        text: String,
        _message: Option<Message>,
        label: String,
        mode: StreamMode,
        recalled: usize,
        tx: mpsc::UnboundedSender<Result<AgentEvent>>,
    ) -> Result<()> {
        let agent = snap.agent_name.clone();
        let callbacks = snap.callbacks.clone();

        match mode {
            StreamMode::Execute => {
                info!(agent = %agent, "🌊 Agent starting streaming task execution{label}")
            }
            StreamMode::Chat => {
                info!(agent = %agent, "🌊 Agent starting streaming multi-round conversation{label}")
            }
        }

        if recalled > 0 {
            yield_event!(tx, AgentEvent::MemoryRecalled { count: recalled });
        }

        // Log user input audit (fire-and-forget: clone what we need)
        {
            let audit_logger = snap.audit_logger.clone();
            let session_id = snap.session_id.clone();
            let agent_name = snap.agent_name.clone();
            let text_clone = text.clone();
            tokio::spawn(async move {
                crate::agent::react::ReactAgent::log_user_input_audit_static(
                    audit_logger,
                    session_id,
                    agent_name,
                    text_clone,
                )
                .await;
            });
        }

        // Fire UserPromptSubmit hook
        {
            let hook_ctx = crate::skills::hooks::HookContext::for_user_prompt_submit(
                &text,
                None,
                snap.session_id.as_deref().unwrap_or(""),
                &snap.agent_name,
            );
            let registry = snap.hook_registry.read().await.clone();
            let prompt_result = registry.run_lifecycle_hooks(&hook_ctx).await;
            if prompt_result.block {
                yield_event!(
                    tx,
                    AgentEvent::FinalAnswer(format!(
                        "Blocked by UserPromptSubmit hook: {}",
                        prompt_result.block_reason.unwrap_or_default()
                    ))
                );
                // Fire SessionEnd hook
                Self::fire_lifecycle_hook_static(
                    snap.hook_registry.clone(),
                    crate::skills::hooks::HookEvent::SessionEnd,
                    Some("blocked"),
                    snap.session_id.as_deref().unwrap_or(""),
                    &snap.agent_name,
                )
                .await;
                return Ok(());
            }
            if let Some(ctx) = &prompt_result.injected_context {
                context
                    .lock()
                    .await
                    .push(Message::system(ctx.clone()));
            }
            for msg in &prompt_result.messages {
                context.lock().await.push(Message::system(msg.clone()));
            }
        }

        let mut stop_hook_continued = false;

        for iteration in 0..snap.max_iterations {
            for cb in &callbacks {
                cb.on_iteration(&agent, iteration).await;
            }

            debug!(agent = %agent, iteration = iteration + 1, "--- Streaming iteration{label} ---");

            // Fire PreCompact hooks
            Self::fire_lifecycle_hook_static(
                snap.hook_registry.clone(),
                crate::skills::hooks::HookEvent::PreCompact,
                Some("auto"),
                snap.session_id.as_deref().unwrap_or(""),
                &snap.agent_name,
            )
            .await;

            let prepare_result = try_send!(tx, context.lock().await.prepare(None).await);

            if let Some(ref stats) = prepare_result.compressed {
                yield_event!(tx, AgentEvent::ContextCompressed {
                    before_count: stats.before_count,
                    after_count: stats.after_count,
                    before_tokens: stats.before_tokens,
                    after_tokens: stats.after_tokens,
                });

                // Fire PostCompact hook
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
                        snap.session_id.as_deref().unwrap_or(""),
                        &snap.agent_name,
                    );
                    let registry = snap.hook_registry.read().await.clone();
                    let post_result = registry.run_lifecycle_hooks(&hook_ctx).await;
                    if let Some(ctx) = &post_result.injected_context {
                        context.lock().await.push(Message::system(format!(
                            "[Hook:PostCompact] {}",
                            ctx
                        )));
                    }
                    for msg in &post_result.messages {
                        context
                            .lock()
                            .await
                            .push(Message::system(msg.clone()));
                    }
                }
            }

            let messages = prepare_result.messages;

            for cb in &callbacks {
                cb.on_think_start(&agent, &messages).await;
            }

            // Create LLM stream
            let llm_stream = try_send!(
                tx,
                Self::create_llm_stream_static(
                    &snap,
                    &agent,
                    messages.clone(),
                )
                .await
            );
            let mut llm_stream = Box::pin(llm_stream);

            // Collect streaming response
            let mut content_buffer = String::new();
            let mut tool_call_map: HashMap<u32, (String, String, String)> = HashMap::new();
            let mut last_usage: Option<crate::llm::types::Usage> = None;
            let mut in_reasoning = false;

            while let Some(chunk_result) = llm_stream.next().await {
                let chunk = try_send!(tx, chunk_result);
                if chunk.usage.is_some() {
                    last_usage = chunk.usage.clone();
                }
                for event in
                    Self::process_stream_chunk(&chunk, &mut content_buffer, &mut tool_call_map, &mut in_reasoning)
                {
                    yield_event!(tx, event);
                }
            }

            let prompt_tokens = last_usage
                .as_ref()
                .and_then(|u| u.prompt_tokens)
                .unwrap_or(0) as usize;
            let completion_tokens = last_usage
                .as_ref()
                .and_then(|u| u.completion_tokens)
                .unwrap_or(0) as usize;

            if in_reasoning {
                yield_event!(tx, AgentEvent::ThinkEnd {
                    prompt_tokens,
                    completion_tokens,
                });
            }

            let has_tool_calls = !tool_call_map.is_empty();

            if has_tool_calls {
                let (msg_tool_calls, steps) = Self::build_tool_calls_from_map(&tool_call_map);

                for (_, name, args) in &steps {
                    yield_event!(tx, AgentEvent::ToolCall {
                        name: name.clone(),
                        args: args.clone(),
                    });
                }

                // on_think_end callbacks
                {
                    let think_steps: Vec<StepType> = steps
                        .iter()
                        .map(|(id, name, args)| StepType::Call {
                            tool_call_id: id.clone(),
                            function_name: name.clone(),
                            arguments: args.clone(),
                        })
                        .collect();
                    for cb in &callbacks {
                        cb.on_think_end(&agent, &think_steps, prompt_tokens, completion_tokens)
                            .await;
                    }
                }

                context
                    .lock()
                    .await
                    .push(Message::assistant_with_tools(msg_tool_calls));

                // Separate approval tools from concurrent tools
                #[cfg(feature = "human-loop")]
                let (approval_steps, concurrent_steps) = {
                    let mut approval = Vec::new();
                    let mut concurrent = Vec::new();
                    for step in steps {
                        if Self::tool_needs_approval_static(
                            &snap,
                            &step.1,
                        )
                        .await
                        {
                            approval.push(step);
                        } else {
                            concurrent.push(step);
                        }
                    }
                    (approval, concurrent)
                };
                #[cfg(not(feature = "human-loop"))]
                let (approval_steps, concurrent_steps): (
                    Vec<(String, String, Value)>,
                    Vec<(String, String, Value)>,
                ) = (Vec::new(), steps);

                // Execute non-approval tools concurrently
                if !concurrent_steps.is_empty() {
                    let max_concurrency = snap.tool_manager.max_concurrency();
                    let tool_names: Vec<&str> =
                        concurrent_steps.iter().map(|(_, n, _)| n.as_str()).collect();
                    info!(
                        agent = %agent,
                        tools = ?tool_names,
                        max_concurrency = ?max_concurrency,
                        "⚡ Streaming concurrent execution of {} tool calls",
                        concurrent_steps.len(),
                    );

                    let futures: Vec<_> = concurrent_steps
                        .iter()
                        .map(|(_, name, args)| {
                            Self::execute_tool_feedback_raw_static(
                                &snap,
                                name,
                                args,
                                snap.tool_error_feedback,
                                agent.clone(),
                            )
                            .instrument(info_span!("tool_execute", tool.name = %name))
                        })
                        .collect();

                    let batch_timeout = super::retry::compute_concurrent_tool_batch_timeout(
                        &snap.tool_execution,
                        futures.len(),
                        max_concurrency,
                    );

                    let results: Vec<
                        std::result::Result<super::execution::ToolExecutionOutcome, ToolExecutionFailure>,
                    >;
                    if let Some(timeout) = batch_timeout {
                        results = try_send!(
                            tx,
                            tokio::time::timeout(timeout, join_all(futures))
                                .await
                                .map_err(|_| ReactError::from(crate::error::ToolError::Timeout(
                                    format!("parallel tool batch exceeded total timeout after {:?}", timeout)
                                )))
                        );
                    } else {
                        results = join_all(futures).await;
                    }

                    for (step, result) in concurrent_steps.into_iter().zip(results) {
                        let tool_call_id = step.0;
                        let function_name = step.1;
                        let tool_result = match result {
                            Ok(outcome) => {
                                Self::apply_hook_messages_static(
                                    &snap,
                                    &function_name,
                                    &outcome.hook_messages,
                                )
                                .await;
                                Ok(Self::truncate_tool_output_static(
                                    &snap,
                                    outcome.output,
                                )
                                .await)
                            }
                            Err(failure) => {
                                Self::apply_hook_messages_static(
                                    &snap,
                                    &function_name,
                                    &failure.hook_messages,
                                )
                                .await;
                                Err(failure.error)
                            }
                        };

                        match tool_result {
                            Ok(output) => {
                                yield_event!(tx, AgentEvent::ToolResult {
                                    name: function_name.clone(),
                                    output: output.clone(),
                                });

                                #[cfg(feature = "chart")]
                                if output.contains("vega-lite")
                                    && let Ok(spec) =
                                        serde_json::from_str::<serde_json::Value>(&output)
                                {
                                    yield_event!(tx, AgentEvent::Chart { spec });
                                }

                                context.lock().await.push(Message::tool_result(
                                    tool_call_id,
                                    function_name.clone(),
                                    output.clone(),
                                ));

                                if function_name == TOOL_FINAL_ANSWER {
                                    Self::auto_snapshot_static(
                                        &snap,
                                        iteration,
                                    )
                                    .await;
                                    for cb in &callbacks {
                                        cb.on_final_answer(&agent, &output).await;
                                    }
                                    info!(agent = %agent, "🏁 Streaming execution completed{label}");

                                    Self::log_final_answer_audit_static(
                                        snap.audit_logger.clone(),
                                        snap.session_id.clone(),
                                        snap.agent_name.clone(),
                                        output.clone(),
                                    )
                                    .await;
                                    Self::save_checkpoint_static(
                                        snap.checkpointer.clone(),
                                        context.clone(),
                                    )
                                    .await;

                                    yield_event!(tx, AgentEvent::FinalAnswer(output));

                                    // Fire Stop hook
                                    {
                                        let hook_ctx =
                                            crate::skills::hooks::HookContext::for_stop(
                                                None,
                                                snap.session_id.as_deref().unwrap_or(""),
                                                &snap.agent_name,
                                                stop_hook_continued,
                                            );
                                        let registry =
                                            snap.hook_registry.read().await.clone();
                                        let stop_result =
                                            registry.run_lifecycle_hooks(&hook_ctx).await;
                                        if stop_result.block {
                                            warn!(agent = %agent, reason = ?stop_result.block_reason, "Stop hook blocked but answer already yielded in stream");
                                        }
                                        if let Some(reason) = &stop_result.continue_reason {
                                            if !stop_hook_continued {
                                                info!(agent = %agent, reason = %reason, "Stop hook requested continuation");
                                                context.lock().await.push(Message::system(
                                                    format!("[Hook:Stop] Continue: {}", reason),
                                                ));
                                                stop_hook_continued = true;
                                                continue; // continue iteration loop
                                            }
                                        }
                                        for msg in &stop_result.messages {
                                            context.lock().await.push(Message::system(
                                                msg.clone(),
                                            ));
                                        }
                                    }

                                    // Fire SessionEnd hook
                                    Self::fire_lifecycle_hook_static(
                                        snap.hook_registry.clone(),
                                        crate::skills::hooks::HookEvent::SessionEnd,
                                        Some("complete"),
                                        snap.session_id.as_deref().unwrap_or(""),
                                        &snap.agent_name,
                                    )
                                    .await;
                                    return Ok(());
                                }
                            }
                            Err(error) => {
                                yield_event!(tx, AgentEvent::ToolError {
                                    name: function_name.clone(),
                                    error: error.to_string(),
                                });

                                context.lock().await.push(Message::tool_result(
                                    tool_call_id,
                                    function_name.clone(),
                                    format!("[Error] {error}"),
                                ));
                            }
                        }
                    }
                }

                // Execute approval tools serially
                for (tool_call_id, function_name, arguments) in approval_steps {
                    match Self::execute_tool_feedback_static(
                        &snap,
                        &function_name,
                        &arguments,
                        agent.clone(),
                    )
                    .await
                    {
                        Ok(output) => {
                            yield_event!(tx, AgentEvent::ToolResult {
                                name: function_name.clone(),
                                output: output.clone(),
                            });

                            #[cfg(feature = "chart")]
                            if output.contains("vega-lite")
                                && let Ok(spec) =
                                    serde_json::from_str::<serde_json::Value>(&output)
                            {
                                yield_event!(tx, AgentEvent::Chart { spec });
                            }

                            context.lock().await.push(Message::tool_result(
                                tool_call_id,
                                function_name.clone(),
                                output.clone(),
                            ));

                            if function_name == TOOL_FINAL_ANSWER {
                                Self::auto_snapshot_static(&snap, iteration).await;
                                for cb in &callbacks {
                                    cb.on_final_answer(&agent, &output).await;
                                }
                                info!(agent = %agent, "🏁 Streaming execution completed{label}");

                                Self::log_final_answer_audit_static(
                                    snap.audit_logger.clone(),
                                    snap.session_id.clone(),
                                    snap.agent_name.clone(),
                                    output.clone(),
                                )
                                .await;
                                Self::save_checkpoint_static(
                                    snap.checkpointer.clone(),
                                    context.clone(),
                                )
                                .await;

                                yield_event!(tx, AgentEvent::FinalAnswer(output));

                                // Fire Stop hook
                                {
                                    let hook_ctx = crate::skills::hooks::HookContext::for_stop(
                                        None,
                                        snap.session_id.as_deref().unwrap_or(""),
                                        &snap.agent_name,
                                        stop_hook_continued,
                                    );
                                    let registry = snap.hook_registry.read().await.clone();
                                    let stop_result =
                                        registry.run_lifecycle_hooks(&hook_ctx).await;
                                    if stop_result.block {
                                        warn!(agent = %agent, reason = ?stop_result.block_reason, "Stop hook blocked but answer already yielded in stream");
                                    }
                                    if let Some(reason) = &stop_result.continue_reason {
                                        if !stop_hook_continued {
                                            info!(agent = %agent, reason = %reason, "Stop hook requested continuation");
                                            context.lock().await.push(Message::system(
                                                format!("[Hook:Stop] Continue: {}", reason),
                                            ));
                                            stop_hook_continued = true;
                                            continue;
                                        }
                                    }
                                    for msg in &stop_result.messages {
                                        context
                                            .lock()
                                            .await
                                            .push(Message::system(msg.clone()));
                                    }
                                }

                                Self::fire_lifecycle_hook_static(
                                    snap.hook_registry.clone(),
                                    crate::skills::hooks::HookEvent::SessionEnd,
                                    Some("complete"),
                                    snap.session_id.as_deref().unwrap_or(""),
                                    &snap.agent_name,
                                )
                                .await;
                                return Ok(());
                            }
                        }
                        Err(error) => {
                            yield_event!(tx, AgentEvent::ToolError {
                                name: function_name.clone(),
                                error: error.to_string(),
                            });

                            context.lock().await.push(Message::tool_result(
                                tool_call_id,
                                function_name.clone(),
                                format!("[Error] {error}"),
                            ));
                        }
                    }
                }

                Self::auto_snapshot_static(&snap, iteration).await;
            } else if !content_buffer.is_empty() {
                // Plain text response
                let think_steps = vec![StepType::Thought(content_buffer.clone())];
                for cb in &callbacks {
                    cb.on_think_end(&agent, &think_steps, prompt_tokens, completion_tokens)
                        .await;
                }
                for cb in &callbacks {
                    cb.on_final_answer(&agent, &content_buffer).await;
                }
                context
                    .lock()
                    .await
                    .push(Message::assistant(content_buffer.clone()));

                Self::auto_snapshot_static(&snap, iteration).await;
                Self::log_final_answer_audit_static(
                    snap.audit_logger.clone(),
                    snap.session_id.clone(),
                    snap.agent_name.clone(),
                    content_buffer.clone(),
                )
                .await;
                Self::save_checkpoint_static(snap.checkpointer.clone(), context.clone()).await;

                yield_event!(tx, AgentEvent::FinalAnswer(content_buffer));

                // Fire Stop hook
                {
                    let hook_ctx = crate::skills::hooks::HookContext::for_stop(
                        None,
                        snap.session_id.as_deref().unwrap_or(""),
                        &snap.agent_name,
                        stop_hook_continued,
                    );
                    let registry = snap.hook_registry.read().await.clone();
                    let stop_result = registry.run_lifecycle_hooks(&hook_ctx).await;
                    if stop_result.block {
                        warn!(agent = %agent, reason = ?stop_result.block_reason, "Stop hook blocked but answer already yielded in stream");
                    }
                    if let Some(reason) = &stop_result.continue_reason {
                        if !stop_hook_continued {
                            info!(agent = %agent, reason = %reason, "Stop hook requested continuation");
                            context.lock().await.push(Message::system(format!(
                                "[Hook:Stop] Continue: {}",
                                reason
                            )));
                            stop_hook_continued = true;
                            continue;
                        }
                    }
                    for msg in &stop_result.messages {
                        context
                            .lock()
                            .await
                            .push(Message::system(msg.clone()));
                    }
                }

                Self::fire_lifecycle_hook_static(
                    snap.hook_registry.clone(),
                    crate::skills::hooks::HookEvent::SessionEnd,
                    Some("complete"),
                    snap.session_id.as_deref().unwrap_or(""),
                    &snap.agent_name,
                )
                .await;
                return Ok(());
            } else {
                let _ = tx.send(Err(ReactError::Agent(Box::new(AgentError::NoResponse {
                    model: snap.model_name.clone(),
                    agent: snap.agent_name.clone(),
                }))));
                return Ok(());
            }
        }

        // Max iterations exceeded
        Self::fire_lifecycle_hook_static(
            snap.hook_registry.clone(),
            crate::skills::hooks::HookEvent::SessionEnd,
            Some("max_iterations"),
            snap.session_id.as_deref().unwrap_or(""),
            &snap.agent_name,
        )
        .await;

        let sf_result = Self::fire_lifecycle_hook_static(
            snap.hook_registry.clone(),
            crate::skills::hooks::HookEvent::StopFailure,
            Some("max_iterations"),
            snap.session_id.as_deref().unwrap_or(""),
            &snap.agent_name,
        )
        .await;
        if !sf_result.messages.is_empty() || sf_result.injected_context.is_some() {
            warn!(agent = %agent, "StopFailure hook (max_iterations) produced output that cannot be injected (terminal path)");
        }

        let _ = tx.send(Err(ReactError::Agent(Box::new(
            AgentError::MaxIterationsExceeded(snap.max_iterations),
        ))));
        Ok(())
    }
}
