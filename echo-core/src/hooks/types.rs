//! Hook type definitions — shared across all workspace crates.
//!
//! This module contains the core types for the unified hook system,
//! defined here in `echo-core` so that both `echo-execution` and
//! `echo-orchestration` can reference them without circular dependencies.
//!
//! The execution engine (`HookRegistry`, `parse_hook_output`, etc.)
//! remains in `echo-execution/src/skills/hooks.rs`.

use crate::tools::permission::{PermissionDecision, PermissionMode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

// ── Hook Event Category ────────────────────────────────────────────────

/// Category of a hook event, used to determine matcher semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HookEventCategory {
    /// Tool execution events — matcher filters by tool name.
    Tool,
    /// Session lifecycle events — matcher filters by lifecycle hint.
    Lifecycle,
    /// Subagent dispatch events — matcher filters by subagent type/name.
    Subagent,
    /// Task execution events — matcher filters by task subject/name.
    Task,
    /// Error/failure events — generally do not support matcher.
    Error,
    /// Evolution/memory lifecycle events — matcher filters by memory source or layer.
    Evolution,
}

// ── Subagent Stop Status ────────────────────────────────────────────────

/// Terminal status of a subagent run, carried by the `SubagentStop` event.
///
/// Industry alignment (Claude Code / Codex / OpenAI Agents SDK / AGTP): all
/// converge on "two boundary events (Start/Stop) + a status enum on the
/// terminal event" rather than one independent event per terminal state
/// (Stop/Cancelled/Failed/TimedOut). `SubagentStop` is always emitted exactly
/// once by the subagent executor's `finalize(status)` convergence point,
/// regardless of how the run ended.
///
/// `SubagentCancelled` (the former independent event) is removed per this
/// model — cancelled is a `SubagentStop` status value, not a separate event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentStopStatus {
    /// Subagent finished normally with output.
    Completed,
    /// Subagent exited with an error / reported failure.
    Failed,
    /// Subagent was cancelled (user abort, parent run cancelled, etc.).
    Cancelled,
    /// Subagent hit a deadline / timeout.
    TimedOut,
}

impl SubagentStopStatus {
    /// Stable wire name (used in serialized hook payloads and logs).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::TimedOut => "timed_out",
        }
    }
}

// ── Hook Event ─────────────────────────────────────────────────────────

/// When a hook fires in the agent lifecycle.
///
/// ## Event categories
///
/// | Category | Events | Matcher semantics |
/// |----------|--------|-------------------|
/// | Tool | `PreToolUse`, `PostToolUse`, `PostToolUseFailure`, `PermissionRequest`, `PermissionDenied` | Tool name (exact, glob, `|`-separated) |
/// | Lifecycle | `SessionStart`, `SessionEnd`, `Stop`, `Notification`, `UserPromptSubmit`, `PreCompact`, `PostCompact`, `ConfigChange`, `InstructionsLoaded`, `PostToolBatch` | Lifecycle hint (e.g. "startup", "permission_prompt") |
/// | Subagent | `SubagentStart`, `SubagentStop` | Subagent type/name. `SubagentStop` carries a `subagent_stop_status` (completed/failed/cancelled/timed_out); there is no separate `SubagentCancelled` event. |
/// | Task | `TaskCreated`, `TaskCompleted` | Task subject/name |
/// | Error | `StopFailure` | Not supported |
/// | Evolution | `PostMemoryWrite`, `MemoryLayerChange` | Memory source or layer name |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum HookEvent {
    // ── Tool execution events ──
    /// Before tool execution. Can block or modify input.
    PreToolUse,
    /// After tool execution succeeds.
    PostToolUse,
    /// After tool execution fails.
    PostToolUseFailure,
    /// Permission dialog appears. Can auto-approve/deny.
    PermissionRequest,
    /// After a tool is denied permission. Can signal retry.
    PermissionDenied,

    // ── Session lifecycle events ──
    /// Session begins or resumes.
    SessionStart,
    /// Session terminates.
    SessionEnd,
    /// Agent finishes responding. Can request continuation.
    Stop,
    /// Agent needs user attention.
    Notification,
    /// User submits a prompt. Can inject context or block.
    UserPromptSubmit,
    /// Before context compression.
    PreCompact,
    /// After context compression.
    PostCompact,
    /// Configuration file changes.
    ConfigChange,
    /// Skills/instructions loaded. Useful for post-load validation.
    InstructionsLoaded,
    /// After a batch of parallel tool calls resolves. Aggregation point.
    PostToolBatch,

    // ── Subagent events ──
    /// Before subagent dispatch.
    SubagentStart,
    /// After subagent completes (success or failure).
    SubagentStop,

    // ── Task events ──
    /// Task created/scheduled.
    TaskCreated,
    /// Task completed (success or failure).
    TaskCompleted,

    // ── Error events ──
    /// Agent encounters an unrecoverable error.
    StopFailure,

    // ── Plugin lifecycle events ──
    /// A plugin has been loaded and its components registered.
    PluginLoaded,
    /// A plugin has been disabled or unloaded.
    PluginDisabled,

    // ── Extended task events ──
    /// Task execution timed out.
    TaskTimeout,
    /// Task was cancelled by the user or system.
    TaskCancelled,

    // ── Extended subagent events ──
    // SubagentCancelled removed: the terminal model is now
    // SubagentStop(status = completed|failed|cancelled|timed_out), emitted
    // exactly once. Cancelled is a SubagentStop status, not a separate event.
    // (Claude Code / Codex / OpenAI Agents SDK / AGTP all converge on this
    // two-event + status-enum model.)

    // ── Evolution events ──
    /// After any memory is persisted to the Store.
    PostMemoryWrite,
    /// After a memory is promoted or demoted between layers.
    MemoryLayerChange,
    /// After a skill candidate is detected from memory patterns.
    SkillCandidateDetected,
    /// After a skill transitions between lifecycle states.
    SkillLifecycleTransition,
    /// After a skill health check completes.
    SkillHealthCheck,
    /// After a skill patch is applied.
    SkillPatchApplied,
    /// After two or more skills are merged.
    SkillMergeApplied,
    /// After a memory is promoted to a rule in AGENTS.md.
    RulePromoted,
}

