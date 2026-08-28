//! Tool execution pipeline — composable stages for tool call processing.
//!
//! Replaces the monolithic `execute_tool_feedback_raw` with a configurable
//! pipeline of discrete internal stages coordinated by `ToolExecutionPipeline`.

use super::context::HookMessageBatches;
use crate::agent::{ToolInvocation, ToolInvocationRewrite};
use crate::error::{ReactError, Result};
use crate::tools::{ToolParameters, ToolResult, ToolStreamEvent, is_write_tool};
use async_trait::async_trait;
use echo_core::tools::permission::{PermissionDecision, PermissionMode};
use serde_json::Value;
use tokio::sync::mpsc;
use tracing::debug;

// Complete tool results stay inline so the pipeline preserves the same typed
// stream event without adding an allocation to every streamed item.
#[allow(clippy::large_enum_variant)]
pub(crate) enum ToolPipelineEvent {
    Invocation {
        call_id: String,
        invocation: ToolInvocation,
    },
    Stream {
        call_id: String,
        name: String,
        event: ToolStreamEvent,
    },
}

// ── ToolExecutionContext ────────────────────────────────────────────

/// Mutable context that flows through the pipeline stages.
pub(crate) struct ToolExecutionContext {
    /// Unique call ID linking the canonical ToolCall and typed ToolResult.
    pub call_id: String,
    /// Original model-requested tool name.
    pub requested_tool_name: String,
    /// Original model-requested arguments.
    pub requested_input: Value,
    /// Name of the tool being executed.
    pub tool_name: String,
    /// Parsed tool parameters.
    pub params: ToolParameters,
    /// Raw JSON input from the LLM.
    pub input: Value,
    /// Messages accumulated by hooks (pre/post).
    pub hook_messages: HookMessageBatches,
    /// Filled by the Execute stage.
    pub result: Option<ToolResult>,
    /// Final output string (after output guard + truncation).
    pub output: Option<String>,
    /// Whether a stage has blocked execution.
    pub blocked: bool,
    /// Reason for blocking (if blocked).
    pub block_reason: Option<String>,
    /// Structured terminal facts when execution is blocked before the tool starts.
    pub block_failure: Option<crate::tools::ToolFailure>,
    /// Execution duration in milliseconds.
    pub duration_ms: u64,
    /// Whether the agent is in plan mode (read-only tools only).
    pub plan_mode: bool,
    /// Permission decision returned by PreToolUse hooks for this call only.
    pub permission_decision: Option<PermissionDecision>,
    /// Permission mode override returned by hooks for this call only.
    pub permission_mode_override: Option<PermissionMode>,
    /// Ordered provenance for policy rewrites applied before execution.
    pub rewrites: Vec<ToolInvocationRewrite>,
    /// Whether the canonical invocation event has been emitted.
    pub invocation_emitted: bool,
    /// Incremental tool events, tagged with their stable invocation identity.
    pub stream_tx: Option<mpsc::Sender<ToolPipelineEvent>>,
}

impl ToolExecutionContext {
    fn block(&mut self, category: crate::tools::ToolFailureCategory, reason: String) {
        self.blocked = true;
        self.block_reason = Some(reason.clone());
        self.block_failure = Some(crate::tools::ToolFailure::new(category));
        self.output = Some(reason);
    }

    fn replace_input(&mut self, input: Value, rewrite: ToolInvocationRewrite) -> Result<()> {
        let Value::Object(map) = &input else {
            return Err(ReactError::Other(format!(
                "Tool '{}' rewrite returned non-object arguments",
                self.tool_name
            )));
        };
        if input != self.input {
            self.rewrites.push(rewrite);
        }
        self.params = map
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        self.input = input;
        Ok(())
    }

    async fn emit_invocation(&mut self) -> Result<()> {
        if self.invocation_emitted {
            return Ok(());
        }
        if let Some(stream_tx) = self.stream_tx.as_ref() {
            stream_tx
                .send(ToolPipelineEvent::Invocation {
                    call_id: self.call_id.clone(),
                    invocation: ToolInvocation {
                        requested_name: self.requested_tool_name.clone(),
                        requested_args: self.requested_input.clone(),
                        name: self.tool_name.clone(),
                        args: self.input.clone(),
                        rewrites: self.rewrites.clone(),
                    },
                })
                .await
                .map_err(|_| ReactError::Other("Tool event receiver closed".to_string()))?;
        }
        self.invocation_emitted = true;
        Ok(())
    }
}

// ── PipelineStage trait ────────────────────────────────────────────

/// A single stage in the tool execution pipeline.
///
/// Each stage receives a mutable [`ToolExecutionContext`] and may modify it.
/// Returning `Err` short-circuits the pipeline (tool execution failure).
/// Setting `ctx.blocked = true` causes subsequent stages to be skipped.
#[async_trait]
pub(crate) trait PipelineStage: Send + Sync {
    /// Human-readable name for logging and debugging.
    fn name(&self) -> &str;

    /// Execute this stage.
    async fn run(
        &self,
        ctx: &mut ToolExecutionContext,
        snapshot: &crate::agent::snapshot::AgentRunSnapshot,
    ) -> Result<()>;
}

// ── Stage implementations ──────────────────────────────────────────

/// Intervention callback stage — checks intervention callbacks before all other stages.
///
/// This is the highest-priority decision point. Intervention callbacks can
/// block, cancel, redirect, modify arguments, or inject context.
pub struct InterventionStage;

#[async_trait]
impl PipelineStage for InterventionStage {
    fn name(&self) -> &str {
        "intervention"
    }

    async fn run(
        &self,
        ctx: &mut ToolExecutionContext,
        snapshot: &crate::agent::snapshot::AgentRunSnapshot,
    ) -> Result<()> {
        for intervention in &snapshot.tools.intervention_callbacks {
            let result = intervention
                .on_tool_call(&snapshot.config.agent_name, &ctx.tool_name, &ctx.input)
                .await;
            if result.cancel {
                return Err(ReactError::Other(format!(
                    "Agent execution cancelled by intervention: tool {}",
                    ctx.tool_name
                )));
            }
            if result.block {
                let reason = result
                    .block_reason
                    .unwrap_or_else(|| "blocked by intervention callback".into());
                ctx.block(
                    crate::tools::ToolFailureCategory::Permanent,
                    format!("Tool {} blocked by intervention: {}", ctx.tool_name, reason),
                );
                return Ok(());
            }
            if let Some(redirect) = result.redirect_to {
                if redirect != ctx.tool_name {
                    ctx.rewrites
                        .push(ToolInvocationRewrite::InterventionRedirect);
                }
                ctx.tool_name = redirect;
            }
            if let Some(modified) = result.modified_args {
                ctx.replace_input(modified, ToolInvocationRewrite::InterventionArguments)?;
            }
            if let Some(injected) = result.injected_context {
                ctx.hook_messages.pre.push(injected);
            }
        }
        Ok(())
    }
}

/// Enforces the invocation-scoped tool surface at execution time.
pub struct ToolVisibilityStage;

#[async_trait]
impl PipelineStage for ToolVisibilityStage {
    fn name(&self) -> &str {
        "tool_visibility"
    }

