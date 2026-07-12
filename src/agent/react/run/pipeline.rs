//! Tool execution pipeline — composable stages for tool call processing.
//!
//! Replaces the monolithic `execute_tool_feedback_raw` with a configurable
//! pipeline of discrete stages. Each stage implements [`PipelineStage`] and
//! can be added, removed, or reordered via [`ToolExecutionPipeline`].

use super::context::HookMessageBatches;
use crate::error::{ReactError, Result};
use crate::tools::{ToolParameters, ToolResult, ToolStreamEvent, is_write_tool};
use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::mpsc;
use tracing::debug;

// ── ToolExecutionContext ────────────────────────────────────────────

/// Mutable context that flows through the pipeline stages.
pub(crate) struct ToolExecutionContext {
    /// Unique call ID (links ToolCall/ToolResult/ToolError).
    pub call_id: String,
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
    /// Execution duration in milliseconds.
    pub duration_ms: u64,
    /// Whether the agent is in plan mode (read-only tools only).
    pub plan_mode: bool,
    /// Incremental tool events, tagged with their stable invocation identity.
    pub stream_tx: Option<mpsc::Sender<(String, String, ToolStreamEvent)>>,
}

// ── PipelineStage trait ────────────────────────────────────────────

/// A single stage in the tool execution pipeline.
///
/// Each stage receives a mutable [`ToolExecutionContext`] and may modify it.
/// Returning `Err` short-circuits the pipeline (tool execution failure).
/// Setting `ctx.blocked = true` causes subsequent stages to be skipped.
#[async_trait]
#[allow(dead_code)] // Internal trait, public for crate API consistency
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
                ctx.blocked = true;
                ctx.block_reason = Some(format!(
                    "Tool {} blocked by intervention: {}",
                    ctx.tool_name, reason
                ));
                ctx.output = Some(ctx.block_reason.clone().unwrap_or_default());
                return Ok(());
            }
            if let Some(redirect) = result.redirect_to {
                ctx.tool_name = redirect;
            }
            if let Some(modified) = result.modified_args {
                ctx.input = modified;
                if let serde_json::Value::Object(map) = &ctx.input {
                    ctx.params = map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                }
            }
            if let Some(injected) = result.injected_context {
                ctx.hook_messages.pre.push(injected);
            }
        }
        Ok(())
    }
}

/// Validates tool parameters with type checking.
pub struct ParseValidateStage;

#[async_trait]
impl PipelineStage for ParseValidateStage {
    fn name(&self) -> &str {
        "parse_validate"
    }