impl HookEvent {
    /// Event category — determines matcher semantics and routing.
    pub fn category(self) -> HookEventCategory {
        match self {
            HookEvent::PreToolUse
            | HookEvent::PostToolUse
            | HookEvent::PostToolUseFailure
            | HookEvent::PermissionRequest
            | HookEvent::PermissionDenied => HookEventCategory::Tool,

            HookEvent::SubagentStart | HookEvent::SubagentStop => HookEventCategory::Subagent,

            HookEvent::TaskCreated
            | HookEvent::TaskCompleted
            | HookEvent::TaskTimeout
            | HookEvent::TaskCancelled => HookEventCategory::Task,

            HookEvent::StopFailure => HookEventCategory::Error,

            HookEvent::SessionStart
            | HookEvent::SessionEnd
            | HookEvent::Stop
            | HookEvent::Notification
            | HookEvent::UserPromptSubmit
            | HookEvent::PreCompact
            | HookEvent::PostCompact
            | HookEvent::ConfigChange
            | HookEvent::InstructionsLoaded
            | HookEvent::PostToolBatch
            | HookEvent::PluginLoaded
            | HookEvent::PluginDisabled => HookEventCategory::Lifecycle,

            HookEvent::PostMemoryWrite
            | HookEvent::MemoryLayerChange
            | HookEvent::SkillCandidateDetected
            | HookEvent::SkillLifecycleTransition
            | HookEvent::SkillHealthCheck
            | HookEvent::SkillPatchApplied
            | HookEvent::SkillMergeApplied
            | HookEvent::RulePromoted => HookEventCategory::Evolution,
        }
    }

    /// Whether this event is a tool execution event (uses tool_name matcher).
    pub fn is_tool_event(self) -> bool {
        self.category() == HookEventCategory::Tool
    }

    /// Whether this event supports the matcher field.
    pub fn supports_matcher(self) -> bool {
        matches!(
            self.category(),
            HookEventCategory::Tool
                | HookEventCategory::Lifecycle
                | HookEventCategory::Subagent
                | HookEventCategory::Task
                | HookEventCategory::Evolution
        )
    }

    /// Return the PascalCase event name as a static string (no allocation).
    ///
    /// Prefer this over `format!("{:?}", event)` in hot paths.
    pub const fn as_str(self) -> &'static str {
        match self {
            HookEvent::PreToolUse => "PreToolUse",
            HookEvent::PostToolUse => "PostToolUse",
            HookEvent::PostToolUseFailure => "PostToolUseFailure",
            HookEvent::PermissionRequest => "PermissionRequest",
            HookEvent::PermissionDenied => "PermissionDenied",
            HookEvent::SessionStart => "SessionStart",
            HookEvent::SessionEnd => "SessionEnd",
            HookEvent::Stop => "Stop",
            HookEvent::Notification => "Notification",
            HookEvent::UserPromptSubmit => "UserPromptSubmit",
            HookEvent::PreCompact => "PreCompact",
            HookEvent::PostCompact => "PostCompact",
            HookEvent::ConfigChange => "ConfigChange",
            HookEvent::InstructionsLoaded => "InstructionsLoaded",
            HookEvent::PostToolBatch => "PostToolBatch",
            HookEvent::SubagentStart => "SubagentStart",
            HookEvent::SubagentStop => "SubagentStop",
            HookEvent::TaskCreated => "TaskCreated",
            HookEvent::TaskCompleted => "TaskCompleted",
            HookEvent::StopFailure => "StopFailure",
            HookEvent::PluginLoaded => "PluginLoaded",
            HookEvent::PluginDisabled => "PluginDisabled",
            HookEvent::TaskTimeout => "TaskTimeout",
            HookEvent::TaskCancelled => "TaskCancelled",
            HookEvent::PostMemoryWrite => "PostMemoryWrite",
            HookEvent::MemoryLayerChange => "MemoryLayerChange",
            HookEvent::SkillCandidateDetected => "SkillCandidateDetected",
            HookEvent::SkillLifecycleTransition => "SkillLifecycleTransition",
            HookEvent::SkillHealthCheck => "SkillHealthCheck",
            HookEvent::SkillPatchApplied => "SkillPatchApplied",
            HookEvent::SkillMergeApplied => "SkillMergeApplied",
            HookEvent::RulePromoted => "RulePromoted",
        }
    }
}