    async fn run(
        &self,
        ctx: &mut ToolExecutionContext,
        snapshot: &crate::agent::snapshot::AgentRunSnapshot,
    ) -> Result<()> {
        if snapshot.tools.disabled_tools.contains(&ctx.tool_name) {
            snapshot.tools.tool_manager.record_tool_selection_failure();
            ctx.block(
                crate::tools::ToolFailureCategory::Unavailable,
                format!(
                    "Tool '{}' is not available in this invocation",
                    ctx.tool_name
                ),
            );
        } else if snapshot
            .tools
            .visibility
            .as_ref()
            .is_some_and(|visibility| !visibility.is_visible(&ctx.tool_name))
        {
            snapshot.tools.tool_manager.record_tool_selection_failure();
            ctx.block(
                crate::tools::ToolFailureCategory::Unavailable,
                format!(
                    "Tool '{}' is not activated in this invocation; use tool_search first",
                    ctx.tool_name
                ),
            );
        }
        Ok(())
    }
}

/// Runs PreToolUse hooks.
pub struct PreToolUseHookStage;

#[async_trait]
impl PipelineStage for PreToolUseHookStage {
    fn name(&self) -> &str {
        "pre_tool_use_hook"
    }

    async fn run(
        &self,
        ctx: &mut ToolExecutionContext,
        snapshot: &crate::agent::snapshot::AgentRunSnapshot,
    ) -> Result<()> {
        let has_hooks = {
            let hook_reg = snapshot.tools.hook_registry.read().await;
            !hook_reg.is_empty()
        };
        if !has_hooks {
            return Ok(());
        }

        let hook_reg = {
            let guard = snapshot.tools.hook_registry.read().await;
            guard.clone()
        };
        let hook_result = hook_reg
            .run_pre_tool_use(
                &ctx.tool_name,
                &ctx.input,
                snapshot.config.session_id.as_deref().unwrap_or(""),
            )
            .await;
        ctx.hook_messages.pre = hook_result.messages.clone();
        ctx.permission_decision = hook_result.permission_decision.clone();
        ctx.permission_mode_override = hook_result.permission_mode_override;

        if hook_result.block {
            let reason = hook_result
                .block_reason
                .unwrap_or_else(|| format!("Tool {} blocked by hook", ctx.tool_name));
            ctx.block(crate::tools::ToolFailureCategory::Permanent, reason);
            return Ok(());
        }

        if let Some(updated) = hook_result.updated_input {
            ctx.replace_input(updated, ToolInvocationRewrite::PreToolUseHook)?;
        }
        Ok(())
    }
}

/// Checks tool approval (PermissionService).
pub struct PermissionStage;

#[async_trait]
impl PipelineStage for PermissionStage {
    fn name(&self) -> &str {
        "permission"
    }

    async fn run(
        &self,
        ctx: &mut ToolExecutionContext,
        snapshot: &crate::agent::snapshot::AgentRunSnapshot,
    ) -> Result<()> {
        if let Some(decision) = ctx.permission_decision.take() {
            match decision {
                PermissionDecision::Allow => return Ok(()),
                PermissionDecision::Deny { reason } => {
                    ctx.block(crate::tools::ToolFailureCategory::Permanent, reason);
                    return Ok(());
                }
                PermissionDecision::Ask { .. } | PermissionDecision::RequireApproval => {}
            }
        }

        let permission_hook = {
            let registry = snapshot.tools.hook_registry.read().await.clone();
            let context = crate::skills::hooks::HookContext::for_permission_request(
                &ctx.tool_name,
                &ctx.input,
                snapshot.config.session_id.as_deref().unwrap_or(""),
                &snapshot.config.agent_name,
            );
            registry.run_lifecycle_hooks(&context).await
        };
        ctx.hook_messages.pre.extend(permission_hook.messages);
        if permission_hook.block {
            let reason = permission_hook
                .block_reason
                .unwrap_or_else(|| format!("Tool {} blocked by permission hook", ctx.tool_name));
            ctx.block(crate::tools::ToolFailureCategory::Permanent, reason);
            return Ok(());
        }
        if let Some(mode) = permission_hook.permission_mode_override {
            ctx.permission_mode_override = Some(mode);
        }
        if let Some(decision) = permission_hook.permission_decision {
            match decision {
                PermissionDecision::Allow => return Ok(()),
                PermissionDecision::Deny { reason } => {
                    ctx.block(crate::tools::ToolFailureCategory::Permanent, reason);
                    return Ok(());
                }
                PermissionDecision::Ask { .. } | PermissionDecision::RequireApproval => {}
            }
        }

        #[cfg(feature = "human-loop")]
        let approval_modified = snapshot
            .check_tool_approval(
                &ctx.call_id,
                &ctx.tool_name,
                &ctx.input,
                ctx.permission_mode_override,
            )
            .await
            .map_err(|error| ReactError::Other(error.to_string()))?;

        #[cfg(not(feature = "human-loop"))]
        let approval_modified = snapshot
            .check_tool_approval(
                &ctx.call_id,
                &ctx.tool_name,
                &ctx.input,
                ctx.permission_mode_override,
            )
            .await
            .map_err(|_| ReactError::Other("Permission check failed".into()))?;

        if let Some(modified) = approval_modified {
            ctx.replace_input(modified, ToolInvocationRewrite::Approval)?;
        }
        Ok(())
    }
}

/// Enforces read-before-edit policy.
pub struct ReadBeforeEditStage;

#[async_trait]
impl PipelineStage for ReadBeforeEditStage {
    fn name(&self) -> &str {
        "read_before_edit"
    }

    async fn run(
        &self,
        ctx: &mut ToolExecutionContext,
        snapshot: &crate::agent::snapshot::AgentRunSnapshot,
    ) -> Result<()> {
        if !snapshot.config.force_read_before_edit {
            return Ok(());
        }
        if !is_write_tool(&ctx.tool_name) {
            return Ok(());
        }
        let paths = extract_path_params(&ctx.tool_name, &ctx.params);
        let ttl = std::time::Duration::from_secs(30 * 60);
        let mut files = snapshot
            .recently_read_files
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        for path in paths {
            let canonical = resolve_tracking_path(&path, snapshot.config.working_dir.as_deref());
            let read = match files.get(&canonical) {
                Some(instant) if instant.elapsed() < ttl => true,
                Some(_) => {
                    files.remove(&canonical);
                    false
                }
                None => false,
            };
            if !read {
                ctx.block(crate::tools::ToolFailureCategory::Unavailable, format!(
                    "Read-before-edit is enabled. File '{path}' has not been read. Use read_file first."
                ));
                break;
            }
        }
        Ok(())
    }
}

/// Checks skill-based tool permissions (skill_allowed_tools whitelist).
pub struct SkillPermissionStage;

#[async_trait]
impl PipelineStage for SkillPermissionStage {
    fn name(&self) -> &str {
        "skill_permission"
    }

    async fn run(
        &self,
        ctx: &mut ToolExecutionContext,
        snapshot: &crate::agent::snapshot::AgentRunSnapshot,
    ) -> Result<()> {
        // Check if a skill is activated and has tool restrictions
        if !snapshot.tools.is_skill_tool_allowed(&ctx.tool_name) {
            ctx.block(
                crate::tools::ToolFailureCategory::Unavailable,
                format!(
                    "Tool '{}' is not permitted by the activated skill's allowed_tools whitelist",
                    ctx.tool_name
                ),
            );
        }
        Ok(())
    }
}

/// Records business audit logs for tool execution.
pub struct AuditStage;

#[async_trait]
impl PipelineStage for AuditStage {
    fn name(&self) -> &str {
        "audit"
    }