    async fn run(
        &self,
        ctx: &mut ToolExecutionContext,
        _snapshot: &crate::agent::snapshot::AgentRunSnapshot,
    ) -> Result<()> {
        // Convert raw input to type-safe ToolCallParams
        let params = echo_core::tools::ToolCallParams::from_value(&ctx.input);
        // Validate common required parameters based on tool name
        match ctx.tool_name.as_str() {
            "read_file" => {
                if let Err(e) = params.validate_required("path", "string") {
                    ctx.blocked = true;
                    ctx.block_reason = Some(e);
                }
            }
            "edit_file" | "write_file" | "append_file" | "create_file" => {
                if let Err(e) = params.validate_required("path", "string") {
                    ctx.blocked = true;
                    ctx.block_reason = Some(e);
                }
            }
            "shell" => {
                if let Err(e) = params.validate_required("command", "string") {
                    ctx.blocked = true;
                    ctx.block_reason = Some(e);
                }
            }
            _ => {}
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

        if hook_result.block {
            ctx.blocked = true;
            ctx.block_reason = Some(
                hook_result
                    .block_reason
                    .unwrap_or_else(|| format!("Tool {} blocked by hook", ctx.tool_name)),
            );
            return Ok(());
        }

        if let Some(updated) = hook_result.updated_input {
            ctx.input = updated.clone();
            if let Value::Object(map) = &updated {
                ctx.params = map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            }
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
        #[cfg(feature = "human-loop")]
        let approval_modified = snapshot
            .check_tool_approval(&ctx.tool_name, &ctx.input)
            .await
            .map_err(|error| ReactError::Other(error.to_string()))?;

        #[cfg(not(feature = "human-loop"))]
        let approval_modified = snapshot
            .check_tool_approval(&ctx.tool_name, &ctx.input)
            .await
            .map_err(|_| ReactError::Other("Permission check failed".into()))?;

        if let Some(modified) = approval_modified
            && let Value::Object(map) = &modified
        {
            ctx.params = map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
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
        if let Some(path) = extract_path_param(&ctx.tool_name, &ctx.params) {
            let canonical = std::fs::canonicalize(&path)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| path.clone());
            let ttl = std::time::Duration::from_secs(30 * 60);
            let mut files = snapshot
                .recently_read_files
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let read = match files.get(&canonical) {
                Some(instant) if instant.elapsed() < ttl => true,
                Some(_) => {
                    files.remove(&canonical);
                    false
                }
                None => false,
            };
            if !read {
                ctx.blocked = true;
                ctx.block_reason = Some(format!(
                    "Read-before-edit is enabled. File '{}' has not been read. Use read_file first.",
                    path
                ));
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
        if let Some(allowed_tools) = snapshot.tools.skill_allowed_tools.as_ref() {
            // Check if the tool is in the whitelist
            let permitted = allowed_tools.iter().any(|pattern| {
                echo_execution::skills::external::types::tool_matcher(pattern, &ctx.tool_name)
            });

            if !permitted {
                ctx.blocked = true;
                ctx.block_reason = Some(format!(
                    "Tool '{}' is not permitted by the activated skill's allowed_tools whitelist",
                    ctx.tool_name
                ));
            }
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
            execution_id: snapshot.current_execution_id.clone(),
            cancel: snapshot.external_cancel.clone(),
            trace_sink: snapshot.external_trace_sink.clone(),
            delegation_policy: snapshot.external_delegation_policy,
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
                        .execute_tool_stream_with_context(
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
                                    stream_tx
                                        .send((ctx.call_id.clone(), ctx.tool_name.clone(), event))
                                        .await
                                        .map_err(|_| ReactError::Other("Tool stream receiver closed".into()))?;
                                }
                                None => stream_open = false,
                            }
                        }
                        result = &mut execution => break result,
                    }
                };
                while let Some(event) = event_rx.recv().await {
                    stream_tx
                        .send((ctx.call_id.clone(), ctx.tool_name.clone(), event))
                        .await
                        .map_err(|_| ReactError::Other("Tool stream receiver closed".into()))?;
                }
                result
            } else {
                snapshot
                    .tools
                    .tool_manager
                    .execute_tool_stream_with_context(
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
                .execute_tool_with_context(&ctx.tool_name, ctx.params.clone(), &tool_ctx)
                .await
        };

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
                    bytes: None,
                    data: None,
                    truncated: false,
                    mime_type: None,
                    metadata: std::collections::HashMap::new(),
                }
            }
        };

        ctx.duration_ms = execution_start.elapsed().as_millis() as u64;
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