// ── Compression Stats ──────────────────────────────────────────────────

/// Compression statistics passed to PreCompact/PostCompact hooks.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CompressHookStats {
    pub before_count: usize,
    pub after_count: usize,
    pub before_tokens: usize,
    pub after_tokens: usize,
}

// ── Hook Context ────────────────────────────────────────────────────────

/// Contextual data passed to hook execution, varying by event type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookContext {
    /// The event that triggered this hook.
    pub event: HookEvent,
    /// Session ID.
    pub session_id: String,
    /// Agent name.
    pub agent_name: String,
    /// Working directory at the time of the event.
    pub cwd: String,

    // ── Tool event fields ──
    /// Tool name (PreToolUse, PostToolUse, PostToolUseFailure, PermissionRequest, PermissionDenied).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// Tool input (PreToolUse, PostToolUse, PostToolUseFailure, PermissionRequest, PermissionDenied).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_input: Option<Value>,
    /// Tool output (PostToolUse only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_output: Option<String>,
    /// Tool error message (PostToolUseFailure only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_error: Option<String>,

    // ── PermissionDenied fields ──
    /// Reason the permission was denied (PermissionDenied only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub denial_reason: Option<String>,
    /// Whether the model may retry with different params (PermissionDenied only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_allowed: Option<bool>,

    // ── Lifecycle event fields ──
    /// Matcher hint for lifecycle events.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matcher: Option<String>,
    /// User prompt text (UserPromptSubmit only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// Compression stats (PreCompact, PostCompact only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compress_stats: Option<CompressHookStats>,
    /// Changed config file path (ConfigChange only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_path: Option<String>,
    /// Loaded skill names (InstructionsLoaded only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_names: Option<Vec<String>>,

    // ── PostToolBatch fields ──
    /// Tool names in this batch (PostToolBatch only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub batch_tool_names: Option<Vec<String>>,
    /// Number of tools in batch that succeeded (PostToolBatch only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub batch_success_count: Option<usize>,
    /// Number of tools in batch that failed (PostToolBatch only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub batch_failure_count: Option<usize>,

    // ── Subagent fields ──
    /// Subagent name/type (SubagentStart, SubagentStop only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subagent_name: Option<String>,
    /// Subagent execution mode (SubagentStart, SubagentStop only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subagent_mode: Option<String>,
    /// Subagent task description (SubagentStart only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subagent_task: Option<String>,
    /// Subagent result summary (SubagentStop only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subagent_result: Option<String>,
    /// Terminal status of the subagent run (SubagentStop only).
    ///
    /// Always present on SubagentStop contexts. Lets consumers distinguish
    /// completed/failed/cancelled/timed_out without inspecting free-form
    /// result text. Industry-aligned (see [`SubagentStopStatus`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subagent_stop_status: Option<SubagentStopStatus>,

    // ── Task fields ──
    /// Task ID (TaskCreated, TaskCompleted only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    /// Task subject/name (TaskCreated, TaskCompleted only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_subject: Option<String>,
    /// Task result summary (TaskCompleted only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_result: Option<String>,

    // ── StopFailure fields ──
    /// Error message (StopFailure only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_error: Option<String>,
    /// Error category: "max_iterations", "api_error", "tool_batch_timeout", etc.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_category: Option<String>,

    // ── Stop hook loop prevention ──
    /// Whether this Stop hook is being re-invoked (prevents infinite loops).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_hook_active: Option<bool>,

    // ── Evolution event fields ──
    /// Memory key that was written (PostMemoryWrite only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_key: Option<String>,
    /// Memory source that triggered the write (PostMemoryWrite only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_source: Option<String>,
    /// Layer the memory was promoted/demoted from (MemoryLayerChange only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_from_layer: Option<String>,
    /// Layer the memory was promoted/demoted to (MemoryLayerChange only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_to_layer: Option<String>,
}

impl Default for HookContext {
    fn default() -> Self {
        Self {
            event: HookEvent::PreToolUse, // arbitrary, always overridden by factory
            session_id: String::new(),
            agent_name: String::new(),
            cwd: HookContext::cwd_default(),
            tool_name: None,
            tool_input: None,
            tool_output: None,
            tool_error: None,
            denial_reason: None,
            retry_allowed: None,
            matcher: None,
            prompt: None,
            compress_stats: None,
            config_path: None,
            skill_names: None,
            batch_tool_names: None,
            batch_success_count: None,
            batch_failure_count: None,
            subagent_name: None,
            subagent_mode: None,
            subagent_task: None,
            subagent_result: None,
            subagent_stop_status: None,
            task_id: None,
            task_subject: None,
            task_result: None,
            failure_error: None,
            failure_category: None,
            stop_hook_active: None,
            memory_key: None,
            memory_source: None,
            memory_from_layer: None,
            memory_to_layer: None,
        }
    }
}

impl HookContext {
    fn cwd_default() -> String {
        std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default()
    }

    // ── Factory methods for tool events ──

    pub fn for_pre_tool_use(
        tool_name: &str,
        tool_input: &Value,
        session_id: &str,
        agent_name: &str,
    ) -> Self {
        Self {
            event: HookEvent::PreToolUse,
            session_id: session_id.to_string(),
            agent_name: agent_name.to_string(),
            tool_name: Some(tool_name.to_string()),
            tool_input: Some(tool_input.clone()),
            ..Self::default()
        }
    }