    async fn run(
        &self,
        ctx: &mut ToolExecutionContext,
        snapshot: &crate::agent::snapshot::AgentRunSnapshot,
    ) -> Result<()> {
        // Log tool execution start to audit logger
        if let Some(al) = &snapshot.guard.audit_logger {
            let ev = crate::audit::AuditEvent::now(
                snapshot.config.session_id.clone(),
                snapshot.config.agent_name.clone(),
                crate::audit::AuditEventType::ToolCall {
                    tool: ctx.tool_name.clone(),
                    input: ctx.input.clone(),
                    output: String::new(),
                    success: true,
                    duration_ms: 0,
                },
            );
            if let Err(e) = al.log(ev).await {
                tracing::error!(error = %e, "audit log write failed — event dropped");
            }
        }
        Ok(())
    }
}

/// Publishes the one canonical requested/effective invocation before execution.
pub struct InvocationStage;

#[async_trait]
impl PipelineStage for InvocationStage {
    fn name(&self) -> &str {
        "invocation"
    }

    async fn run(
        &self,
        ctx: &mut ToolExecutionContext,
        _snapshot: &crate::agent::snapshot::AgentRunSnapshot,
    ) -> Result<()> {
        ctx.emit_invocation().await
    }
}

/// Executes the tool via ToolManager.
pub struct ExecuteStage;

#[async_trait]
impl PipelineStage for ExecuteStage {
    fn name(&self) -> &str {
        "execute"
    }

    async fn run(
        &self,
        ctx: &mut ToolExecutionContext,
        snapshot: &crate::agent::snapshot::AgentRunSnapshot,
    ) -> Result<()> {
        let execution_start = std::time::Instant::now();

        if ctx.call_id.is_empty() {
            ctx.call_id = format!("call_{}", uuid::Uuid::new_v4());
        }
        // Record ToolCall trace event (redaction handled by new_tool_call)
        snapshot
            .record_event(crate::trace::RunEvent::new_tool_call(
                ctx.call_id.clone(),
                ctx.tool_name.clone(),
                Some(ctx.input.clone()),
                None,
                0,
            ))
            .await;

        // Execute tool; on infrastructure error, convert to ToolResult{success:false}
        // so downstream stages (trace, post-hook, callback) still execute.
        //
        // Build a per-agent ToolContext from the snapshot's RuntimeConfig so
        // the (shared, stateless) ToolManager receives the correct working_dir
        // for THIS agent/session — avoiding cross-session cwd contamination.
        let tool_ctx = echo_core::tools::ToolContext {
            working_dir: snapshot.config.working_dir.clone(),
            conversation_id: snapshot.config.conversation_id.clone(),
            run_id: snapshot.current_run_id.clone(),
            turn_id: snapshot.current_turn_id.clone(),
            message_id: snapshot.current_message_id.clone(),
            execution_id: snapshot.current_execution_id.clone(),
            call_id: Some(ctx.call_id.clone()),
            active_message: snapshot.current_message.clone(),
            output_artifacts: snapshot.config.tool_output_artifacts.clone(),
            tool_visibility: snapshot.tools.visibility.clone(),
            script_execution_profile: None,
            cancel: snapshot.external_cancel.clone(),
            trace_sink: snapshot.external_trace_sink.clone(),
            delegation_policy: snapshot.external_delegation_policy,
            resource_guards: snapshot.resource_guards.clone(),
        };
        let execution_result = if snapshot
            .tools
            .tool_manager
            .supports_streaming(&ctx.tool_name)
        {
            if let Some(stream_tx) = ctx.stream_tx.as_ref() {
                let (event_tx, mut event_rx) = mpsc::channel(64);
                let mut execution = Box::pin(
                    snapshot
                        .tools
                        .tool_manager
                        .execute_tool_stream_with_context_draining_started(
                            &ctx.tool_name,
                            ctx.params.clone(),
                            &tool_ctx,
                            Some(event_tx),
                        ),
                );
                let mut stream_open = true;
                let result = loop {
                    tokio::select! {
                        biased;
                        event = event_rx.recv(), if stream_open => {
                            match event {
                                Some(event) => {
                                    if !matches!(event, ToolStreamEvent::Complete(_)) {
                                        stream_tx
                                            .send(ToolPipelineEvent::Stream {
                                                call_id: ctx.call_id.clone(),
                                                name: ctx.tool_name.clone(),
                                                event,
                                            })
                                            .await
                                            .map_err(|_| ReactError::Other("Tool stream receiver closed".into()))?;
                                    }
                                }
                                None => stream_open = false,
                            }
                        }
                        result = &mut execution => break result,
                    }
                };
                while let Some(event) = event_rx.recv().await {
                    if !matches!(event, ToolStreamEvent::Complete(_)) {
                        stream_tx
                            .send(ToolPipelineEvent::Stream {
                                call_id: ctx.call_id.clone(),
                                name: ctx.tool_name.clone(),
                                event,
                            })
                            .await
                            .map_err(|_| ReactError::Other("Tool stream receiver closed".into()))?;
                    }
                }
                result
            } else {
                snapshot
                    .tools
                    .tool_manager
                    .execute_tool_stream_with_context_draining_started(
                        &ctx.tool_name,
                        ctx.params.clone(),
                        &tool_ctx,
                        None,
                    )
                    .await
            }
        } else {
            snapshot
                .tools
                .tool_manager
                .execute_tool_with_context_draining_started(
                    &ctx.tool_name,
                    ctx.params.clone(),
                    &tool_ctx,
                )
                .await
        };

        let may_have_side_effects = snapshot
            .tools
            .tool_manager
            .get_tool(&ctx.tool_name)
            .is_none_or(|tool| tool.risk_level() != crate::tools::ToolRiskLevel::ReadOnly);
        let result = match execution_result {
            Ok(r) => r,
            Err(e) => {
                let err_msg = e.to_string();
                // Log failure to audit logger
                if let Some(al) = &snapshot.guard.audit_logger {
                    let ev = crate::audit::AuditEvent::now(
                        snapshot.config.session_id.clone(),
                        snapshot.config.agent_name.clone(),
                        crate::audit::AuditEventType::ToolCall {
                            tool: ctx.tool_name.clone(),
                            input: ctx.input.clone(),
                            output: err_msg.clone(),
                            success: false,
                            duration_ms: 0,
                        },
                    );
                    if let Err(e) = al.log(ev).await {
                        tracing::error!(error = %e, "audit log write failed — event dropped");
                    }
                }

                ToolResult {
                    kind: echo_core::tools::ToolResultKind::StructuredError {
                        error_code: "tool_execution_failed".into(),
                    },
                    success: false,
                    output: String::new(),
                    error: Some(err_msg),
                    failure: Some(crate::tools::ToolFailure::from_error(
                        &e,
                        may_have_side_effects,
                    )),
                    data: None,
                    truncated: false,
                    mime_type: None,
                    artifact: None,
                    metadata: std::collections::HashMap::new(),
                    model_content: Vec::new(),
                }
            }
        };

        ctx.duration_ms = u64::try_from(execution_start.elapsed().as_millis()).unwrap_or(u64::MAX);
        #[cfg(feature = "telemetry")]
        {
            crate::telemetry::Metrics::record_tool_execution(
                &ctx.tool_name,
                if result.success { "success" } else { "error" },
            );
            crate::telemetry::Metrics::record_tool_latency(
                &ctx.tool_name,
                execution_start.elapsed().as_secs_f64() * 1000.0,
            );
        }
        ctx.result = Some(result.clone());

        // Record file read if tool was read_file and succeeded
        if result.success
            && ctx.tool_name == "read_file"
            && let Some(path) = ctx.params.get("path").and_then(|v| v.as_str())
        {
            snapshot.record_file_read(path);
        }

        Ok(())
    }
}

/// Runs PostToolUse or PostToolUseFailure hooks according to the tool result.
pub struct PostToolUseHookStage;