/// Runs PostToolUse hooks.
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
        let output = ctx
            .result
            .as_ref()
            .map(|r| {
                if r.output.is_empty() {
                    r.error.as_deref().unwrap_or("")
                } else {
                    r.output.as_str()
                }
            })
            .unwrap_or("");
        let post_result = hook_reg
            .run_post_tool_use(
                &ctx.tool_name,
                &ctx.input,
                output,
                snapshot.config.session_id.as_deref().unwrap_or(""),
            )
            .await;
        ctx.hook_messages.post = post_result.messages;
        if post_result.block {
            ctx.blocked = true;
            ctx.block_reason = post_result.block_reason;
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
        let processed = snapshot.process_tool_output(raw.to_string());
        if let Some(result) = ctx.result.as_mut() {
            result.truncated = processed.truncated;
            result.metadata.extend(processed.metadata);
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
                })
                .await;
            if !result.success {
                snapshot
                    .record_event(crate::trace::RunEvent::ToolError {
                        call_id: ctx.call_id.clone(),
                        name: ctx.tool_name.clone(),
                        message: result.error.clone().unwrap_or_default(),
                    })
                    .await;
            }
        }
        Ok(())
    }
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
    #[allow(dead_code)]
    /// Create a new empty pipeline.
    pub fn new() -> Self {
        Self { stages: Vec::new() }
    }

    #[allow(dead_code)]
    /// Add a stage to the end of the pipeline.
    pub(crate) fn with_stage(mut self, stage: Box<dyn PipelineStage>) -> Self {
        self.stages.push(stage);
        self
    }

    /// Build a pipeline with all standard stages in the correct order.
    pub fn default_pipeline() -> Self {
        Self {
            stages: vec![
                Box::new(InterventionStage),
                Box::new(ParseValidateStage),
                Box::new(PlanModeStage),
                Box::new(PreToolUseHookStage),
                Box::new(PermissionStage),
                Box::new(ReadBeforeEditStage),
                Box::new(SkillPermissionStage),
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
            stage.run(ctx, snapshot).await?;
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
            ctx.blocked = true;
            ctx.block_reason = Some(format!(
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

fn extract_path_param(_tool_name: &str, params: &ToolParameters) -> Option<String> {
    params
        .get("path")
        .or_else(|| params.get("file_path"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentEvent;
    use echo_core::tools::{Tool, ToolContext, ToolOutputChannel};
    use futures::Stream;
    use std::pin::Pin;
    use std::sync::Arc;

    #[test]
    fn test_is_write_tool() {
        assert!(is_write_tool("edit_file"));
        assert!(is_write_tool("write_file"));
        assert!(!is_write_tool("read_file"));
    }

    #[test]
    fn test_extract_path_param() {
        let params = vec![("path".to_string(), Value::String("src/main.rs".to_string()))]
            .into_iter()
            .collect();
        assert_eq!(
            extract_path_param("edit_file", &params),
            Some("src/main.rs".to_string())
        );
    }

    struct InterleavingTool;

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

    fn streaming_context(
        call_id: &str,
        label: &str,
        initial_delay: u64,
        finish_delay: u64,
        stream_tx: mpsc::Sender<(String, String, ToolStreamEvent)>,
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
            tool_name: "interleaving".to_string(),
            params,
            input,
            hook_messages: HookMessageBatches::default(),
            result: None,
            output: None,
            blocked: false,
            block_reason: None,
            duration_ms: 0,
            plan_mode: false,
            stream_tx: Some(stream_tx),
        }
    }

    fn completed_context(output: String) -> ToolExecutionContext {
        ToolExecutionContext {
            call_id: "call-output-budget".to_string(),
            tool_name: "shell".to_string(),
            params: ToolParameters::new(),
            input: serde_json::json!({}),
            hook_messages: HookMessageBatches::default(),
            result: Some(ToolResult::success(output)),
            output: None,
            blocked: false,
            block_reason: None,
            duration_ms: 0,
            plan_mode: false,
            stream_tx: None,
        }
    }

    #[tokio::test]
    async fn truncation_stage_spills_without_token_limit_and_read_file_can_recover() -> Result<()> {
        let working_dir =
            tempfile::tempdir().map_err(|error| ReactError::Other(error.to_string()))?;
        let agent = crate::agent::ReactAgentBuilder::new()
            .model("test-model")
            .working_dir(working_dir.path())
            .build()?;
        let snapshot = crate::agent::snapshot::AgentRunSnapshot::from_agent(&agent);
        let original = format!("{}END", "中文🙂\n".repeat(300_000));
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
        let artifact_path = result
            .metadata
            .get("artifact_path")
            .ok_or_else(|| ReactError::Other("spill metadata lacks artifact_path".to_string()))?;
        assert!(std::path::Path::new(artifact_path).starts_with(working_dir.path()));
        assert!(ctx.output.as_deref().is_some_and(|output| {
            output.contains("Output spilled to disk") && output.contains(artifact_path)
        }));

        let read_tool = crate::tools::files::files::ReadFileTool::new();
        let params = [
            ("path".to_string(), Value::String(artifact_path.clone())),
            ("offset".to_string(), Value::from(300_001_u64)),
            ("limit".to_string(), Value::from(1_u64)),
        ]
        .into_iter()
        .collect();
        let tool_context = ToolContext {
            working_dir: Some(working_dir.path().to_path_buf()),
            ..ToolContext::default()
        };
        let recovered = read_tool
            .execute_with_context(params, &tool_context)
            .await?;
        assert!(recovered.success);
        assert!(recovered.output.contains("END"));
        assert_eq!(
            std::fs::read_to_string(artifact_path).ok().as_deref(),
            Some(original.as_str())
        );
        Ok(())
    }

    #[tokio::test]
    async fn truncation_stage_is_utf8_safe() -> Result<()> {
        let agent = crate::agent::ReactAgentBuilder::new()
            .model("test-model")
            .max_tool_output_tokens(20)
            .build()?;
        let snapshot = crate::agent::snapshot::AgentRunSnapshot::from_agent(&agent);
        let mut ctx = completed_context("中文🙂abc".repeat(500));

        TruncationStage.run(&mut ctx, &snapshot).await?;

        let output = ctx
            .output
            .as_deref()
            .ok_or_else(|| ReactError::Other("truncation stage produced no output".to_string()))?;
        assert!(output.contains("Output truncated"));
        assert!(ctx.result.as_ref().is_some_and(|result| result.truncated));
        Ok(())
    }

    #[tokio::test]
    async fn truncation_stage_falls_back_when_spill_directory_is_unwritable() -> Result<()> {
        let working_dir_file =
            tempfile::NamedTempFile::new().map_err(|error| ReactError::Other(error.to_string()))?;
        let agent = crate::agent::ReactAgentBuilder::new()
            .model("test-model")
            .working_dir(working_dir_file.path())
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
    async fn multiplexed_streams_preserve_identity_and_terminal_order() {
        let agent = crate::agent::ReactAgentBuilder::new()
            .model("test-model")
            .enable_tools()
            .tool(Box::new(InterleavingTool))
            .build()
            .expect("test agent should build");
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
                completion_tx
                    .send((
                        ctx.call_id.clone(),
                        ctx.tool_name.clone(),
                        result,
                        ctx.result,
                    ))
                    .await
                    .expect("completion receiver should remain open");
            });
        }
        drop(stream_tx);
        drop(completion_tx);

        let mut events = Vec::new();
        let mut terminal_count = 0;
        while terminal_count < 2 {
            tokio::select! {
                biased;
                Some((call_id, name, event)) = stream_rx.recv() => {
                    events.push(AgentEvent::ToolStream { call_id, name, event });
                }
                Some((call_id, name, execution, result)) = completion_rx.recv() => {
                    execution.expect("execute stage should succeed");
                    let result = result.expect("execute stage should set a result");
                    events.push(AgentEvent::ToolResult {
                        call_id,
                        name,
                        output: result.output,
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
                AgentEvent::ToolResult { call_id, .. } | AgentEvent::ToolError { call_id, .. } => {
                    Some(call_id.clone())
                }
                _ => None,
            })
            .collect();
        assert_eq!(terminal_ids, vec!["call-b", "call-a"]);
        assert!(matches!(events.last(), Some(AgentEvent::ToolBatchEnd)));
    }
}