    pub fn for_post_tool_use(
        tool_name: &str,
        tool_input: &Value,
        tool_output: &str,
        session_id: &str,
        agent_name: &str,
    ) -> Self {
        Self {
            event: HookEvent::PostToolUse,
            session_id: session_id.to_string(),
            agent_name: agent_name.to_string(),
            tool_name: Some(tool_name.to_string()),
            tool_input: Some(tool_input.clone()),
            tool_output: Some(tool_output.to_string()),
            ..Self::default()
        }
    }

    pub fn for_post_tool_use_failure(
        tool_name: &str,
        tool_input: &Value,
        tool_error: &str,
        session_id: &str,
        agent_name: &str,
    ) -> Self {
        Self {
            event: HookEvent::PostToolUseFailure,
            session_id: session_id.to_string(),
            agent_name: agent_name.to_string(),
            tool_name: Some(tool_name.to_string()),
            tool_input: Some(tool_input.clone()),
            tool_error: Some(tool_error.to_string()),
            ..Self::default()
        }
    }

    pub fn for_permission_request(
        tool_name: &str,
        tool_input: &Value,
        session_id: &str,
        agent_name: &str,
    ) -> Self {
        Self {
            event: HookEvent::PermissionRequest,
            session_id: session_id.to_string(),
            agent_name: agent_name.to_string(),
            tool_name: Some(tool_name.to_string()),
            tool_input: Some(tool_input.clone()),
            ..Self::default()
        }
    }

    pub fn for_permission_denied(
        tool_name: &str,
        tool_input: &Value,
        denial_reason: &str,
        retry_allowed: bool,
        session_id: &str,
        agent_name: &str,
    ) -> Self {
        Self {
            event: HookEvent::PermissionDenied,
            session_id: session_id.to_string(),
            agent_name: agent_name.to_string(),
            tool_name: Some(tool_name.to_string()),
            tool_input: Some(tool_input.clone()),
            denial_reason: Some(denial_reason.to_string()),
            retry_allowed: Some(retry_allowed),
            ..Self::default()
        }
    }

    // ── Factory methods for lifecycle events ──

    pub fn for_session_start(matcher: &str, session_id: &str, agent_name: &str) -> Self {
        Self {
            event: HookEvent::SessionStart,
            session_id: session_id.to_string(),
            agent_name: agent_name.to_string(),
            matcher: Some(matcher.to_string()),
            ..Self::default()
        }
    }

    pub fn for_session_end(matcher: &str, session_id: &str, agent_name: &str) -> Self {
        Self {
            event: HookEvent::SessionEnd,
            session_id: session_id.to_string(),
            agent_name: agent_name.to_string(),
            matcher: Some(matcher.to_string()),
            ..Self::default()
        }
    }

    /// Generic constructor for lifecycle events that don't have a dedicated factory.
    pub fn for_lifecycle(
        event: HookEvent,
        matcher: &str,
        session_id: &str,
        agent_name: &str,
    ) -> Self {
        Self {
            event,
            session_id: session_id.to_string(),
            agent_name: agent_name.to_string(),
            matcher: Some(matcher.to_string()),
            ..Self::default()
        }
    }

    pub fn for_stop(
        matcher: Option<&str>,
        session_id: &str,
        agent_name: &str,
        stop_hook_active: bool,
    ) -> Self {
        Self {
            event: HookEvent::Stop,
            session_id: session_id.to_string(),
            agent_name: agent_name.to_string(),
            matcher: matcher.map(|s| s.to_string()),
            stop_hook_active: Some(stop_hook_active),
            ..Self::default()
        }
    }

    pub fn for_notification(matcher: &str, session_id: &str, agent_name: &str) -> Self {
        Self {
            event: HookEvent::Notification,
            session_id: session_id.to_string(),
            agent_name: agent_name.to_string(),
            matcher: Some(matcher.to_string()),
            ..Self::default()
        }
    }

    pub fn for_user_prompt_submit(
        prompt: &str,
        matcher: Option<&str>,
        session_id: &str,
        agent_name: &str,
    ) -> Self {
        Self {
            event: HookEvent::UserPromptSubmit,
            session_id: session_id.to_string(),
            agent_name: agent_name.to_string(),
            matcher: matcher.map(|s| s.to_string()),
            prompt: Some(prompt.to_string()),
            ..Self::default()
        }
    }

    pub fn for_pre_compact(
        stats: &CompressHookStats,
        matcher: &str,
        session_id: &str,
        agent_name: &str,
    ) -> Self {
        Self {
            event: HookEvent::PreCompact,
            session_id: session_id.to_string(),
            agent_name: agent_name.to_string(),
            matcher: Some(matcher.to_string()),
            compress_stats: Some(stats.clone()),
            ..Self::default()
        }
    }

    pub fn for_post_compact(
        stats: &CompressHookStats,
        matcher: &str,
        session_id: &str,
        agent_name: &str,
    ) -> Self {
        Self {
            event: HookEvent::PostCompact,
            session_id: session_id.to_string(),
            agent_name: agent_name.to_string(),
            matcher: Some(matcher.to_string()),
            compress_stats: Some(stats.clone()),
            ..Self::default()
        }
    }