#[async_trait]
impl PipelineStage for PostToolUseHookStage {
    fn name(&self) -> &str {
        "post_tool_use_hook"
    }

    async fn run(
        &self,
        ctx: &mut ToolExecutionContext,
        snapshot: &crate::agent::snapshot::AgentRunSnapshot,
    ) -> Result<()> {
        let hook_reg = {
            let guard = snapshot.tools.hook_registry.read().await;
            if guard.is_empty() {
                return Ok(());
            }
            guard.clone()
        };
        let Some(tool_result) = ctx.result.as_ref() else {
            return Ok(());
        };
        let post_result = if tool_result.success {
            hook_reg
                .run_post_tool_use(
                    &ctx.tool_name,
                    &ctx.input,
                    &tool_result.output,
                    snapshot.config.session_id.as_deref().unwrap_or(""),
                )
                .await
        } else {
            let error = tool_result
                .error
                .as_deref()
                .filter(|value| !value.is_empty())
                .unwrap_or(&tool_result.output);
            hook_reg
                .run_post_tool_use_failure(
                    &ctx.tool_name,
                    &ctx.input,
                    error,
                    snapshot.config.session_id.as_deref().unwrap_or(""),
                )
                .await
        };
        ctx.hook_messages.post = post_result.messages;
        if post_result.block {
            let reason = post_result.block_reason.unwrap_or_else(|| {
                format!("Tool {} output blocked by post-use hook", ctx.tool_name)
            });
            ctx.block(crate::tools::ToolFailureCategory::PartialSideEffect, reason);
        }
        Ok(())
    }
}

/// Runs output guard checks.
pub struct OutputGuardStage;

#[async_trait]
impl PipelineStage for OutputGuardStage {
    fn name(&self) -> &str {
        "output_guard"
    }

    async fn run(
        &self,
        ctx: &mut ToolExecutionContext,
        snapshot: &crate::agent::snapshot::AgentRunSnapshot,
    ) -> Result<()> {
        if let Some(ref result) = ctx.result
            && result.success
            && let Some(guarded) = snapshot.check_tool_output_guard(&result.output).await
        {
            ctx.output = Some(guarded);
        }
        Ok(())
    }
}

/// Truncates tool output based on token budget.
pub struct TruncationStage;

#[async_trait]
impl PipelineStage for TruncationStage {
    fn name(&self) -> &str {
        "truncation"
    }

    async fn run(
        &self,
        ctx: &mut ToolExecutionContext,
        snapshot: &crate::agent::snapshot::AgentRunSnapshot,
    ) -> Result<()> {
        let raw = ctx
            .output
            .as_deref()
            .or_else(|| ctx.result.as_ref().map(|r| r.output.as_str()))
            .unwrap_or("");
        let existing_artifact = ctx
            .result
            .as_ref()
            .and_then(|result| result.artifact.clone());
        let processed = snapshot.process_tool_output_for_call(
            raw.to_string(),
            &ctx.call_id,
            &ctx.tool_name,
            existing_artifact,
        );
        if let Some(result) = ctx.result.as_mut() {
            result.truncated = result.truncated || processed.truncated;
            result.artifact = processed.artifact;
            result.metadata.extend(processed.metadata);
            snapshot.tools.tool_manager.record_tool_result(
                &ctx.tool_name,
                result,
                processed.output.len(),
                ctx.duration_ms,
            );
        }
        ctx.output = Some(processed.output);
        Ok(())
    }
}

/// Fires callbacks (on_tool_start, on_tool_end, on_tool_error).
pub struct CallbackStage {
    phase: CallbackPhase,
}

pub enum CallbackPhase {
    Start,
    End,
}

impl CallbackStage {
    pub const START: Self = Self {
        phase: CallbackPhase::Start,
    };
    pub const END: Self = Self {
        phase: CallbackPhase::End,
    };
}

#[async_trait]
impl PipelineStage for CallbackStage {
    fn name(&self) -> &str {
        match self.phase {
            CallbackPhase::Start => "callback_start",
            CallbackPhase::End => "callback_end",
        }
    }

    async fn run(
        &self,
        ctx: &mut ToolExecutionContext,
        snapshot: &crate::agent::snapshot::AgentRunSnapshot,
    ) -> Result<()> {
        let agent_name = &snapshot.config.agent_name;
        for cb in &snapshot.config.callbacks {
            match self.phase {
                CallbackPhase::Start => {
                    cb.on_tool_start(agent_name, &ctx.tool_name, &ctx.input)
                        .await;
                }
                CallbackPhase::End => {
                    let output = ctx
                        .output
                        .as_deref()
                        .or_else(|| ctx.result.as_ref().map(|r| r.output.as_str()))
                        .unwrap_or("");
                    cb.on_tool_end(agent_name, &ctx.tool_name, output).await;
                }
            }
        }
        Ok(())
    }
}

/// Records trace events for tool results/errors.
pub struct TraceRecordingStage;

#[async_trait]
impl PipelineStage for TraceRecordingStage {
    fn name(&self) -> &str {
        "trace_recording"
    }

    async fn run(
        &self,
        ctx: &mut ToolExecutionContext,
        snapshot: &crate::agent::snapshot::AgentRunSnapshot,
    ) -> Result<()> {
        if let Some(ref result) = ctx.result {
            // Always record ToolResult — success determines the outcome.
            // ToolError is additionally recorded for failure details.
            snapshot
                .record_event(crate::trace::RunEvent::ToolResult {
                    call_id: ctx.call_id.clone(),
                    name: ctx.tool_name.clone(),
                    success: result.success,
                    output_preview: Some(if result.success {
                        ctx.output
                            .as_deref()
                            .unwrap_or(&result.output)
                            .chars()
                            .take(200)
                            .collect()
                    } else {
                        result
                            .error
                            .clone()
                            .unwrap_or_default()
                            .chars()
                            .take(200)
                            .collect()
                    }),
                    output_truncated: result.truncated,
                    duration_ms: ctx.duration_ms,
                    original_bytes: metadata_u64(result, "original_bytes"),
                    returned_bytes: metadata_u64(result, "returned_bytes"),
                    estimated_tokens: metadata_usize(result, "estimated_tokens"),
                    output_handling: result.metadata.get("output_handling").cloned(),
                    artifact: result.artifact.clone(),
                })
                .await;
            if !result.success {
                snapshot
                    .record_event(crate::trace::RunEvent::ToolError {
                        call_id: ctx.call_id.clone(),
                        name: ctx.tool_name.clone(),
                        message: result.error.clone().unwrap_or_default(),
                        failure: result.failure.clone(),
                    })
                    .await;
            }
        }
        Ok(())
    }
}

fn metadata_u64(result: &crate::tools::ToolResult, key: &str) -> u64 {
    result
        .metadata
        .get(key)
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0)
}

fn metadata_usize(result: &crate::tools::ToolResult, key: &str) -> usize {
    result
        .metadata
        .get(key)
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0)
}

// ── ToolExecutionPipeline ──────────────────────────────────────────

/// Composable pipeline for tool execution stages.
///
/// # Example
///
/// ```rust,ignore
/// let pipeline = ToolExecutionPipeline::default();
/// let mut ctx = ToolExecutionContext { ... };
/// pipeline.run(&mut ctx, &agent).await?;
/// ```
pub struct ToolExecutionPipeline {
    stages: Vec<Box<dyn PipelineStage>>,
}