    pub fn for_config_change(
        config_path: &str,
        matcher: Option<&str>,
        session_id: &str,
        agent_name: &str,
    ) -> Self {
        Self {
            event: HookEvent::ConfigChange,
            session_id: session_id.to_string(),
            agent_name: agent_name.to_string(),
            matcher: matcher.map(|s| s.to_string()),
            config_path: Some(config_path.to_string()),
            ..Self::default()
        }
    }

    pub fn for_instructions_loaded(
        matcher: &str,
        skill_names: &[String],
        session_id: &str,
        agent_name: &str,
    ) -> Self {
        Self {
            event: HookEvent::InstructionsLoaded,
            session_id: session_id.to_string(),
            agent_name: agent_name.to_string(),
            matcher: Some(matcher.to_string()),
            skill_names: Some(skill_names.to_vec()),
            ..Self::default()
        }
    }

    // ── Factory methods for aggregation events ──

    pub fn for_post_tool_batch(
        batch_tool_names: &[String],
        success_count: usize,
        failure_count: usize,
        session_id: &str,
        agent_name: &str,
    ) -> Self {
        Self {
            event: HookEvent::PostToolBatch,
            session_id: session_id.to_string(),
            agent_name: agent_name.to_string(),
            matcher: Some(batch_tool_names.join("|")),
            batch_tool_names: Some(batch_tool_names.to_vec()),
            batch_success_count: Some(success_count),
            batch_failure_count: Some(failure_count),
            ..Self::default()
        }
    }

    // ── Factory methods for subagent events ──

    pub fn for_subagent_start(
        subagent_name: &str,
        subagent_mode: &str,
        task: &str,
        session_id: &str,
        agent_name: &str,
    ) -> Self {
        Self {
            event: HookEvent::SubagentStart,
            session_id: session_id.to_string(),
            agent_name: agent_name.to_string(),
            matcher: Some(subagent_name.to_string()),
            subagent_name: Some(subagent_name.to_string()),
            subagent_mode: Some(subagent_mode.to_string()),
            subagent_task: Some(task.to_string()),
            ..Self::default()
        }
    }

    pub fn for_subagent_stop(
        subagent_name: &str,
        subagent_mode: &str,
        result: &str,
        status: SubagentStopStatus,
        session_id: &str,
        agent_name: &str,
    ) -> Self {
        Self {
            event: HookEvent::SubagentStop,
            session_id: session_id.to_string(),
            agent_name: agent_name.to_string(),
            matcher: Some(subagent_name.to_string()),
            subagent_name: Some(subagent_name.to_string()),
            subagent_mode: Some(subagent_mode.to_string()),
            subagent_result: Some(result.to_string()),
            subagent_stop_status: Some(status),
            ..Self::default()
        }
    }

    // ── Factory methods for task events ──

    pub fn for_task_created(
        task_id: &str,
        task_subject: &str,
        session_id: &str,
        agent_name: &str,
    ) -> Self {
        Self {
            event: HookEvent::TaskCreated,
            session_id: session_id.to_string(),
            agent_name: agent_name.to_string(),
            matcher: Some(task_subject.to_string()),
            task_id: Some(task_id.to_string()),
            task_subject: Some(task_subject.to_string()),
            ..Self::default()
        }
    }

    pub fn for_task_completed(
        task_id: &str,
        task_subject: &str,
        result: &str,
        session_id: &str,
        agent_name: &str,
    ) -> Self {
        Self {
            event: HookEvent::TaskCompleted,
            session_id: session_id.to_string(),
            agent_name: agent_name.to_string(),
            matcher: Some(task_subject.to_string()),
            task_id: Some(task_id.to_string()),
            task_subject: Some(task_subject.to_string()),
            task_result: Some(result.to_string()),
            ..Self::default()
        }
    }

    // ── Factory methods for error events ──

    pub fn for_stop_failure(
        failure_error: &str,
        failure_category: &str,
        session_id: &str,
        agent_name: &str,
    ) -> Self {
        Self {
            event: HookEvent::StopFailure,
            session_id: session_id.to_string(),
            agent_name: agent_name.to_string(),
            failure_error: Some(failure_error.to_string()),
            failure_category: Some(failure_category.to_string()),
            ..Self::default()
        }
    }

    // ── Factory methods for evolution events ──

    /// Create context for PostMemoryWrite event.
    pub fn for_post_memory_write(
        memory_key: &str,
        memory_source: &str,
        session_id: &str,
        agent_name: &str,
    ) -> Self {
        Self {
            event: HookEvent::PostMemoryWrite,
            session_id: session_id.to_string(),
            agent_name: agent_name.to_string(),
            matcher: Some(memory_source.to_string()),
            memory_key: Some(memory_key.to_string()),
            memory_source: Some(memory_source.to_string()),
            ..Self::default()
        }
    }

    /// Create context for MemoryLayerChange event.
    pub fn for_memory_layer_change(
        memory_key: &str,
        from_layer: &str,
        to_layer: &str,
        session_id: &str,
        agent_name: &str,
    ) -> Self {
        Self {
            event: HookEvent::MemoryLayerChange,
            session_id: session_id.to_string(),
            agent_name: agent_name.to_string(),
            matcher: Some(format!("{from_layer}->{to_layer}")),
            memory_key: Some(memory_key.to_string()),
            memory_from_layer: Some(from_layer.to_string()),
            memory_to_layer: Some(to_layer.to_string()),
            ..Self::default()
        }
    }
}