impl ToolExecutionPipeline {
    /// Build a pipeline with all standard stages in the correct order.
    pub fn default_pipeline() -> Self {
        Self {
            stages: vec![
                Box::new(InterventionStage),
                Box::new(ToolVisibilityStage),
                Box::new(PlanModeStage),
                Box::new(PreToolUseHookStage),
                Box::new(PermissionStage),
                Box::new(ReadBeforeEditStage),
                Box::new(SkillPermissionStage),
                Box::new(InvocationStage),
                Box::new(CallbackStage::START),
                Box::new(AuditStage),
                Box::new(ExecuteStage),
                Box::new(PostToolUseHookStage),
                Box::new(OutputGuardStage),
                Box::new(TruncationStage),
                Box::new(TraceRecordingStage),
                Box::new(CallbackStage::END),
            ],
        }
    }

    /// Run all stages in order. Short-circuits if `ctx.blocked` is set.
    pub(crate) async fn run(
        &self,
        ctx: &mut ToolExecutionContext,
        snapshot: &crate::agent::snapshot::AgentRunSnapshot,
    ) -> Result<()> {
        for stage in &self.stages {
            if ctx.blocked {
                ctx.emit_invocation().await?;
                debug!(
                    agent = %snapshot.config.agent_name,
                    stage = stage.name(),
                    reason = ?ctx.block_reason,
                    "Pipeline stage skipped (blocked)"
                );
                break;
            }
            debug!(
                agent = %snapshot.config.agent_name,
                stage = stage.name(),
                tool = %ctx.tool_name,
                "Pipeline stage running"
            );
            if let Err(error) = stage.run(ctx, snapshot).await {
                ctx.emit_invocation().await?;
                return Err(error);
            }
        }
        Ok(())
    }
}

// ── PlanModeStage ──────────────────────────────────────────────────

/// In plan mode, blocks write/destructive tools — the agent can only read and analyze.
pub struct PlanModeStage;

#[async_trait]
impl PipelineStage for PlanModeStage {
    fn name(&self) -> &str {
        "plan_mode"
    }

    async fn run(
        &self,
        ctx: &mut ToolExecutionContext,
        _snapshot: &crate::agent::snapshot::AgentRunSnapshot,
    ) -> Result<()> {
        if !ctx.plan_mode {
            return Ok(());
        }
        if is_write_tool(&ctx.tool_name)
            || ctx.tool_name == "shell"
            || ctx.tool_name == "delete_file"
        {
            ctx.block(crate::tools::ToolFailureCategory::Unavailable, format!(
                "Plan mode: '{}' is blocked. Read and analyze only. Use /plan off to enable writes.",
                ctx.tool_name
            ));
        }
        Ok(())
    }
}

impl Default for ToolExecutionPipeline {
    fn default() -> Self {
        Self::default_pipeline()
    }
}

// ── Helpers ────────────────────────────────────────────────────────

fn extract_path_params(tool_name: &str, params: &ToolParameters) -> Vec<String> {
    if tool_name == "apply_patch" {
        #[cfg(feature = "files")]
        return params
            .get("patch")
            .and_then(Value::as_str)
            .and_then(|patch| echo_tools::files::apply_patch::existing_file_paths(patch).ok())
            .unwrap_or_default();
        #[cfg(not(feature = "files"))]
        return Vec::new();
    }
    params
        .get("path")
        .or_else(|| params.get("file_path"))
        .and_then(Value::as_str)
        .map(|path| vec![path.to_string()])
        .unwrap_or_default()
}