// ── Hook Result ─────────────────────────────────────────────────────────

/// Result of executing a hook.
#[derive(Debug, Clone, Default)]
pub struct HookResult {
    /// If true, the operation should be blocked.
    pub block: bool,
    /// Reason for blocking (if block is true).
    pub block_reason: Option<String>,
    /// Modified tool input (PreToolUse only).
    pub updated_input: Option<Value>,
    /// Messages to inject into context.
    pub messages: Vec<String>,
    /// If true, prevent further hooks from running.
    pub stop_propagation: bool,
    /// Permission decision (PreToolUse, PermissionRequest, Notification).
    pub permission_decision: Option<PermissionDecision>,
    /// Permission mode override (PreToolUse, PermissionRequest).
    pub permission_mode_override: Option<PermissionMode>,
    /// For Stop event: if set, agent continues instead of stopping.
    pub continue_reason: Option<String>,
    /// For UserPromptSubmit/SessionStart: additional context to inject.
    pub injected_context: Option<String>,
    /// For PermissionDenied: if true, the model may retry with different parameters.
    pub retry: bool,
    /// Arbitrary metadata from hooks.
    pub metadata: Option<Value>,
    /// For ActivateSkill hooks: skill to activate directly (name + reason).
    /// Populated by `execute_action` when an `ActivateSkill` action matches;
    /// consumed by `fire_lifecycle_hook` to call `ReactAgent::activate_skill`.
    pub activate_skill: Option<(String, String)>,
}

impl HookResult {
    /// Create an allow result.
    pub fn allow() -> Self {
        Self {
            permission_decision: Some(PermissionDecision::Allow),
            ..Self::default()
        }
    }

    /// Create a deny result.
    pub fn deny(reason: String) -> Self {
        Self {
            block: true,
            block_reason: Some(reason.clone()),
            permission_decision: Some(PermissionDecision::Deny { reason }),
            ..Self::default()
        }
    }

    /// Create an ask result.
    pub fn ask(suggestions: Vec<String>) -> Self {
        Self {
            permission_decision: Some(PermissionDecision::Ask { suggestions }),
            ..Self::default()
        }
    }

    /// Check if hook made a permission decision.
    pub fn has_permission_decision(&self) -> bool {
        self.permission_decision.is_some()
    }

    /// Whether this result indicates the agent should continue working.
    pub fn should_continue(&self) -> bool {
        self.continue_reason.is_some()
    }

    /// Create a result requesting direct skill activation.
    pub fn with_activate_skill(skill: String, reason: String) -> Self {
        Self {
            activate_skill: Some((skill, reason)),
            ..Self::default()
        }
    }
}

// ── Hook Source ──────────────────────────────────────────────────────────

/// Source of hook registration.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum HookSource {
    /// Hooks from a file-based skill.
    Skill(String),
    /// Hooks from user configuration (echo-agent.yaml).
    UserConfig,
    /// Hooks from an installed plugin.
    Plugin(String),
}

impl std::fmt::Display for HookSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HookSource::Skill(name) => write!(f, "skill:{}", name),
            HookSource::UserConfig => write!(f, "user_config"),
            HookSource::Plugin(name) => write!(f, "plugin:{}", name),
        }
    }
}

// ── Unified Hook Executor Fn ─────────────────────────────────────────────