fn resolve_tracking_path(path: &str, working_dir: Option<&std::path::Path>) -> String {
    let path = std::path::Path::new(path);
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else if let Some(working_dir) = working_dir {
        working_dir.join(path)
    } else {
        path.to_path_buf()
    };
    std::fs::canonicalize(&resolved)
        .unwrap_or(resolved)
        .to_string_lossy()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentEvent;
    use echo_core::tools::{InvocationResourceGuard, Tool, ToolContext, ToolOutputChannel};
    use futures::Stream;
    #[cfg(feature = "files")]
    use std::collections::HashMap;
    use std::pin::Pin;
    use std::sync::Arc;

    #[test]
    fn test_is_write_tool() {
        assert!(is_write_tool("apply_patch"));
        assert!(is_write_tool("write_file"));
        assert!(!is_write_tool("read_file"));
    }

    #[test]
    fn test_extract_write_path_param() {
        let params = vec![("path".to_string(), Value::String("src/main.rs".to_string()))]
            .into_iter()
            .collect();
        assert_eq!(
            extract_path_params("write_file", &params),
            vec!["src/main.rs".to_string()]
        );
    }

    #[cfg(feature = "files")]
    #[test]
    fn test_extract_apply_patch_paths() {
        let patch_params = HashMap::from([(
            "patch".to_string(),
            Value::String(
                "*** Begin Patch\n*** Update File: src/lib.rs\n@@\n-old\n+new\n*** Delete File: stale.txt\n*** End Patch"
                    .to_string(),
            ),
        )]);
        assert_eq!(
            extract_path_params("apply_patch", &patch_params),
            vec!["src/lib.rs".to_string(), "stale.txt".to_string()]
        );
    }

    struct InterleavingTool;

    struct InvalidResultTool;

    struct ResourceGuardStreamingTool {
        observed_guard_count: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl Tool for InvalidResultTool {
        fn name(&self) -> &str {
            "invalid_result"
        }

        fn description(&self) -> &str {
            "returns a classified unsuccessful result"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        fn execute<'a>(
            &'a self,
            _parameters: ToolParameters,
        ) -> futures::future::BoxFuture<'a, crate::error::Result<ToolResult>> {
            Box::pin(async {
                Ok(ToolResult::failure(
                    crate::tools::ToolFailureCategory::InvalidArguments,
                    "missing query",
                ))
            })
        }

        fn permissions(&self) -> Vec<echo_core::tools::permission::ToolPermission> {
            vec![echo_core::tools::permission::ToolPermission::Execute]
        }
    }

    impl Tool for InterleavingTool {
        fn name(&self) -> &str {
            "interleaving"
        }

        fn description(&self) -> &str {
            "test stream interleaving"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        fn execute<'a>(
            &'a self,
            _parameters: ToolParameters,
        ) -> futures::future::BoxFuture<'a, crate::error::Result<ToolResult>> {
            Box::pin(async { Ok(ToolResult::success("unused")) })
        }

        fn execute_stream_with_context<'a>(
            &'a self,
            params: ToolParameters,
            _ctx: &ToolContext,
        ) -> futures::future::BoxFuture<
            'a,
            crate::error::Result<Pin<Box<dyn Stream<Item = ToolStreamEvent> + Send + 'a>>>,
        > {
            Box::pin(async move {
                let label = params
                    .get("label")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string();
                let initial_delay = params
                    .get("initial_delay")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let finish_delay = params
                    .get("finish_delay")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let events = futures::stream::unfold(
                    (0_u8, label, initial_delay, finish_delay),
                    |(state, label, initial_delay, finish_delay)| async move {
                        match state {
                            0 => {
                                tokio::time::sleep(std::time::Duration::from_millis(initial_delay))
                                    .await;
                                Some((
                                    ToolStreamEvent::Output {
                                        channel: ToolOutputChannel::Stdout,
                                        chunk: format!("{label}-1"),
                                    },
                                    (1, label, initial_delay, finish_delay),
                                ))
                            }
                            1 => {
                                tokio::time::sleep(std::time::Duration::from_millis(finish_delay))
                                    .await;
                                Some((
                                    ToolStreamEvent::Output {
                                        channel: ToolOutputChannel::Stdout,
                                        chunk: format!("{label}-2"),
                                    },
                                    (2, label, initial_delay, finish_delay),
                                ))
                            }
                            2 => Some((
                                ToolStreamEvent::Complete(ToolResult::success(label.clone())),
                                (3, label, initial_delay, finish_delay),
                            )),
                            _ => None,
                        }
                    },
                );
                Ok(Box::pin(events) as Pin<Box<dyn Stream<Item = ToolStreamEvent> + Send + 'a>>)
            })
        }

        fn supports_streaming(&self) -> bool {
            true
        }
    }

    impl Tool for ResourceGuardStreamingTool {
        fn name(&self) -> &str {
            "resource_guard_stream"
        }

        fn description(&self) -> &str {
            "captures invocation resource guards on the streaming path"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        fn execute<'a>(
            &'a self,
            _parameters: ToolParameters,
        ) -> futures::future::BoxFuture<'a, crate::error::Result<ToolResult>> {
            Box::pin(async { Ok(ToolResult::success("fallback")) })
        }

        fn supports_streaming(&self) -> bool {
            true
        }

        fn execute_stream_with_context<'a>(
            &'a self,
            _parameters: ToolParameters,
            context: &ToolContext,
        ) -> futures::future::BoxFuture<
            'a,
            crate::error::Result<Pin<Box<dyn Stream<Item = ToolStreamEvent> + Send + 'a>>>,
        > {
            self.observed_guard_count.store(
                context.resource_guards.len(),
                std::sync::atomic::Ordering::SeqCst,
            );
            Box::pin(async {
                Ok(Box::pin(futures::stream::iter([ToolStreamEvent::Complete(
                    ToolResult::success("streamed"),
                )]))
                    as Pin<Box<dyn Stream<Item = ToolStreamEvent> + Send>>)
            })
        }
    }

    fn streaming_context(
        call_id: &str,
        label: &str,
        initial_delay: u64,
        finish_delay: u64,
        stream_tx: mpsc::Sender<ToolPipelineEvent>,
    ) -> ToolExecutionContext {
        let input = serde_json::json!({
            "label": label,
            "initial_delay": initial_delay,
            "finish_delay": finish_delay,
        });
        let params = match &input {
            Value::Object(map) => map.clone().into_iter().collect(),
            _ => ToolParameters::new(),
        };
        ToolExecutionContext {
            call_id: call_id.to_string(),
            requested_tool_name: "interleaving".to_string(),
            requested_input: input.clone(),
            tool_name: "interleaving".to_string(),
            params,
            input,
            hook_messages: HookMessageBatches::default(),
            result: None,
            output: None,
            blocked: false,
            block_reason: None,
            block_failure: None,
            duration_ms: 0,
            plan_mode: false,
            permission_decision: None,
            permission_mode_override: None,
            rewrites: Vec::new(),
            invocation_emitted: false,
            stream_tx: Some(stream_tx),
        }
    }

    fn completed_context(output: String) -> ToolExecutionContext {
        ToolExecutionContext {
            call_id: "call-output-budget".to_string(),
            requested_tool_name: "shell".to_string(),
            requested_input: serde_json::json!({}),
            tool_name: "shell".to_string(),
            params: ToolParameters::new(),
            input: serde_json::json!({}),
            hook_messages: HookMessageBatches::default(),
            result: Some(ToolResult::success(output)),
            output: None,
            blocked: false,
            block_reason: None,
            block_failure: None,
            duration_ms: 0,
            plan_mode: false,
            permission_decision: None,
            permission_mode_override: None,
            rewrites: Vec::new(),
            invocation_emitted: false,
            stream_tx: None,
        }
    }

    #[tokio::test]
    async fn unsuccessful_result_runs_failure_hook_instead_of_success_hook() -> Result<()> {
        use crate::skills::hooks::{HookAction, HookEvent, HookRule, HooksDefinition};

        let agent = crate::agent::ReactAgentBuilder::new()
            .model("test-model")
            .build()?;
        let mut definition = HooksDefinition::default();
        definition.add_rules(
            HookEvent::PostToolUse,
            vec![HookRule {
                matcher: "invalid_result".to_string(),
                hooks: vec![HookAction::Prompt {
                    prompt: "success-hook".to_string(),
                }],
            }],
        );
        definition.add_rules(
            HookEvent::PostToolUseFailure,
            vec![HookRule {
                matcher: "invalid_result".to_string(),
                hooks: vec![HookAction::Prompt {
                    prompt: "failure-hook".to_string(),
                }],
            }],
        );
        agent
            .hook_registry()
            .write()
            .await
            .register_user_hooks(definition);
        let snapshot = crate::agent::snapshot::AgentRunSnapshot::from_agent(&agent);
        let mut ctx = completed_context(String::new());
        ctx.tool_name = "invalid_result".to_string();
        ctx.result = Some(ToolResult::failure(
            crate::tools::ToolFailureCategory::InvalidArguments,
            "missing query",
        ));

        PostToolUseHookStage.run(&mut ctx, &snapshot).await?;

        assert_eq!(ctx.hook_messages.post, vec!["failure-hook"]);
        Ok(())
    }

    #[cfg(feature = "human-loop")]
    #[tokio::test]
    async fn permission_stage_consumes_call_scoped_mode_override() -> Result<()> {
        let service = Arc::new(
            crate::human_loop::PermissionService::new()
                .with_mode(echo_core::tools::permission::PermissionMode::Default),
        );
        let agent = crate::agent::ReactAgentBuilder::new()
            .model("test-model")
            .tool(Box::new(InvalidResultTool))
            .permission_service(service.clone())
            .build()?;
        let snapshot = crate::agent::snapshot::AgentRunSnapshot::from_agent(&agent);
        let mut ctx = completed_context(String::new());
        ctx.tool_name = "invalid_result".to_string();
        ctx.permission_mode_override =
            Some(echo_core::tools::permission::PermissionMode::BypassPermissions);

        PermissionStage.run(&mut ctx, &snapshot).await?;

        assert!(!ctx.blocked);
        assert_eq!(
            service.mode().await,
            echo_core::tools::permission::PermissionMode::Default
        );
        Ok(())
    }

    #[tokio::test]
    async fn invocation_disabled_tool_is_blocked_before_execution() -> Result<()> {
        let agent = crate::agent::ReactAgentBuilder::new()
            .model("test-model")
            .build()?;
        let invocation = echo_core::agent::AgentInvocationContext {
            disabled_tools: Some(std::collections::HashSet::from(["hidden_tool".to_string()])),
            ..Default::default()
        };
        let snapshot = crate::agent::snapshot::AgentRunSnapshot::from_agent_with_invocation(
            &agent,
            &invocation,
        );
        let mut ctx = completed_context(String::new());
        ctx.tool_name = "hidden_tool".to_string();

        ToolVisibilityStage.run(&mut ctx, &snapshot).await?;

        assert!(ctx.blocked);
        assert!(
            ctx.block_reason
                .as_deref()
                .is_some_and(|reason| reason.contains("not available in this invocation"))
        );
        Ok(())
    }

    #[cfg(feature = "files")]
    #[tokio::test]
    async fn truncation_stage_spills_without_token_limit_and_read_artifact_can_recover()
    -> Result<()> {
        let working_dir =
            tempfile::tempdir().map_err(|error| ReactError::Other(error.to_string()))?;
        let agent = crate::agent::ReactAgentBuilder::new()
            .model("test-model")
            .working_dir(working_dir.path())
            .tool_output_artifacts(
                echo_core::tools::artifact::ToolOutputArtifactConfig::new(
                    working_dir.path(),
                    "test",
                )
                .threshold_bytes(8),
            )
            .build()?;
        let snapshot = crate::agent::snapshot::AgentRunSnapshot::from_agent(&agent);
        let original = format!("{}END", "中文🙂\n".repeat(3_000));
        let mut ctx = completed_context(original.clone());

        TruncationStage.run(&mut ctx, &snapshot).await?;

        let result = ctx
            .result
            .as_ref()
            .ok_or_else(|| ReactError::Other("truncation stage lost tool result".to_string()))?;
        assert!(result.truncated);
        assert_eq!(
            result.metadata.get("output_handling").map(String::as_str),
            Some("spilled")
        );
        let artifact = result
            .artifact
            .as_ref()
            .ok_or_else(|| ReactError::Other("spill result lacks typed artifact".to_string()))?;
        let artifact_path = &artifact.path;
        assert!(artifact_path.starts_with(working_dir.path()));
        let artifact_path_text = artifact_path.to_string_lossy();
        assert!(ctx.output.as_deref().is_some_and(|output| {
            output.contains("Tool output preview only")
                && output.contains("not a summary")
                && output.contains("Full output artifact")
                && output.contains("Use read_artifact with this exact path")
                && output.contains(artifact_path_text.as_ref())
        }));

        let read_tool = crate::tools::files::artifact::ReadArtifactTool;
        let tool_context = ToolContext {
            working_dir: Some(working_dir.path().to_path_buf()),
            output_artifacts: agent.tool_output_artifacts(),
            ..ToolContext::default()
        };
        let mut cursor = None;
        let mut recovered = String::new();
        loop {
            let mut params = ToolParameters::from([
                (
                    "path".to_string(),
                    Value::String(artifact_path_text.to_string()),
                ),
                ("max_tokens".to_string(), Value::from(500_u64)),
            ]);
            if let Some(value) = cursor.clone() {
                params.insert("cursor".to_string(), Value::String(value));
            }
            let page = read_tool
                .execute_with_context(params, &tool_context)
                .await?;
            assert!(page.success);
            let (content, _) = page.output.split_once("\n\n[Artifact ").ok_or_else(|| {
                ReactError::Other("read_artifact page omitted its cursor notice".to_string())
            })?;
            recovered.push_str(content);
            cursor = page.metadata.get("next_cursor").cloned();
            if cursor.is_none() {
                break;
            }
        }
        assert_eq!(recovered, original);
        assert_eq!(
            std::fs::read_to_string(artifact_path).ok().as_deref(),
            Some(original.as_str())
        );
        assert_eq!(artifact.sha256.len(), 64);
        assert_eq!(artifact.retention, "test");
        Ok(())
    }

    #[tokio::test]
    async fn spill_projection_uses_artifact_reader_outside_working_dir() -> Result<()> {
        let working_dir =
            tempfile::tempdir().map_err(|error| ReactError::Other(error.to_string()))?;
        let artifact_dir =
            tempfile::tempdir().map_err(|error| ReactError::Other(error.to_string()))?;
        let agent = crate::agent::ReactAgentBuilder::new()
            .model("test-model")
            .working_dir(working_dir.path())
            .tool_output_artifacts(
                echo_core::tools::artifact::ToolOutputArtifactConfig::new(
                    artifact_dir.path(),
                    "test",
                )
                .threshold_bytes(8),
            )
            .build()?;
        let snapshot = crate::agent::snapshot::AgentRunSnapshot::from_agent(&agent);
        let mut ctx = completed_context("large complete output".to_string());

        TruncationStage.run(&mut ctx, &snapshot).await?;

        let output = ctx
            .output
            .as_deref()
            .ok_or_else(|| ReactError::Other("truncation stage produced no output".to_string()))?;
        assert!(output.contains("Use read_artifact with this exact path"));
        assert!(!output.contains("Use read_file with this exact path"));
        Ok(())
    }

    #[tokio::test]
    async fn token_budget_spills_even_below_byte_threshold() -> Result<()> {
        let artifact_dir =
            tempfile::tempdir().map_err(|error| ReactError::Other(error.to_string()))?;
        let agent = crate::agent::ReactAgentBuilder::new()
            .model("test-model")
            .max_tool_output_tokens(20)
            .tool_output_artifacts(
                echo_core::tools::artifact::ToolOutputArtifactConfig::new(
                    artifact_dir.path(),
                    "test",
                )
                .threshold_bytes(1024 * 1024),
            )
            .build()?;
        let snapshot = crate::agent::snapshot::AgentRunSnapshot::from_agent(&agent);
        let mut ctx = completed_context("small but token-heavy 中文🙂".repeat(100));

        TruncationStage.run(&mut ctx, &snapshot).await?;

        let result = ctx
            .result
            .as_ref()
            .ok_or_else(|| ReactError::Other("truncation stage lost tool result".to_string()))?;
        assert_eq!(
            result.metadata.get("output_handling").map(String::as_str),
            Some("spilled")
        );
        assert!(result.artifact.is_some());
        Ok(())
    }

    #[tokio::test]
    async fn truncation_stage_preserves_tool_level_truncation() -> Result<()> {
        let agent = crate::agent::ReactAgentBuilder::new()
            .model("test-model")
            .build()?;
        let snapshot = crate::agent::snapshot::AgentRunSnapshot::from_agent(&agent);
        let mut ctx = completed_context("partial page".to_string());
        if let Some(result) = ctx.result.as_mut() {
            result.truncated = true;
        }

        TruncationStage.run(&mut ctx, &snapshot).await?;

        assert!(ctx.result.as_ref().is_some_and(|result| result.truncated));
        Ok(())
    }

    #[tokio::test]
    async fn truncation_stage_is_utf8_safe() -> Result<()> {
        let config = crate::agent::AgentConfig::new("test-model", "test-agent", "test")
            .max_tool_output_tokens(20)
            .tool_output_artifacts(None);
        let agent = crate::agent::ReactAgent::new(config);
        let snapshot = crate::agent::snapshot::AgentRunSnapshot::from_agent(&agent);
        let mut ctx = completed_context("中文🙂abc".repeat(500));

        TruncationStage.run(&mut ctx, &snapshot).await?;

        let output = ctx
            .output
            .as_deref()
            .ok_or_else(|| ReactError::Other("truncation stage produced no output".to_string()))?;
        assert!(output.contains("Output truncated"));
        assert!(ctx.result.as_ref().is_some_and(|result| result.truncated));
        let result = ctx
            .result
            .as_ref()
            .ok_or_else(|| ReactError::Other("truncation stage lost tool result".to_string()))?;
        assert_eq!(
            result.metadata.get("output_handling").map(String::as_str),
            Some("truncated")
        );
        assert!(metadata_u64(result, "original_bytes") > metadata_u64(result, "returned_bytes"));
        assert!(metadata_usize(result, "estimated_tokens") > 20);
        Ok(())
    }

    #[tokio::test]
    async fn truncation_stage_records_inline_output_metrics() -> Result<()> {
        let agent = crate::agent::ReactAgentBuilder::new()
            .model("test-model")
            .max_tool_output_tokens(100)
            .build()?;
        let snapshot = crate::agent::snapshot::AgentRunSnapshot::from_agent(&agent);
        let mut ctx = completed_context("short output".to_string());

        TruncationStage.run(&mut ctx, &snapshot).await?;

        let result = ctx
            .result
            .as_ref()
            .ok_or_else(|| ReactError::Other("truncation stage lost tool result".to_string()))?;
        assert!(!result.truncated);
        assert_eq!(
            result.metadata.get("output_handling").map(String::as_str),
            Some("inline")
        );
        assert_eq!(metadata_u64(result, "original_bytes"), 12);
        assert_eq!(metadata_u64(result, "returned_bytes"), 12);
        assert!(metadata_usize(result, "estimated_tokens") > 0);
        Ok(())
    }

    #[tokio::test]
    async fn truncation_stage_falls_back_when_spill_directory_is_unwritable() -> Result<()> {
        let working_dir_file =
            tempfile::NamedTempFile::new().map_err(|error| ReactError::Other(error.to_string()))?;
        let agent = crate::agent::ReactAgentBuilder::new()
            .model("test-model")
            .working_dir(working_dir_file.path())
            .tool_output_artifacts(echo_core::tools::artifact::ToolOutputArtifactConfig::new(
                working_dir_file.path().join("artifacts"),
                "test",
            ))
            .build()?;
        let snapshot = crate::agent::snapshot::AgentRunSnapshot::from_agent(&agent);
        let mut ctx = completed_context("失败回退🙂".repeat(300_000));

        TruncationStage.run(&mut ctx, &snapshot).await?;

        let result = ctx
            .result
            .as_ref()
            .ok_or_else(|| ReactError::Other("truncation stage lost tool result".to_string()))?;
        assert!(result.truncated);
        assert_eq!(
            result.metadata.get("output_handling").map(String::as_str),
            Some("spill_failed_truncated")
        );
        assert!(result.metadata.contains_key("spill_error"));
        assert!(
            ctx.output
                .as_deref()
                .is_some_and(|output| output.contains("Output truncated"))
        );
        Ok(())
    }

    #[test]
    fn default_pipeline_records_trace_after_output_budgeting() {
        let pipeline = ToolExecutionPipeline::default_pipeline();
        let names: Vec<&str> = pipeline.stages.iter().map(|stage| stage.name()).collect();
        let truncation = names.iter().position(|name| *name == "truncation");
        let trace = names.iter().position(|name| *name == "trace_recording");
        assert!(matches!((truncation, trace), (Some(left), Some(right)) if left < right));
    }

    #[tokio::test]
    async fn unsuccessful_tool_result_remains_a_terminal_failure() -> crate::error::Result<()> {
        let agent = crate::agent::ReactAgentBuilder::new()
            .model("test-model")
            .enable_tools()
            .tool(Box::new(InvalidResultTool))
            .build()?;
        let snapshot = crate::agent::snapshot::AgentRunSnapshot::from_agent(&agent);
        let result = snapshot
            .execute_tool_with_policy(
                "call-invalid".to_string(),
                "invalid_result",
                &ToolParameters::new(),
                &serde_json::json!({}),
                None,
            )
            .await;
        let failure = match result {
            Ok(_) => {
                return Err(crate::error::ReactError::Other(
                    "unsuccessful ToolResult was projected as success".to_string(),
                ));
            }
            Err(failure) => failure,
        };

        assert_eq!(
            failure.result.failure.map(|failure| failure.category),
            Some(crate::tools::ToolFailureCategory::InvalidArguments)
        );
        Ok(())
    }

    #[tokio::test]
    async fn streaming_pipeline_forwards_invocation_resource_guards() -> crate::error::Result<()> {
        let observed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let agent = crate::agent::ReactAgentBuilder::new()
            .model("test-model")
            .enable_tools()
            .tool(Box::new(ResourceGuardStreamingTool {
                observed_guard_count: Arc::clone(&observed),
            }))
            .build()?;
        let invocation = echo_core::agent::AgentInvocationContext {
            resource_guards: vec![InvocationResourceGuard::new("stream-lease".to_string())],
            ..echo_core::agent::AgentInvocationContext::default()
        };
        let snapshot = crate::agent::snapshot::AgentRunSnapshot::from_agent_with_invocation(
            &agent,
            &invocation,
        );
        let (stream_tx, _stream_rx) = mpsc::channel(4);
        let execution = snapshot
            .execute_tool_with_policy(
                "guard-stream-call".to_string(),
                "resource_guard_stream",
                &ToolParameters::new(),
                &serde_json::json!({}),
                Some(stream_tx),
            )
            .await;
        if let Err(failure) = execution {
            return Err(crate::error::ReactError::Other(format!(
                "streaming guard tool failed: {}",
                failure.result.output
            )));
        }
        assert_eq!(observed.load(std::sync::atomic::Ordering::SeqCst), 1);
        Ok(())
    }

    /// Two single-tool ExecuteStage instances run concurrently; this test
    /// pins per-stream identity and that terminal events reflect actual
    /// completion order (execution fact). This is NOT the batch-contract
    /// test: the concurrent batch path (multiple tool calls in one turn)
    /// emits results in CALL order and is covered by
    /// `concurrent_batch_results_follow_call_order` in stream_channel.rs
    /// (F-RCT-04-P1-01).
    #[tokio::test]
    async fn multiplexed_streams_preserve_identity_and_terminal_order() -> crate::error::Result<()>
    {
        let agent = crate::agent::ReactAgentBuilder::new()
            .model("test-model")
            .enable_tools()
            .tool(Box::new(InterleavingTool))
            .build()?;
        let snapshot = Arc::new(crate::agent::snapshot::AgentRunSnapshot::from_agent(&agent));
        let (stream_tx, mut stream_rx) = mpsc::channel(64);
        let (completion_tx, mut completion_rx) = mpsc::channel(2);

        for mut ctx in [
            streaming_context("call-a", "a", 0, 60, stream_tx.clone()),
            streaming_context("call-b", "b", 10, 10, stream_tx.clone()),
        ] {
            let snapshot = Arc::clone(&snapshot);
            let completion_tx = completion_tx.clone();
            tokio::spawn(async move {
                let result = ExecuteStage.run(&mut ctx, &snapshot).await;
                let _ = completion_tx
                    .send((
                        ctx.call_id.clone(),
                        ctx.tool_name.clone(),
                        result,
                        ctx.result,
                    ))
                    .await;
            });
        }
        drop(stream_tx);
        drop(completion_tx);

        let mut events = Vec::new();
        let mut terminal_count = 0;
        while terminal_count < 2 {
            tokio::select! {
                biased;
                Some(event) = stream_rx.recv() => {
                    if let ToolPipelineEvent::Stream { call_id, name, event } = event {
                        events.push(AgentEvent::ToolStream { call_id, name, event });
                    }
                }
                Some((call_id, name, execution, result)) = completion_rx.recv() => {
                    execution?;
                    let result = result.ok_or_else(|| {
                        ReactError::Other("execute stage did not set a result".to_string())
                    })?;
                    events.push(AgentEvent::ToolResult {
                        call_id,
                        name,
                        result,
                    });
                    terminal_count += 1;
                }
            }
        }
        events.push(AgentEvent::ToolBatchEnd);

        let stream_chunks: Vec<(String, String)> = events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::ToolStream {
                    call_id,
                    event: ToolStreamEvent::Output { chunk, .. },
                    ..
                } => Some((call_id.clone(), chunk.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(
            stream_chunks,
            vec![
                ("call-a".into(), "a-1".into()),
                ("call-b".into(), "b-1".into()),
                ("call-b".into(), "b-2".into()),
                ("call-a".into(), "a-2".into()),
            ]
        );

        let terminal_ids: Vec<String> = events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::ToolResult { call_id, .. } => Some(call_id.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(terminal_ids, vec!["call-b", "call-a"]);
        assert!(matches!(events.last(), Some(AgentEvent::ToolBatchEnd)));
        Ok(())
    }
}