/// Type-erased callback for firing unified hooks from
/// orchestration/subagent layers that don't have direct access to
/// `HookRegistry` (which lives in `echo-execution`).
///
/// The agent layer injects this via construction so that `TaskExecutor`
/// and `SubagentExecutor` can fire `TaskCreated`/`TaskCompleted` and
/// `SubagentStart`/`SubagentStop` events into the unified system.
///
/// Handles Lifecycle, Subagent, and Task events (not just Lifecycle).
pub type UnifiedHookExecutorFn =
    Arc<dyn Fn(HookContext) -> Pin<Box<dyn Future<Output = HookResult> + Send>> + Send + Sync>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hook_event_category() {
        assert_eq!(HookEvent::PreToolUse.category(), HookEventCategory::Tool);
        assert_eq!(
            HookEvent::PermissionDenied.category(),
            HookEventCategory::Tool
        );
        assert_eq!(
            HookEvent::SessionStart.category(),
            HookEventCategory::Lifecycle
        );
        assert_eq!(
            HookEvent::PostToolBatch.category(),
            HookEventCategory::Lifecycle
        );
        assert_eq!(
            HookEvent::InstructionsLoaded.category(),
            HookEventCategory::Lifecycle
        );
        assert_eq!(
            HookEvent::SubagentStart.category(),
            HookEventCategory::Subagent
        );
        assert_eq!(
            HookEvent::SubagentStop.category(),
            HookEventCategory::Subagent
        );
        assert_eq!(HookEvent::TaskCreated.category(), HookEventCategory::Task);
        assert_eq!(HookEvent::TaskCompleted.category(), HookEventCategory::Task);
        assert_eq!(HookEvent::StopFailure.category(), HookEventCategory::Error);
        assert_eq!(
            HookEvent::PostMemoryWrite.category(),
            HookEventCategory::Evolution
        );
        assert_eq!(
            HookEvent::MemoryLayerChange.category(),
            HookEventCategory::Evolution
        );
    }

    #[test]
    fn test_hook_event_is_tool_event() {
        assert!(HookEvent::PreToolUse.is_tool_event());
        assert!(HookEvent::PostToolUse.is_tool_event());
        assert!(HookEvent::PermissionDenied.is_tool_event());
        assert!(!HookEvent::SessionStart.is_tool_event());
        assert!(!HookEvent::StopFailure.is_tool_event());
    }

    #[test]
    fn test_hook_event_supports_matcher() {
        assert!(HookEvent::PreToolUse.supports_matcher());
        assert!(HookEvent::SessionStart.supports_matcher());
        assert!(HookEvent::SubagentStart.supports_matcher());
        assert!(HookEvent::TaskCreated.supports_matcher());
        assert!(!HookEvent::StopFailure.supports_matcher());
    }

    #[test]
    fn test_hook_event_as_str() {
        assert_eq!(HookEvent::PreToolUse.as_str(), "PreToolUse");
        assert_eq!(HookEvent::PostToolUse.as_str(), "PostToolUse");
        assert_eq!(HookEvent::PostToolUseFailure.as_str(), "PostToolUseFailure");
        assert_eq!(HookEvent::PermissionRequest.as_str(), "PermissionRequest");
        assert_eq!(HookEvent::PermissionDenied.as_str(), "PermissionDenied");
        assert_eq!(HookEvent::SessionStart.as_str(), "SessionStart");
        assert_eq!(HookEvent::SessionEnd.as_str(), "SessionEnd");
        assert_eq!(HookEvent::Stop.as_str(), "Stop");
        assert_eq!(HookEvent::Notification.as_str(), "Notification");
        assert_eq!(HookEvent::UserPromptSubmit.as_str(), "UserPromptSubmit");
        assert_eq!(HookEvent::PreCompact.as_str(), "PreCompact");
        assert_eq!(HookEvent::PostCompact.as_str(), "PostCompact");
        assert_eq!(HookEvent::ConfigChange.as_str(), "ConfigChange");
        assert_eq!(HookEvent::InstructionsLoaded.as_str(), "InstructionsLoaded");
        assert_eq!(HookEvent::PostToolBatch.as_str(), "PostToolBatch");
        assert_eq!(HookEvent::SubagentStart.as_str(), "SubagentStart");
        assert_eq!(HookEvent::SubagentStop.as_str(), "SubagentStop");
        assert_eq!(HookEvent::TaskCreated.as_str(), "TaskCreated");
        assert_eq!(HookEvent::TaskCompleted.as_str(), "TaskCompleted");
        assert_eq!(HookEvent::StopFailure.as_str(), "StopFailure");
        assert_eq!(HookEvent::PostMemoryWrite.as_str(), "PostMemoryWrite");
        assert_eq!(HookEvent::MemoryLayerChange.as_str(), "MemoryLayerChange");
    }

    #[test]
    fn test_hook_event_serde() {
        let event = HookEvent::PermissionDenied;
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(json, "\"PermissionDenied\"");

        let event = HookEvent::SubagentStart;
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(json, "\"SubagentStart\"");

        let parsed: HookEvent = serde_json::from_str("\"TaskCompleted\"").unwrap();
        assert_eq!(parsed, HookEvent::TaskCompleted);
    }

    #[test]
    fn test_hook_context_for_permission_denied() {
        let ctx = HookContext::for_permission_denied(
            "Bash",
            &serde_json::json!({"command": "rm -rf /"}),
            "unsafe command",
            true,
            "sess-1",
            "agent",
        );
        assert_eq!(ctx.event, HookEvent::PermissionDenied);
        assert_eq!(ctx.tool_name.as_deref(), Some("Bash"));
        assert_eq!(ctx.denial_reason.as_deref(), Some("unsafe command"));
        assert_eq!(ctx.retry_allowed, Some(true));
    }

    #[test]
    fn test_hook_context_for_stop_failure() {
        let ctx = HookContext::for_stop_failure(
            "MaxIterationsExceeded",
            "max_iterations",
            "sess-1",
            "agent",
        );
        assert_eq!(ctx.event, HookEvent::StopFailure);
        assert_eq!(ctx.failure_error.as_deref(), Some("MaxIterationsExceeded"));
        assert_eq!(ctx.failure_category.as_deref(), Some("max_iterations"));
    }

    #[test]
    fn test_hook_context_for_post_tool_batch() {
        let names = vec!["Bash".to_string(), "Read".to_string()];
        let ctx = HookContext::for_post_tool_batch(&names, 1, 1, "sess-1", "agent");
        assert_eq!(ctx.event, HookEvent::PostToolBatch);
        assert_eq!(ctx.batch_tool_names.as_ref().unwrap().len(), 2);
        assert_eq!(ctx.batch_success_count, Some(1));
        assert_eq!(ctx.batch_failure_count, Some(1));
    }

    #[test]
    fn test_hook_context_for_subagent() {
        let ctx = HookContext::for_subagent_start(
            "coder",
            "sync",
            "implement feature X",
            "sess-1",
            "agent",
        );
        assert_eq!(ctx.event, HookEvent::SubagentStart);
        assert_eq!(ctx.subagent_name.as_deref(), Some("coder"));
        assert_eq!(ctx.subagent_mode.as_deref(), Some("sync"));
        assert_eq!(ctx.subagent_task.as_deref(), Some("implement feature X"));
    }

    #[test]
    fn test_hook_context_for_task() {
        let ctx = HookContext::for_task_created("t-1", "build API", "sess-1", "agent");
        assert_eq!(ctx.event, HookEvent::TaskCreated);
        assert_eq!(ctx.task_id.as_deref(), Some("t-1"));
        assert_eq!(ctx.task_subject.as_deref(), Some("build API"));
    }

    #[test]
    fn test_hook_result_retry() {
        let mut result = HookResult::default();
        assert!(!result.retry);
        result.retry = true;
        assert!(result.retry);
    }

    #[test]
    fn test_hook_result_allow_deny_ask() {
        let result = HookResult::allow();
        assert!(result.has_permission_decision());
        assert!(result.permission_decision.unwrap().is_allowed());

        let result = HookResult::deny("unsafe".to_string());
        assert!(result.block);
        assert!(result.permission_decision.unwrap().is_denied());

        let result = HookResult::ask(vec!["Allow".to_string()]);
        assert!(result.permission_decision.unwrap().requires_approval());
    }

    #[test]
    fn test_lifecycle_hook_executor_fn() {
        let executor: UnifiedHookExecutorFn = Arc::new(|ctx: HookContext| {
            Box::pin(async move {
                let mut result = HookResult::default();
                if ctx.event == HookEvent::SubagentStart {
                    result.injected_context = Some("subagent context".to_string());
                }
                result
            })
        });

        let ctx = HookContext::for_subagent_start("test", "sync", "task", "", "");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(executor(ctx));
        assert_eq!(
            result.injected_context,
            Some("subagent context".to_string())
        );
    }

    #[test]
    fn test_evolution_event_category() {
        assert_eq!(
            HookEvent::PostMemoryWrite.category(),
            HookEventCategory::Evolution
        );
        assert_eq!(
            HookEvent::MemoryLayerChange.category(),
            HookEventCategory::Evolution
        );
    }

    #[test]
    fn test_evolution_event_supports_matcher() {
        assert!(HookEvent::PostMemoryWrite.supports_matcher());
        assert!(HookEvent::MemoryLayerChange.supports_matcher());
    }

    #[test]
    fn test_evolution_event_as_str() {
        assert_eq!(HookEvent::PostMemoryWrite.as_str(), "PostMemoryWrite");
        assert_eq!(HookEvent::MemoryLayerChange.as_str(), "MemoryLayerChange");
    }

    #[test]
    fn test_for_post_memory_write() {
        let ctx = HookContext::for_post_memory_write("build_java8", "error_resolution", "s1", "a1");
        assert_eq!(ctx.event, HookEvent::PostMemoryWrite);
        assert_eq!(ctx.memory_key.as_deref(), Some("build_java8"));
        assert_eq!(ctx.memory_source.as_deref(), Some("error_resolution"));
        assert_eq!(ctx.matcher.as_deref(), Some("error_resolution"));
    }

    #[test]
    fn test_for_memory_layer_change() {
        let ctx = HookContext::for_memory_layer_change("build_java8", "warm", "hot", "s1", "a1");
        assert_eq!(ctx.event, HookEvent::MemoryLayerChange);
        assert_eq!(ctx.memory_key.as_deref(), Some("build_java8"));
        assert_eq!(ctx.memory_from_layer.as_deref(), Some("warm"));
        assert_eq!(ctx.memory_to_layer.as_deref(), Some("hot"));
        assert_eq!(ctx.matcher.as_deref(), Some("warm->hot"));
    }

    #[test]
    fn activate_skill_defaults_to_none() {
        let r = HookResult::default();
        assert!(r.activate_skill.is_none());
    }

    #[test]
    fn activate_skill_constructor_sets_field() {
        let r =
            HookResult::with_activate_skill("docx".to_string(), "检测到 .docx 文件".to_string());
        assert_eq!(
            r.activate_skill,
            Some(("docx".to_string(), "检测到 .docx 文件".to_string()))
        );
    }

    #[test]
    fn test_subagent_stop_status_serde_and_str() {
        // Industry-aligned terminal model: SubagentStop carries a status enum,
        // there is no separate SubagentCancelled event.
        for (status, expected) in [
            (SubagentStopStatus::Completed, "completed"),
            (SubagentStopStatus::Failed, "failed"),
            (SubagentStopStatus::Cancelled, "cancelled"),
            (SubagentStopStatus::TimedOut, "timed_out"),
        ] {
            assert_eq!(status.as_str(), expected);
            let json = serde_json::to_string(&status).unwrap_or_default();
            assert_eq!(json, format!("\"{expected}\""));
        }
    }

    #[test]
    fn test_for_subagent_stop_carries_status() {
        let ctx = HookContext::for_subagent_stop(
            "coder",
            "sync",
            "done",
            SubagentStopStatus::TimedOut,
            "sess-1",
            "agent",
        );
        assert_eq!(ctx.event, HookEvent::SubagentStop);
        assert_eq!(ctx.subagent_stop_status, Some(SubagentStopStatus::TimedOut));
    }
}
