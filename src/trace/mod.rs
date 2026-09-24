//! Execution trace infrastructure — observability, replay, resumption.
//!
//! Unlike [`AgentEvent`](crate::agent::AgentEvent) which is UI-focused and streamed in real time,
//! [`Run`] is a complete record of a single agent execution suitable for storage, analytics,
//! and replay. Use [`RunStore`] to persist and query runs.
//!
//! # Quick Start
//!
//! ```rust,no_run
//! use echo_agent::trace::{InMemoryRunStore, RunStore};
//! use std::sync::Arc;
//!
//! let store = Arc::new(InMemoryRunStore::new());
//! // Attach via ReactAgentBuilder::with_run_store(store)
//! ```

pub mod analyzer;

use chrono::{DateTime, Utc};
use echo_core::utils::retention::ContentRetentionPolicy;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex, OnceLock, Weak};
use tokio::sync::{Mutex, RwLock};

pub use crate::error::Result;

// Re-export analyzer types for convenience
pub use analyzer::{
    ErrorPattern, SessionSummary, TokenBreakdown, ToolFailureClass, ToolFailurePattern,
    ToolReliabilityReport, ToolUsageStats, TraceAnalyzer,
};

// ── Run ──────────────────────────────────────────────────────────────

/// A complete execution record for a single agent invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Run {
    /// Unique run identifier.
    pub run_id: String,

    /// Parent run ID for subagent invocations.
    #[serde(default)]
    pub parent_run_id: Option<String>,

    /// Agent invocation identity.
    #[serde(default)]
    pub agent_name: String,

    /// Model used by this invocation.
    #[serde(default)]
    pub model: String,

    /// Provider used by this invocation, when known.
    #[serde(default)]
    pub provider: Option<String>,

    /// Product turn correlated with this trace invocation.
    #[serde(default)]
    pub turn_id: Option<String>,

    /// Concrete subagent/tool execution correlated with this trace invocation.
    #[serde(default)]
    pub execution_id: Option<String>,

    /// Session this run belongs to.
    pub session_id: String,

    /// Execution status.
    pub status: RunStatus,

    /// User input that triggered this run.
    pub input: String,

    /// Chronological execution events.
    pub events: Vec<RunEvent>,

    /// Final output text (set when status is Completed).
    pub final_output: Option<String>,

    /// Error message (set when status is Failed).
    pub error: Option<String>,

    /// Token usage breakdown.
    pub token_usage: TokenUsage,

    /// Timing breakdown.
    pub timings: RunTimings,

    /// When the run started.
    #[serde(with = "crate::utils::time::local_rfc3339")]
    pub started_at: DateTime<Utc>,

    /// When the run finished (set on completion, failure, or cancellation).
    #[serde(with = "crate::utils::time::option_local_rfc3339")]
    pub finished_at: Option<DateTime<Utc>>,
}

impl Run {
    /// Append an event and update the run-level aggregates derived from it.
    pub fn push_event(&mut self, event: RunEvent) {
        if let RunEvent::LlmCall {
            prompt_tokens,
            completion_tokens,
            cached_prompt_tokens,
            cache_creation_prompt_tokens,
            usage_reported,
            duration_ms,
            ..
        } = &event
        {
            self.token_usage.add_llm_call(
                *prompt_tokens,
                *completion_tokens,
                *cached_prompt_tokens,
                *cache_creation_prompt_tokens,
                *usage_reported,
            );
            self.timings.llm_duration_ms =
                self.timings.llm_duration_ms.saturating_add(*duration_ms);
        }
        self.events.push(event);
    }

    /// Apply the content-retention contract to a run before custom storage.
    ///
    /// This only sanitizes user/model/tool content: `run_id`, correlation
    /// identities, tool names, paths, enum values, counters, and timestamps
    /// remain typed diagnostic facts so a stored run can still be addressed
    /// and replayed. Custom [`RunStore`] implementations that accept direct
    /// caller writes should invoke this method before persisting a run.
    pub fn apply_retention(&mut self, retention: &ContentRetentionPolicy) {
        self.input = retention.sanitize_text(&self.input);
        if let Some(output) = self.final_output.as_mut() {
            *output = retention.sanitize_text(output);
        }
        if let Some(error) = self.error.as_mut() {
            *error = retention.sanitize_text(error);
        }
        for event in &mut self.events {
            event.apply_retention(retention);
        }
    }

    fn summary(&self) -> RunSummary {
        RunSummary {
            run_id: self.run_id.clone(),
            parent_run_id: self.parent_run_id.clone(),
            session_id: self.session_id.clone(),
            agent_name: self.agent_name.clone(),
            model: self.model.clone(),
            provider: self.provider.clone(),
            turn_id: self.turn_id.clone(),
            execution_id: self.execution_id.clone(),
            status: self.status,
            input_preview: self.input.chars().take(80).collect(),
            started_at: self.started_at,
            finished_at: self.finished_at,
            token_usage: self.token_usage,
            total_duration_ms: self.timings.total_duration_ms,
        }
    }
}

// ── RunStatus ────────────────────────────────────────────────────────

/// Execution status of a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    /// Run created but not yet started.
    Pending,
    /// Run is currently executing.
    Running,
    /// Run completed successfully.
    Completed,
    /// Run failed with an error.
    Failed,
    /// Run was cancelled.
    Cancelled,
}

/// Local estimated context distribution for one LLM request.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmContextBreakdown {
    pub system_tokens: usize,
    pub user_tokens: usize,
    pub assistant_tokens: usize,
    pub tool_tokens: usize,
    pub summary_tokens: usize,
    pub memory_tokens: usize,
}

impl LlmContextBreakdown {
    pub fn estimate(
        messages: &[echo_core::llm::types::Message],
        tokenizer: &dyn echo_core::tokenizer::Tokenizer,
    ) -> Self {
        use echo_core::llm::types::Role;

        let mut breakdown = Self::default();
        for message in messages {
            let text = message.content.as_text().unwrap_or_default();
            let tokens = tokenizer.count_tokens(text.as_str());
            if text.contains("[memory_context]")
                || text.contains("[Relevant historical memories]")
                || text.contains("[Related historical memories]")
            {
                breakdown.memory_tokens = breakdown.memory_tokens.saturating_add(tokens);
                continue;
            }
            match message.role {
                Role::System if text.contains("[对话历史摘要]") => {
                    breakdown.summary_tokens = breakdown.summary_tokens.saturating_add(tokens);
                }
                Role::System => {
                    breakdown.system_tokens = breakdown.system_tokens.saturating_add(tokens);
                }
                Role::User => {
                    breakdown.user_tokens = breakdown.user_tokens.saturating_add(tokens);
                }
                Role::Assistant => {
                    breakdown.assistant_tokens = breakdown.assistant_tokens.saturating_add(tokens);
                }
                Role::Tool => {
                    breakdown.tool_tokens = breakdown.tool_tokens.saturating_add(tokens);
                }
                Role::Custom(_) => {}
            }
        }
        breakdown
    }

    pub fn total_tokens(&self) -> usize {
        self.system_tokens
            .saturating_add(self.user_tokens)
            .saturating_add(self.assistant_tokens)
            .saturating_add(self.tool_tokens)
            .saturating_add(self.summary_tokens)
            .saturating_add(self.memory_tokens)
    }
}

// ── RunEvent ─────────────────────────────────────────────────────────

/// A discrete event within a run's execution timeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunEvent {
    /// A run budget changed the set of allowed next actions.
    BudgetDecision {
        /// Decision name: `wind_down`, `final_only`, or `hard_stop`.
        decision: String,
        /// Stable machine-readable reason.
        reason: String,
        /// Current one-based iteration.
        iteration: usize,
        /// Provider-reported input + output tokens accumulated so far.
        reported_model_tokens: usize,
        /// False when any response omitted usage metadata.
        usage_complete: bool,
    },
    /// An LLM call was made.
    LlmCall {
        /// Number of messages in the request.
        messages: usize,
        /// Prompt tokens consumed.
        prompt_tokens: u32,
        /// Completion tokens received.
        completion_tokens: u32,
        /// Prompt tokens served from provider cache.
        #[serde(default)]
        cached_prompt_tokens: u32,
        /// Prompt tokens written into provider cache.
        #[serde(default)]
        cache_creation_prompt_tokens: u32,
        /// Whether the provider returned usage metadata for this call.
        #[serde(default)]
        usage_reported: bool,
        /// Local estimate of message-context tokens before the request.
        #[serde(default)]
        estimated_context_tokens: usize,
        /// Estimated tokens pinned against context compression.
        #[serde(default)]
        protected_context_tokens: usize,
        /// Messages pinned against context compression.
        #[serde(default)]
        protected_message_count: usize,
        /// Configured context/compression limit for this invocation.
        #[serde(default)]
        context_limit_tokens: usize,
        /// Local estimate grouped by context role/source.
        #[serde(default)]
        context_breakdown: LlmContextBreakdown,
        /// Canonical cache-relevant request fingerprints.
        #[serde(default)]
        cache_fingerprint: echo_core::llm::cache::PromptCacheFingerprint,
        /// Elapsed milliseconds for this LLM call.
        duration_ms: u64,
    },
    /// Context compression completed at a meaningful run boundary.
    ContextCompression {
        source: String,
        before_messages: usize,
        after_messages: usize,
        before_tokens: usize,
        after_tokens: usize,
        #[serde(default)]
        protected_context_tokens: usize,
        #[serde(default)]
        protected_message_count: usize,
    },
    /// A tool was called.
    ToolCall {
        /// Unique call ID (matches ToolResult/ToolError).
        call_id: String,
        /// Tool name.
        name: String,
        /// Tool arguments (may be redacted for secrets).
        #[serde(default)]
        args: Option<serde_json::Value>,
        /// Risk category at call time.
        #[serde(default)]
        risk: Option<String>,
        /// Duration of the tool execution in milliseconds.
        duration_ms: u64,
    },
    /// A requested tool invocation was settled without entering execution.
    ToolExecutionSkipped {
        /// Call ID matching the requested ToolCall.
        call_id: String,
        /// Requested tool name.
        name: String,
        /// Runtime reason for closing the unstarted invocation.
        reason: String,
    },
    /// A tool returned a result.
    ToolResult {
        /// Call ID matching the ToolCall.
        call_id: String,
        /// Tool name.
        name: String,
        /// Whether the tool succeeded.
        success: bool,
        /// First 200 chars of output (for preview; full output may be large).
        #[serde(default)]
        output_preview: Option<String>,
        /// Whether the output was truncated.
        output_truncated: bool,
        /// Duration of the tool execution in milliseconds.
        #[serde(default)]
        duration_ms: u64,
        /// Original tool output size before spill/truncation.
        #[serde(default)]
        original_bytes: u64,
        /// Output size actually returned to the model.
        #[serde(default)]
        returned_bytes: u64,
        /// Estimated tokens in the original output.
        #[serde(default)]
        estimated_tokens: usize,
        /// Stable handling label: inline, truncated, spilled, or fallback.
        #[serde(default)]
        output_handling: Option<String>,
        /// Complete tool-output artifact descriptor when handling is `spilled`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        artifact: Option<echo_core::tools::artifact::ToolOutputArtifactRef>,
    },
    /// A tool returned an error.
    ToolError {
        /// Call ID matching the ToolCall.
        call_id: String,
        /// Tool name.
        name: String,
        /// Error message.
        message: String,
        /// Structured failure facts. `None` is accepted for legacy trace fixtures.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        failure: Option<crate::tools::ToolFailure>,
    },
    /// An error occurred at the run level.
    Error {
        /// Error message.
        message: String,
    },
    /// A checkpoint was saved.
    Checkpoint {
        /// Checkpoint identifier.
        id: String,
    },
    /// A persisted runtime checkpoint was restored before execution continued.
    CheckpointResumed {
        /// Conversation owning the checkpoint.
        conversation_id: String,
        /// Completed tool calls restored from paired message history.
        completed_tool_call_ids: Vec<String>,
        /// Checkpoint capture time.
        checkpoint_timestamp: DateTime<Utc>,
    },
    /// Observation of transcript projection settlement; runtime state remains authoritative.
    TranscriptProjectionSettlement {
        settlement: crate::memory::TranscriptProjectionSettlement,
    },
    /// A tool permission decision was made.
    PermissionDecision {
        /// Tool name.
        tool: String,
        /// Decision: "allow", "deny", "ask".
        decision: String,
        /// Reason for the decision.
        reason: String,
    },
    /// A file was successfully read by a tool that resolved the actual path.
    FileRead {
        /// Tool that performed the read.
        tool: String,
        /// Resolved path that was read.
        path: String,
    },
    /// A file was edited by a write tool.
    FileEdit {
        /// Tool that made the edit.
        tool: String,
        /// Path that was edited.
        path: String,
    },
    /// A test command was run.
    TestRun {
        /// The test command.
        command: String,
        /// Whether the explicitly requested test command completed successfully.
        passed: bool,
        /// Exact number of failing tests, if supplied by a structured report.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        failure_count: Option<usize>,
    },
    /// Agent turn phase transition.
    PhaseTransition {
        /// Phase name (e.g., "receive_input", "think", "act").
        phase: String,
        /// Iteration count at transition.
        iteration: usize,
    },
    /// A subagent was dispatched.
    SubagentRun {
        /// Tool call that admitted this dispatch, when invoked through a tool.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        call_id: Option<String>,
        /// Sub-agent name.
        agent_name: String,
        /// Task given to the subagent.
        task: String,
        /// Outcome: "completed", "failed", "cancelled".
        outcome: String,
    },
}

impl RunEvent {
    /// Create a [`RunEvent::ToolCall`] with secret redaction applied to args.
    pub fn new_tool_call(
        call_id: String,
        name: String,
        args: Option<serde_json::Value>,
        risk: Option<String>,
        duration_ms: u64,
    ) -> Self {
        let mut event = Self::ToolCall {
            call_id,
            name,
            args,
            risk,
            duration_ms,
        };
        event.apply_retention(&ContentRetentionPolicy::default());
        event
    }

    /// Sanitize content-bearing fields before handing this event to a custom
    /// trace backend. Typed event identities and effect metadata are kept
    /// unchanged for correlation and diagnosis.
    pub fn apply_retention(&mut self, retention: &ContentRetentionPolicy) {
        match self {
            Self::BudgetDecision { reason, .. } => {
                *reason = retention.sanitize_text(reason);
            }
            Self::LlmCall { .. } | Self::ContextCompression { .. } => {}
            Self::ToolCall { args, .. } => {
                if let Some(args) = args {
                    retention.sanitize_json(args);
                }
            }
            Self::ToolExecutionSkipped { reason, .. } => {
                *reason = retention.sanitize_text(reason);
            }
            Self::ToolResult { output_preview, .. } => {
                if let Some(preview) = output_preview {
                    *preview = retention.sanitize_text(preview);
                }
            }
            Self::ToolError {
                message, failure, ..
            } => {
                *message = retention.sanitize_text(message);
                if let Some(failure) = failure
                    && let Some(postcondition) = &mut failure.postcondition
                {
                    *postcondition = retention.sanitize_text(postcondition);
                }
            }
            Self::Error { message } => *message = retention.sanitize_text(message),
            Self::Checkpoint { .. } | Self::CheckpointResumed { .. } => {}
            Self::TranscriptProjectionSettlement { settlement } => {
                if let Some(detail) = &mut settlement.detail {
                    *detail = retention.sanitize_text(detail);
                }
            }
            Self::PermissionDecision { reason, .. } => {
                *reason = retention.sanitize_text(reason);
            }
            Self::FileRead { .. } | Self::FileEdit { .. } => {}
            Self::TestRun { command, .. } => *command = retention.sanitize_text(command),
            Self::PhaseTransition { .. } => {}
            Self::SubagentRun { task, .. } => {
                *task = retention.sanitize_text(task);
            }
        }
    }
}

pub(crate) fn skipped_tool_call_ids(events: &[RunEvent]) -> std::collections::HashSet<&str> {
    events
        .iter()
        .filter_map(|event| match event {
            RunEvent::ToolExecutionSkipped { call_id, .. } => Some(call_id.as_str()),
            _ => None,
        })
        .collect()
}

pub(crate) fn apply_run_retention(run: &mut Run, retention: &ContentRetentionPolicy) {
    run.apply_retention(retention);
}

// ── TokenUsage ───────────────────────────────────────────────────────

/// Token usage breakdown for a run.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    /// Total prompt tokens.
    pub prompt_tokens: u32,
    /// Total completion tokens.
    pub completion_tokens: u32,
    /// Total tokens (prompt + completion).
    pub total_tokens: u32,
    /// Prompt tokens served from provider cache.
    #[serde(default)]
    pub cached_prompt_tokens: u32,
    /// Prompt tokens written into provider cache.
    #[serde(default)]
    pub cache_creation_prompt_tokens: u32,
    /// LLM calls whose provider returned usage metadata.
    #[serde(default)]
    pub usage_reported_calls: u32,
    /// LLM calls whose provider omitted usage metadata.
    #[serde(default)]
    pub usage_missing_calls: u32,
}

impl TokenUsage {
    /// Accumulate additional usage into this counter.
    pub fn add(&mut self, prompt: u32, completion: u32) {
        self.prompt_tokens = self.prompt_tokens.saturating_add(prompt);
        self.completion_tokens = self.completion_tokens.saturating_add(completion);
        self.total_tokens = self
            .total_tokens
            .saturating_add(prompt.saturating_add(completion));
    }

    /// Accumulate one provider usage report with cache diagnostics.
    pub fn add_llm_call(
        &mut self,
        prompt: u32,
        completion: u32,
        cached_prompt: u32,
        cache_creation_prompt: u32,
        usage_reported: bool,
    ) {
        self.add(prompt, completion);
        self.cached_prompt_tokens = self.cached_prompt_tokens.saturating_add(cached_prompt);
        self.cache_creation_prompt_tokens = self
            .cache_creation_prompt_tokens
            .saturating_add(cache_creation_prompt);
        if usage_reported {
            self.usage_reported_calls = self.usage_reported_calls.saturating_add(1);
        } else {
            self.usage_missing_calls = self.usage_missing_calls.saturating_add(1);
        }
    }

    /// Provider-reported prompt cache read rate.
    pub fn cache_hit_rate(&self) -> Option<f64> {
        if self.prompt_tokens == 0 || self.usage_reported_calls == 0 {
            None
        } else {
            Some(self.cached_prompt_tokens as f64 / self.prompt_tokens as f64)
        }
    }
}

// ── RunTimings ───────────────────────────────────────────────────────

/// Timing breakdown for a run.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct RunTimings {
    /// Total duration in milliseconds.
    pub total_duration_ms: u64,
    /// Cumulative LLM call duration in milliseconds.
    pub llm_duration_ms: u64,
    /// Cumulative tool execution duration in milliseconds.
    pub tool_duration_ms: u64,
}

// ── RunSummary ───────────────────────────────────────────────────────

/// Lightweight summary used when listing runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSummary {
    pub run_id: String,
    #[serde(default)]
    pub parent_run_id: Option<String>,
    pub session_id: String,
    #[serde(default)]
    pub agent_name: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub turn_id: Option<String>,
    #[serde(default)]
    pub execution_id: Option<String>,
    pub status: RunStatus,
    pub input_preview: String,
    #[serde(with = "crate::utils::time::local_rfc3339")]
    pub started_at: DateTime<Utc>,
    #[serde(with = "crate::utils::time::option_local_rfc3339")]
    pub finished_at: Option<DateTime<Utc>>,
    pub token_usage: TokenUsage,
    pub total_duration_ms: u64,
}

// ── RunStore trait ───────────────────────────────────────────────────

/// Persistence backend for execution traces.
///
/// React trace producers apply the default [`ContentRetentionPolicy`] before
/// invoking custom `save`, `append_event`, and default finalization paths.
/// Implementations that override a mutation method must preserve that
/// boundary: sanitize content with [`Run::apply_retention`] and
/// [`RunEvent::apply_retention`] before storing it, while preserving typed
/// addressing fields (`run_id`, session/turn/execution IDs, call IDs, names,
/// paths, counters, and timestamps). Those fields are diagnostic facts, not
/// secret-content redaction targets; callers must avoid placing credentials in
/// an identity they expect to remain queryable.
///
/// A backend must return an error when a write is only partially accepted or
/// its durability is unknown. The producer reports that error as a separate
/// diagnostic-delivery fact and must not infer a different Agent terminal.
///
/// Built-in implementations:
/// - [`InMemoryRunStore`] — in-memory (testing, short-lived sessions)
/// - [`JsonlRunStore`] — file-based JSONL persistence (production)
#[async_trait::async_trait]
pub trait RunStore: Send + Sync {
    /// Persist a completed run.
    async fn save(&self, run: Run) -> Result<()>;

    /// Load a run by ID.
    async fn load(&self, run_id: &str) -> Result<Option<Run>>;

    /// List runs for a session, newest first.
    async fn list_by_session(&self, session_id: &str) -> Result<Vec<RunSummary>>;

    /// List all runs, newest first (limited to `limit` entries).
    async fn list_all(&self, limit: usize) -> Result<Vec<RunSummary>>;

    /// List trace invocations correlated with one product/business run.
    async fn list_by_parent_run(&self, parent_run_id: &str) -> Result<Vec<RunSummary>> {
        Ok(self
            .list_all(usize::MAX)
            .await?
            .into_iter()
            .filter(|run| run.parent_run_id.as_deref() == Some(parent_run_id))
            .collect())
    }

    /// Append a single event to an existing run (without rewriting the entire run).
    ///
    /// The default implementation loads, modifies, and saves. Implementations
    /// that support efficient append (e.g. JSONL) should override this. The
    /// compatibility path applies the default content-retention policy before
    /// calling a custom backend. Implementations overriding this method must
    /// retain the same sanitization and missing-run error contract; backends
    /// with a stricter policy may apply it again.
    async fn append_event(&self, run_id: &str, event: RunEvent) -> Result<()> {
        let mut run = self
            .load(run_id)
            .await?
            .ok_or_else(|| crate::error::ReactError::Other(format!("run '{run_id}' not found")))?;
        let mut event = event;
        event.apply_retention(&ContentRetentionPolicy::default());
        run.push_event(event);
        self.save(run).await
    }

    /// Atomically finalize one run relative to [`Self::append_event`].
    ///
    /// The compatibility default performs load/update/save. Backends that can
    /// receive concurrent event appends must override this method under the
    /// same mutation authority as `append_event`; both built-in stores do so.
    /// Returns `false` when the run does not exist.
    async fn finalize_run(
        &self,
        run_id: &str,
        status: RunStatus,
        output: Option<&str>,
        error: Option<&str>,
    ) -> Result<bool> {
        validate_terminal_status(status)?;
        let Some(mut run) = self.load(run_id).await? else {
            return Ok(false);
        };
        apply_run_finalization(
            &mut run,
            status,
            output,
            error,
            &ContentRetentionPolicy::default(),
        );
        self.save(run).await?;
        Ok(true)
    }
}

// ── InMemoryRunStore ─────────────────────────────────────────────────

/// In-memory [`RunStore`] implementation backed by a `HashMap`.
///
/// Suitable for testing and short-lived sessions. Runs are not persisted
/// across restarts.
pub struct InMemoryRunStore {
    runs: RwLock<HashMap<String, Run>>,
    retention: echo_core::utils::retention::ContentRetentionPolicy,
}

impl InMemoryRunStore {
    /// Create a new empty store.
    pub fn new() -> Self {
        Self {
            runs: RwLock::new(HashMap::new()),
            retention: echo_core::utils::retention::ContentRetentionPolicy::default(),
        }
    }

    pub fn with_retention_policy(
        mut self,
        retention: echo_core::utils::retention::ContentRetentionPolicy,
    ) -> Self {
        self.retention = retention;
        self
    }

    /// Return the number of stored runs.
    pub async fn len(&self) -> usize {
        self.runs.read().await.len()
    }

    /// Check if the store is empty.
    pub async fn is_empty(&self) -> bool {
        self.runs.read().await.is_empty()
    }
}

impl Default for InMemoryRunStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl RunStore for InMemoryRunStore {
    async fn save(&self, run: Run) -> Result<()> {
        let mut runs = self.runs.write().await;
        let mut merged = merge_run(runs.get(&run.run_id).cloned(), run);
        apply_run_retention(&mut merged, &self.retention);
        runs.insert(merged.run_id.clone(), merged);
        Ok(())
    }

    async fn load(&self, run_id: &str) -> Result<Option<Run>> {
        Ok(self.runs.read().await.get(run_id).cloned().map(|mut run| {
            apply_run_retention(&mut run, &self.retention);
            run
        }))
    }

    async fn list_by_session(&self, session_id: &str) -> Result<Vec<RunSummary>> {
        let runs = self.runs.read().await;
        let mut summaries: Vec<RunSummary> = runs
            .values()
            .filter(|r| r.session_id == session_id)
            .map(|run| {
                let mut run = run.clone();
                apply_run_retention(&mut run, &self.retention);
                run.summary()
            })
            .collect();
        summaries.sort_by_key(|s| s.started_at);
        summaries.reverse();
        Ok(summaries)
    }

    async fn list_all(&self, limit: usize) -> Result<Vec<RunSummary>> {
        let runs = self.runs.read().await;
        let mut summaries: Vec<RunSummary> = runs
            .values()
            .map(|run| {
                let mut run = run.clone();
                apply_run_retention(&mut run, &self.retention);
                run.summary()
            })
            .collect();
        summaries.sort_by_key(|s| s.started_at);
        summaries.reverse();
        summaries.truncate(limit);
        Ok(summaries)
    }

    async fn append_event(&self, run_id: &str, mut event: RunEvent) -> Result<()> {
        let mut runs = self.runs.write().await;
        let run = runs
            .get_mut(run_id)
            .ok_or_else(|| crate::error::ReactError::Other(format!("run '{run_id}' not found")))?;
        event.apply_retention(&self.retention);
        run.push_event(event);
        Ok(())
    }

    async fn finalize_run(
        &self,
        run_id: &str,
        status: RunStatus,
        output: Option<&str>,
        error: Option<&str>,
    ) -> Result<bool> {
        validate_terminal_status(status)?;
        let mut runs = self.runs.write().await;
        let Some(run) = runs.get_mut(run_id) else {
            return Ok(false);
        };
        apply_run_finalization(run, status, output, error, &self.retention);
        Ok(true)
    }
}

// ── JsonlRunStore ────────────────────────────────────────────────────

/// File-based [`RunStore`] that persists each run as a JSONL file.
///
/// Each run is stored in `{dir}/{run_id}.jsonl`: the first line is a compacted
/// [`Run`] snapshot and later lines are individual [`RunEvent`] values.
/// [`RunStore::save`] atomically compacts the file; [`RunStore::append_event`]
/// appends only one bounded event, avoiding quadratic write amplification.
///
/// Suitable for production use with persistent storage across restarts.
#[derive(Clone)]
pub struct JsonlRunStore {
    dir: PathBuf,
    shared: Arc<JsonlRunStoreData>,
    retention: echo_core::utils::retention::ContentRetentionPolicy,
    max_runs: usize,
    #[cfg(test)]
    mutation_test_hook: Option<Arc<JsonlMutationTestHook>>,
}

struct JsonlRunStoreData {
    cache: RwLock<HashMap<String, Run>>,
    mutation_lock: Mutex<()>,
}

#[cfg(test)]
#[derive(Default)]
struct JsonlMutationTestHook {
    physically_committed: tokio::sync::Notify,
    release_publish: tokio::sync::Notify,
}

fn jsonl_store_registry() -> &'static StdMutex<HashMap<PathBuf, Weak<JsonlRunStoreData>>> {
    static REGISTRY: OnceLock<StdMutex<HashMap<PathBuf, Weak<JsonlRunStoreData>>>> =
        OnceLock::new();
    REGISTRY.get_or_init(|| StdMutex::new(HashMap::new()))
}

impl JsonlRunStore {
    const DEFAULT_MAX_RUNS: usize = 1_024;
    /// Create a new store rooted at `dir`. The directory is created if it
    /// does not exist. Existing `.jsonl` files are scanned to populate the
    /// in-memory cache (only the last line of each file is loaded).
    pub fn new(dir: impl Into<PathBuf>) -> Result<Self> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        let dir = std::fs::canonicalize(&dir)?;
        let mut registry = jsonl_store_registry().lock().map_err(|error| {
            crate::error::ReactError::Other(format!("run registry poisoned: {error}"))
        })?;
        if let Some(shared) = registry.get(&dir).and_then(Weak::upgrade) {
            return Ok(Self {
                dir,
                shared,
                retention: echo_core::utils::retention::ContentRetentionPolicy::default(),
                max_runs: Self::DEFAULT_MAX_RUNS,
                #[cfg(test)]
                mutation_test_hook: None,
            });
        }
        let mut cache = HashMap::new();

        // Populate only the newest bounded set and remove expired run logs.
        let mut paths = std::fs::read_dir(&dir)?
            .map(|entry| {
                let entry = entry?;
                if entry.file_type()?.is_symlink() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("run store contains a symlink: {}", entry.path().display()),
                    ));
                }
                Ok(entry.path())
            })
            .collect::<std::io::Result<Vec<_>>>()?
            .into_iter()
            .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
            .collect::<Vec<_>>();
        paths.sort_by_key(|path| {
            std::fs::metadata(path)
                .and_then(|metadata| metadata.modified())
                .ok()
        });
        if paths.len() > Self::DEFAULT_MAX_RUNS {
            let expired = paths.len().saturating_sub(Self::DEFAULT_MAX_RUNS);
            for path in paths.drain(..expired) {
                std::fs::remove_file(path)?;
            }
        }
        for path in paths {
            let loaded = (|| -> Result<Run> {
                let run = Self::load_run(&path)?;
                let expected =
                    path.file_stem()
                        .and_then(|name| name.to_str())
                        .ok_or_else(|| {
                            crate::error::ReactError::Other(
                                "run filename is not valid UTF-8".to_string(),
                            )
                        })?;
                if run.run_id != expected {
                    return Err(crate::error::ReactError::Other(
                        "run identity mismatch in persisted trace".to_string(),
                    ));
                }
                Ok(run)
            })();
            match loaded {
                Ok(run) => {
                    cache.insert(run.run_id.clone(), run);
                }
                Err(error) => quarantine_corrupt_run_log(&path, &error),
            }
        }

        let shared = Arc::new(JsonlRunStoreData {
            cache: RwLock::new(cache),
            mutation_lock: Mutex::new(()),
        });
        registry.insert(dir.clone(), Arc::downgrade(&shared));

        Ok(Self {
            dir,
            shared,
            retention: echo_core::utils::retention::ContentRetentionPolicy::default(),
            max_runs: Self::DEFAULT_MAX_RUNS,
            #[cfg(test)]
            mutation_test_hook: None,
        })
    }

    pub fn with_retention_policy(
        mut self,
        retention: echo_core::utils::retention::ContentRetentionPolicy,
    ) -> Self {
        self.retention = retention;
        self
    }

    /// Bound both durable run files and the in-memory cache.
    pub fn with_max_runs(mut self, max_runs: usize) -> Self {
        self.max_runs = max_runs.max(1);
        self
    }

    #[cfg(test)]
    fn set_mutation_test_hook(&mut self, hook: Option<Arc<JsonlMutationTestHook>>) {
        self.mutation_test_hook = hook;
    }

    #[cfg(test)]
    async fn wait_before_cache_publish(&self) {
        if let Some(hook) = &self.mutation_test_hook {
            hook.physically_committed.notify_one();
            hook.release_publish.notified().await;
        }
    }

    /// Return the file path for a given run ID.
    fn run_path(&self, run_id: &str) -> Result<PathBuf> {
        let name = format!("{run_id}.jsonl");
        Ok(echo_core::utils::fs::join_path_segment(&self.dir, &name)?)
    }

    fn parse_run_log(data: &str, _path: &Path) -> Result<Run> {
        let has_partial_tail = !data.ends_with('\n');
        let lines = data
            .lines()
            .filter(|line| !line.trim().is_empty())
            .collect::<Vec<_>>();
        let first = lines
            .first()
            .ok_or_else(|| crate::error::ReactError::Other("empty run file".to_string()))?;
        let mut run = serde_json::from_str::<Run>(first).map_err(trace_decode_error)?;
        for (index, line) in lines.iter().enumerate().skip(1) {
            match serde_json::from_str::<RunEvent>(line) {
                Ok(event) => run.push_event(event),
                Err(error) if has_partial_tail && index.saturating_add(1) == lines.len() => {
                    tracing::warn!(
                        error_category = ?error.classify(),
                        error_column = error.column(),
                        "ignoring truncated run event tail"
                    );
                }
                Err(error) => return Err(trace_decode_error(error)),
            }
        }
        Ok(run)
    }

    fn load_run(path: &Path) -> Result<Run> {
        let data = std::fs::read_to_string(path)?;
        Self::parse_run_log(&data, path)
    }

    async fn load_run_async(path: &Path) -> Result<Run> {
        let data = tokio::fs::read_to_string(path).await?;
        Self::parse_run_log(&data, path)
    }

    async fn persist_unlocked(&self, mut run: Run) -> Result<()> {
        apply_run_retention(&mut run, &self.retention);
        let run_id = run.run_id.clone();
        let path = self.run_path(&run_id)?;
        let mut bytes = serde_json::to_vec(&run)?;
        bytes.push(b'\n');
        let path_for_write = path.clone();
        tokio::task::spawn_blocking(move || {
            echo_core::utils::fs::atomic_write(&path_for_write, &bytes)
        })
        .await
        .map_err(|error| {
            crate::error::ReactError::Other(format!("run writer join failed: {error}"))
        })??;
        #[cfg(test)]
        self.wait_before_cache_publish().await;
        self.shared.cache.write().await.insert(run_id, run);
        self.prune_unlocked().await?;
        Ok(())
    }

    async fn prune_unlocked(&self) -> Result<()> {
        let mut cache = self.shared.cache.write().await;
        while cache.len() > self.max_runs {
            let Some(oldest) = cache
                .values()
                .min_by_key(|run| run.started_at)
                .map(|run| run.run_id.clone())
            else {
                break;
            };
            cache.remove(&oldest);
            let path = self.run_path(&oldest)?;
            match tokio::fs::remove_file(&path).await {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }
}

fn trace_decode_error(error: serde_json::Error) -> crate::error::ReactError {
    crate::error::ReactError::Other(format!(
        "invalid trace record: {:?} at line {} column {}",
        error.classify(),
        error.line(),
        error.column()
    ))
}

fn quarantine_corrupt_run_log(path: &Path, _error: &dyn std::fmt::Display) {
    let target = path.with_extension(format!("jsonl.corrupt-{}", uuid::Uuid::new_v4()));
    match std::fs::rename(path, &target) {
        Ok(()) => tracing::warn!(
            error_category = "invalid_run_log",
            "isolated unreadable run log"
        ),
        Err(rename_error) => tracing::warn!(
            error_category = "invalid_run_log",
            error_kind = ?rename_error.kind(),
            "failed to isolate unreadable run log"
        ),
    }
}

#[async_trait::async_trait]
impl RunStore for JsonlRunStore {
    async fn save(&self, run: Run) -> Result<()> {
        let store = self.clone();
        tokio::spawn(async move {
            let _guard = store.shared.mutation_lock.lock().await;
            let existing = store.shared.cache.read().await.get(&run.run_id).cloned();
            store.persist_unlocked(merge_run(existing, run)).await
        })
        .await
        .map_err(|error| {
            crate::error::ReactError::Other(format!("owned run save join failed: {error}"))
        })?
    }

    async fn load(&self, run_id: &str) -> Result<Option<Run>> {
        // Check cache first
        if let Some(mut run) = self.shared.cache.read().await.get(run_id).cloned() {
            apply_run_retention(&mut run, &self.retention);
            return Ok(Some(run));
        }
        // Fall back to disk (async)
        let path = self.run_path(run_id)?;
        if tokio::fs::try_exists(&path).await? {
            let mut run = Self::load_run_async(&path).await?;
            if run.run_id != run_id {
                return Err(crate::error::ReactError::Other(
                    "run identity mismatch in persisted trace".to_string(),
                ));
            }
            apply_run_retention(&mut run, &self.retention);
            self.shared
                .cache
                .write()
                .await
                .insert(run_id.to_string(), run.clone());
            return Ok(Some(run));
        }
        Ok(None)
    }

    async fn list_by_session(&self, session_id: &str) -> Result<Vec<RunSummary>> {
        let cache = self.shared.cache.read().await;
        let mut summaries: Vec<RunSummary> = cache
            .values()
            .filter(|r| r.session_id == session_id)
            .map(|run| {
                let mut run = run.clone();
                apply_run_retention(&mut run, &self.retention);
                run.summary()
            })
            .collect();
        summaries.sort_by_key(|s| s.started_at);
        summaries.reverse();
        Ok(summaries)
    }

    async fn list_all(&self, limit: usize) -> Result<Vec<RunSummary>> {
        let cache = self.shared.cache.read().await;
        let mut summaries: Vec<RunSummary> = cache
            .values()
            .map(|run| {
                let mut run = run.clone();
                apply_run_retention(&mut run, &self.retention);
                run.summary()
            })
            .collect();
        summaries.sort_by_key(|s| s.started_at);
        summaries.reverse();
        summaries.truncate(limit);
        Ok(summaries)
    }

    /// Append one sanitized event line without rewriting the accumulated run.
    async fn append_event(&self, run_id: &str, event: RunEvent) -> Result<()> {
        let store = self.clone();
        let run_id = run_id.to_string();
        tokio::spawn(async move {
            let _guard = store.shared.mutation_lock.lock().await;
            let mut run = match store.shared.cache.read().await.get(&run_id).cloned() {
                Some(run) => run,
                None => {
                    return Err(crate::error::ReactError::Other(format!(
                        "run '{run_id}' not found"
                    )));
                }
            };
            let mut safe_event = event;
            safe_event.apply_retention(&store.retention);
            let mut bytes = serde_json::to_vec(&safe_event)?;
            bytes.push(b'\n');
            let path = store.run_path(&run_id)?;
            tokio::task::spawn_blocking(move || -> std::io::Result<()> {
                echo_core::utils::fs::append_existing(
                    &path,
                    &bytes,
                    echo_core::utils::fs::FileDurability::SyncData,
                )
            })
            .await
            .map_err(|error| {
                crate::error::ReactError::Other(format!("run event writer join failed: {error}"))
            })??;
            #[cfg(test)]
            store.wait_before_cache_publish().await;
            run.push_event(safe_event);
            store.shared.cache.write().await.insert(run_id, run);
            Ok(())
        })
        .await
        .map_err(|error| {
            crate::error::ReactError::Other(format!("owned run append join failed: {error}"))
        })?
    }

    async fn finalize_run(
        &self,
        run_id: &str,
        status: RunStatus,
        output: Option<&str>,
        error: Option<&str>,
    ) -> Result<bool> {
        validate_terminal_status(status)?;
        let store = self.clone();
        let run_id = run_id.to_string();
        let output = output.map(str::to_string);
        let error = error.map(str::to_string);
        tokio::spawn(async move {
            let _guard = store.shared.mutation_lock.lock().await;
            let Some(mut run) = store.shared.cache.read().await.get(&run_id).cloned() else {
                return Ok(false);
            };
            apply_run_finalization(
                &mut run,
                status,
                output.as_deref(),
                error.as_deref(),
                &store.retention,
            );
            store.persist_unlocked(run).await?;
            Ok(true)
        })
        .await
        .map_err(|error| {
            crate::error::ReactError::Other(format!("owned run finalize join failed: {error}"))
        })?
    }
}

fn apply_run_finalization(
    run: &mut Run,
    status: RunStatus,
    output: Option<&str>,
    error: Option<&str>,
    retention: &ContentRetentionPolicy,
) {
    if is_terminal(run.status) {
        return;
    }
    run.status = status;
    run.final_output = output.map(str::to_string);
    run.error = error.map(str::to_string);
    if status == RunStatus::Failed
        && !run
            .events
            .iter()
            .any(|event| matches!(event, RunEvent::Error { .. }))
        && let Some(message) = run.error.clone()
    {
        run.push_event(RunEvent::Error { message });
    }
    run.finished_at = Some(Utc::now());
    apply_run_retention(run, retention);
}

fn merge_run(existing: Option<Run>, mut incoming: Run) -> Run {
    let Some(existing) = existing else {
        return incoming;
    };
    if existing.events.len() > incoming.events.len() {
        incoming.events = existing.events;
        incoming.token_usage = existing.token_usage;
        incoming.timings.llm_duration_ms = existing.timings.llm_duration_ms;
    }
    if is_terminal(existing.status) {
        incoming.status = existing.status;
        incoming.final_output = existing.final_output;
        incoming.error = existing.error;
        incoming.finished_at = existing.finished_at;
    }
    incoming
}

fn is_terminal(status: RunStatus) -> bool {
    matches!(
        status,
        RunStatus::Completed | RunStatus::Failed | RunStatus::Cancelled
    )
}

fn validate_terminal_status(status: RunStatus) -> Result<()> {
    if is_terminal(status) {
        Ok(())
    } else {
        Err(crate::error::ReactError::Other(format!(
            "run finalization requires a terminal status, got {status:?}"
        )))
    }
}

// ── Unit tests ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::{
        DiagnosticDeliveryFailure, DiagnosticDeliveryObserver, DiagnosticDeliveryOperation,
        DiagnosticRecordKind,
    };
    use crate::error::ReactError;
    use std::sync::Mutex as StdMutex;

    #[test]
    fn run_event_contract_matrix_covers_all_variants() -> Result<()> {
        // Keep this exhaustive match next to the serialized contract matrix:
        // adding a RunEvent variant without adding its documented discriminator
        // and producer evidence must fail this test at compile time.
        let variant_name = |event: &RunEvent| match event {
            RunEvent::BudgetDecision { .. } => "budget_decision",
            RunEvent::LlmCall { .. } => "llm_call",
            RunEvent::ContextCompression { .. } => "context_compression",
            RunEvent::ToolCall { .. } => "tool_call",
            RunEvent::ToolExecutionSkipped { .. } => "tool_execution_skipped",
            RunEvent::ToolResult { .. } => "tool_result",
            RunEvent::ToolError { .. } => "tool_error",
            RunEvent::Error { .. } => "error",
            RunEvent::Checkpoint { .. } => "checkpoint",
            RunEvent::CheckpointResumed { .. } => "checkpoint_resumed",
            RunEvent::TranscriptProjectionSettlement { .. } => "transcript_projection_settlement",
            RunEvent::PermissionDecision { .. } => "permission_decision",
            RunEvent::FileRead { .. } => "file_read",
            RunEvent::FileEdit { .. } => "file_edit",
            RunEvent::TestRun { .. } => "test_run",
            RunEvent::PhaseTransition { .. } => "phase_transition",
            RunEvent::SubagentRun { .. } => "subagent_run",
        };

        let events = vec![
            (
                "budget_decision",
                RunEvent::BudgetDecision {
                    decision: "wind_down".to_string(),
                    reason: "iteration_wind_down".to_string(),
                    iteration: 1,
                    reported_model_tokens: 1,
                    usage_complete: true,
                },
            ),
            (
                "llm_call",
                RunEvent::LlmCall {
                    messages: 1,
                    prompt_tokens: 1,
                    completion_tokens: 1,
                    cached_prompt_tokens: 0,
                    cache_creation_prompt_tokens: 0,
                    usage_reported: true,
                    estimated_context_tokens: 1,
                    protected_context_tokens: 0,
                    protected_message_count: 0,
                    context_limit_tokens: 1,
                    context_breakdown: LlmContextBreakdown::default(),
                    cache_fingerprint: echo_core::llm::cache::PromptCacheFingerprint::default(),
                    duration_ms: 1,
                },
            ),
            (
                "context_compression",
                RunEvent::ContextCompression {
                    source: "test".to_string(),
                    before_messages: 2,
                    after_messages: 1,
                    before_tokens: 2,
                    after_tokens: 1,
                    protected_context_tokens: 0,
                    protected_message_count: 0,
                },
            ),
            (
                "tool_call",
                RunEvent::ToolCall {
                    call_id: "call-1".to_string(),
                    name: "read_file".to_string(),
                    args: None,
                    risk: None,
                    duration_ms: 1,
                },
            ),
            (
                "tool_execution_skipped",
                RunEvent::ToolExecutionSkipped {
                    call_id: "call-1".to_string(),
                    name: "read_file".to_string(),
                    reason: "cancelled".to_string(),
                },
            ),
            (
                "tool_result",
                RunEvent::ToolResult {
                    call_id: "call-1".to_string(),
                    name: "read_file".to_string(),
                    success: true,
                    output_preview: Some("ok".to_string()),
                    output_truncated: false,
                    duration_ms: 1,
                    original_bytes: 2,
                    returned_bytes: 2,
                    estimated_tokens: 1,
                    output_handling: Some("inline".to_string()),
                    artifact: None,
                },
            ),
            (
                "tool_error",
                RunEvent::ToolError {
                    call_id: "call-1".to_string(),
                    name: "read_file".to_string(),
                    message: "failed".to_string(),
                    failure: None,
                },
            ),
            (
                "error",
                RunEvent::Error {
                    message: "failed".to_string(),
                },
            ),
            (
                "checkpoint",
                RunEvent::Checkpoint {
                    id: "checkpoint-1".to_string(),
                },
            ),
            (
                "checkpoint_resumed",
                RunEvent::CheckpointResumed {
                    conversation_id: "conversation-1".to_string(),
                    completed_tool_call_ids: vec!["call-1".to_string()],
                    checkpoint_timestamp: Utc::now(),
                },
            ),
            (
                "transcript_projection_settlement",
                RunEvent::TranscriptProjectionSettlement {
                    settlement: crate::memory::TranscriptProjectionSettlement {
                        status: crate::memory::TranscriptProjectionSettlementStatus::Settled,
                        operation_id: Some("operation-1".to_string()),
                        conversation_id: Some("conversation-1".to_string()),
                        generation_id: Some("generation-1".to_string()),
                        attempt: 1,
                        error_class: None,
                        detail: None,
                    },
                },
            ),
            (
                "permission_decision",
                RunEvent::PermissionDecision {
                    tool: "read_file".to_string(),
                    decision: "allow".to_string(),
                    reason: "policy".to_string(),
                },
            ),
            (
                "file_read",
                RunEvent::FileRead {
                    tool: "read_file".to_string(),
                    path: "src/lib.rs".to_string(),
                },
            ),
            (
                "file_edit",
                RunEvent::FileEdit {
                    tool: "write_file".to_string(),
                    path: "src/lib.rs".to_string(),
                },
            ),
            (
                "test_run",
                RunEvent::TestRun {
                    command: "cargo test".to_string(),
                    passed: true,
                    failure_count: Some(0),
                },
            ),
            (
                "phase_transition",
                RunEvent::PhaseTransition {
                    phase: "think".to_string(),
                    iteration: 1,
                },
            ),
            (
                "subagent_run",
                RunEvent::SubagentRun {
                    call_id: Some("call-2".to_string()),
                    agent_name: "reviewer".to_string(),
                    task: "review".to_string(),
                    outcome: "completed".to_string(),
                },
            ),
        ];

        assert_eq!(events.len(), 17);
        for (expected, event) in events {
            assert_eq!(variant_name(&event), expected);
            let value = serde_json::to_value(&event)?;
            let actual = value
                .get("type")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| ReactError::Other("RunEvent discriminator missing".to_string()))?;
            assert_eq!(actual, expected);
        }
        Ok(())
    }

    struct MissingRunStore;

    #[async_trait::async_trait]
    impl RunStore for MissingRunStore {
        async fn save(&self, _run: Run) -> Result<()> {
            Ok(())
        }

        async fn load(&self, _run_id: &str) -> Result<Option<Run>> {
            Ok(None)
        }

        async fn list_by_session(&self, _session_id: &str) -> Result<Vec<RunSummary>> {
            Ok(Vec::new())
        }

        async fn list_all(&self, _limit: usize) -> Result<Vec<RunSummary>> {
            Ok(Vec::new())
        }
    }

    /// 保存首次成功（trace start），之后每次 save 都失败：直接复现
    /// "load 成功、最终 trace save 失败" 的 Finalize 场景。
    struct FinalizeSaveFailingRunStore {
        remaining_successes: StdMutex<u32>,
        last_saved: StdMutex<Option<Run>>,
    }

    impl FinalizeSaveFailingRunStore {
        fn new(initial_successes: u32) -> Self {
            Self {
                remaining_successes: StdMutex::new(initial_successes),
                last_saved: StdMutex::new(None),
            }
        }
    }

    #[async_trait::async_trait]
    impl RunStore for FinalizeSaveFailingRunStore {
        async fn save(&self, run: Run) -> Result<()> {
            let mut remaining = self
                .remaining_successes
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if *remaining == 0 {
                return Err(ReactError::Other(
                    "injected finalize save failure".to_string(),
                ));
            }
            *remaining -= 1;
            *self
                .last_saved
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = Some(run);
            Ok(())
        }

        async fn load(&self, run_id: &str) -> Result<Option<Run>> {
            Ok(self
                .last_saved
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clone()
                .filter(|run| run.run_id == run_id))
        }

        async fn list_by_session(&self, _session_id: &str) -> Result<Vec<RunSummary>> {
            Ok(Vec::new())
        }

        async fn list_all(&self, _limit: usize) -> Result<Vec<RunSummary>> {
            Ok(Vec::new())
        }
    }

    struct SaveFailingRunStore;

    #[async_trait::async_trait]
    impl RunStore for SaveFailingRunStore {
        async fn save(&self, _run: Run) -> Result<()> {
            Err(ReactError::Other(
                "injected trace start failure".to_string(),
            ))
        }

        async fn load(&self, _run_id: &str) -> Result<Option<Run>> {
            Ok(None)
        }

        async fn list_by_session(&self, _session_id: &str) -> Result<Vec<RunSummary>> {
            Ok(Vec::new())
        }

        async fn list_all(&self, _limit: usize) -> Result<Vec<RunSummary>> {
            Ok(Vec::new())
        }
    }

    #[derive(Default)]
    struct PartiallyFailingRunStore {
        saved: StdMutex<Vec<Run>>,
    }

    #[async_trait::async_trait]
    impl RunStore for PartiallyFailingRunStore {
        async fn save(&self, run: Run) -> Result<()> {
            self.saved
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(run);
            Err(ReactError::Other(
                "injected partial trace persistence failure".to_string(),
            ))
        }

        async fn load(&self, _run_id: &str) -> Result<Option<Run>> {
            Ok(None)
        }

        async fn list_by_session(&self, _session_id: &str) -> Result<Vec<RunSummary>> {
            Ok(Vec::new())
        }

        async fn list_all(&self, _limit: usize) -> Result<Vec<RunSummary>> {
            Ok(Vec::new())
        }
    }

    #[derive(Default)]
    struct RecordingRunStore {
        runs: StdMutex<HashMap<String, Run>>,
    }

    #[async_trait::async_trait]
    impl RunStore for RecordingRunStore {
        async fn save(&self, run: Run) -> Result<()> {
            self.runs
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(run.run_id.clone(), run);
            Ok(())
        }

        async fn load(&self, run_id: &str) -> Result<Option<Run>> {
            Ok(self
                .runs
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .get(run_id)
                .cloned())
        }

        async fn list_by_session(&self, _session_id: &str) -> Result<Vec<RunSummary>> {
            Ok(Vec::new())
        }

        async fn list_all(&self, _limit: usize) -> Result<Vec<RunSummary>> {
            Ok(Vec::new())
        }
    }

    #[derive(Default)]
    struct RecordingObserver {
        failures: StdMutex<Vec<DiagnosticDeliveryFailure>>,
        changed: std::sync::Condvar,
    }

    impl DiagnosticDeliveryObserver for RecordingObserver {
        fn on_failure(&self, failure: DiagnosticDeliveryFailure) {
            self.failures
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(failure);
            self.changed.notify_all();
        }
    }

    impl RecordingObserver {
        fn wait_for_count(&self, count: usize) -> Result<Vec<DiagnosticDeliveryFailure>> {
            let failures = self
                .failures
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let (failures, wait) = self
                .changed
                .wait_timeout_while(failures, std::time::Duration::from_secs(5), |failures| {
                    failures.len() < count
                })
                .map_err(|error| ReactError::Other(format!("observer wait failed: {error}")))?;
            if wait.timed_out() && failures.len() < count {
                return Err(ReactError::Other(format!(
                    "timed out waiting for {count} diagnostic failures"
                )));
            }
            Ok(failures.clone())
        }
    }

    fn make_run(id: &str, session: &str) -> Run {
        Run {
            run_id: id.to_string(),
            parent_run_id: None,
            agent_name: String::new(),
            model: String::new(),
            provider: None,
            turn_id: None,
            execution_id: None,
            session_id: session.to_string(),
            status: RunStatus::Completed,
            input: "test input".to_string(),
            events: vec![],
            final_output: Some("ok".to_string()),
            error: None,
            token_usage: TokenUsage::default(),
            timings: RunTimings::default(),
            started_at: Utc::now(),
            finished_at: Some(Utc::now()),
        }
    }

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("echo_trace_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn corrupt_trace_logs_do_not_render_secret_like_run_paths() -> Result<()> {
        #[derive(Clone)]
        struct CaptureLogWriter(Arc<StdMutex<Vec<u8>>>);

        impl std::io::Write for CaptureLogWriter {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .extend_from_slice(bytes);
                Ok(bytes.len())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let directory = tempfile::tempdir()?;
        let path = directory.path().join("token=supersecretvalue.jsonl");
        std::fs::write(&path, "invalid trace")?;
        let output = Arc::new(StdMutex::new(Vec::new()));
        let captured = Arc::clone(&output);
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .without_time()
            .with_max_level(tracing::Level::WARN)
            .with_writer(move || CaptureLogWriter(Arc::clone(&captured)))
            .finish();
        let valid_run = serde_json::to_string(&make_run("other", "session"))?;
        tracing::subscriber::with_default(subscriber, || {
            quarantine_corrupt_run_log(&path, &ReactError::Other("invalid".to_string()));
            let _ignored =
                JsonlRunStore::parse_run_log(&format!("{valid_run}\n{{invalid-tail"), &path);
        });
        let logs = String::from_utf8(
            output
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clone(),
        )
        .map_err(|error| ReactError::Other(error.to_string()))?;
        assert!(logs.contains("isolated unreadable run log"));
        assert!(logs.contains("ignoring truncated run event tail"));
        assert!(!logs.contains("supersecretvalue"));
        assert!(!logs.contains("token="));
        Ok(())
    }

    #[test]
    fn trace_retention_zero_limits_preserve_typed_event_and_redact_args() -> Result<()> {
        let mut event = RunEvent::ToolCall {
            call_id: "call_identity".into(),
            name: "tool_identity".into(),
            args: Some(serde_json::json!({"nested": {"password": "tiny-secret"}})),
            risk: Some("high".into()),
            duration_ms: 42,
        };
        event.apply_retention(&ContentRetentionPolicy {
            max_string_chars: 0,
            max_array_items: 0,
        });
        let value = serde_json::to_value(&event)?;
        assert!(!value.to_string().contains("tiny-secret"));
        assert_eq!(
            value.get("call_id"),
            Some(&serde_json::json!("call_identity"))
        );
        assert_eq!(value.get("duration_ms"), Some(&serde_json::json!(42)));
        assert!(matches!(
            serde_json::from_value::<RunEvent>(value)?,
            RunEvent::ToolCall { .. }
        ));
        Ok(())
    }

    #[tokio::test]
    async fn default_append_rejects_a_missing_run() -> Result<()> {
        let result = MissingRunStore
            .append_event("missing", RunEvent::Checkpoint { id: "one".into() })
            .await;
        let error = match result {
            Ok(()) => {
                return Err(ReactError::Other(
                    "missing run append unexpectedly succeeded".to_string(),
                ));
            }
            Err(error) => error,
        };
        assert!(error.to_string().contains("run 'missing' not found"));
        Ok(())
    }

    #[tokio::test]
    async fn failed_trace_start_reports_delivery_and_does_not_publish_run_id() -> Result<()> {
        let observer = Arc::new(RecordingObserver::default());
        let mut agent = crate::agent::ReactAgent::new(crate::agent::AgentConfig::new(
            "model", "agent", "system",
        ));
        agent.run_store = Some(Arc::new(InMemoryRunStore::new()));
        let legacy = agent.capture_legacy_external_context();
        let previous = agent
            .start_legacy_trace_run("previous", &legacy)
            .await
            .ok_or_else(|| ReactError::Other("previous trace did not start".to_string()))?;
        assert_eq!(agent.capture_current_trace_run_id(), Some(previous));

        agent.run_store = Some(Arc::new(SaveFailingRunStore));
        agent.set_diagnostic_delivery_observer(observer.clone());

        let run_id = agent.start_legacy_trace_run("input", &legacy).await;

        assert!(run_id.is_none());
        assert!(agent.capture_current_trace_run_id().is_none());
        let failures = observer.wait_for_count(1)?;
        let failure = failures
            .first()
            .ok_or_else(|| ReactError::Other("missing trace delivery failure".to_string()))?;
        assert_eq!(failure.record_kind, DiagnosticRecordKind::Trace);
        assert_eq!(failure.operation, DiagnosticDeliveryOperation::Start);
        assert!(
            failure
                .record_id
                .as_deref()
                .is_some_and(|id| id.starts_with("run_"))
        );
        assert!(failure.error.contains("injected trace start failure"));
        Ok(())
    }

    #[tokio::test]
    async fn custom_run_store_gets_retained_copy_before_partial_failure() -> Result<()> {
        let observer = Arc::new(RecordingObserver::default());
        let store = Arc::new(PartiallyFailingRunStore::default());
        let mut agent = crate::agent::ReactAgent::new(crate::agent::AgentConfig::new(
            "model", "agent", "system",
        ));
        agent.run_store = Some(store.clone());
        agent.set_diagnostic_delivery_observer(observer.clone());

        let legacy = agent.capture_legacy_external_context();
        let run_id = agent
            .start_legacy_trace_run("password: raw-partial-secret-value", &legacy)
            .await;

        assert!(run_id.is_none());
        let saved = store
            .saved
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .first()
            .cloned()
            .ok_or_else(|| ReactError::Other("partial backend did not receive run".into()))?;
        assert!(!saved.input.contains("raw-partial-secret-value"));
        assert!(saved.input.contains("[REDACTED]"));
        assert!(saved.run_id.starts_with("run_"));

        let failures = observer.wait_for_count(1)?;
        let failure = failures
            .first()
            .ok_or_else(|| ReactError::Other("missing partial-write failure".into()))?;
        assert_eq!(failure.operation, DiagnosticDeliveryOperation::Start);
        assert!(failure.error.contains("partial trace persistence failure"));
        Ok(())
    }

    #[tokio::test]
    async fn default_custom_finalization_retains_output_and_error() -> Result<()> {
        let store = RecordingRunStore::default();
        let mut run = make_run("custom-finalize", "session");
        run.status = RunStatus::Running;
        run.finished_at = None;
        run.final_output = None;
        store.save(run).await?;

        assert!(
            store
                .finalize_run(
                    "custom-finalize",
                    RunStatus::Failed,
                    Some("Bearer abcdefghijklmnopqrstuvwxyz"),
                    Some("password: raw-finalize-secret"),
                )
                .await?
        );
        let finalized = store
            .load("custom-finalize")
            .await?
            .ok_or_else(|| ReactError::Other("custom finalized run missing".into()))?;
        assert_eq!(finalized.status, RunStatus::Failed);
        assert_eq!(finalized.final_output.as_deref(), Some("[REDACTED]"));
        assert_eq!(finalized.error.as_deref(), Some("[REDACTED]"));
        assert!(
            finalized.events.iter().any(
                |event| matches!(event, RunEvent::Error { message } if message == "[REDACTED]")
            )
        );
        Ok(())
    }

    #[tokio::test]
    async fn append_and_finalize_failures_are_observed_without_trace_terminal_authority()
    -> Result<()> {
        let observer = Arc::new(RecordingObserver::default());
        let mut agent = crate::agent::ReactAgent::new(crate::agent::AgentConfig::new(
            "model", "agent", "system",
        ));
        agent.run_store = Some(Arc::new(MissingRunStore));
        agent.set_diagnostic_delivery_observer(observer.clone());
        let legacy = agent.capture_legacy_external_context();
        let run_id = agent
            .start_legacy_trace_run("input", &legacy)
            .await
            .ok_or_else(|| ReactError::Other("trace start did not return an id".to_string()))?;

        agent
            .record_trace_event(RunEvent::Checkpoint { id: "one".into() })
            .await;
        let snapshot = crate::agent::AgentRunSnapshot::from_agent(&agent);
        assert_eq!(snapshot.trace_run_id.as_deref(), Some(run_id.as_str()));
        snapshot
            .finalize_run(RunStatus::Completed, Some("answer"), None)
            .await;

        let failures = observer.wait_for_count(2)?;
        assert_eq!(failures.len(), 2);
        assert_eq!(
            failures.first().map(|failure| failure.operation),
            Some(DiagnosticDeliveryOperation::Append)
        );
        assert_eq!(
            failures.get(1).map(|failure| failure.operation),
            Some(DiagnosticDeliveryOperation::Load)
        );
        Ok(())
    }

    #[tokio::test]
    async fn final_trace_save_failure_reports_finalize_operation() -> Result<()> {
        let observer = Arc::new(RecordingObserver::default());
        let mut agent = crate::agent::ReactAgent::new(crate::agent::AgentConfig::new(
            "model", "agent", "system",
        ));
        agent.run_store = Some(Arc::new(FinalizeSaveFailingRunStore::new(1)));
        agent.set_diagnostic_delivery_observer(observer.clone());
        let legacy = agent.capture_legacy_external_context();
        let run_id = agent
            .start_legacy_trace_run("input", &legacy)
            .await
            .ok_or_else(|| ReactError::Other("trace start did not return an id".to_string()))?;

        // Canonical finalizer: load succeeds, the final save fails, and the
        // producer terminal stays untouched while a Finalize fact is emitted.
        let snapshot = crate::agent::AgentRunSnapshot::from_agent(&agent);
        assert_eq!(snapshot.trace_run_id.as_deref(), Some(run_id.as_str()));
        snapshot
            .finalize_run(RunStatus::Completed, Some("answer"), None)
            .await;

        let failures = observer.wait_for_count(1)?;
        let failure = failures
            .first()
            .ok_or_else(|| ReactError::Other("missing finalize delivery failure".to_string()))?;
        assert_eq!(failure.record_kind, DiagnosticRecordKind::Trace);
        assert_eq!(failure.operation, DiagnosticDeliveryOperation::Finalize);
        assert_eq!(failure.record_id.as_deref(), Some(run_id.as_str()));
        assert!(failure.error.contains("injected finalize save failure"));
        Ok(())
    }

    async fn assert_atomic_finalization_preserves_late_event(
        store: Arc<dyn RunStore>,
        run_id: &str,
    ) -> Result<()> {
        let mut run = make_run(run_id, "session-finalize-race");
        run.status = RunStatus::Running;
        run.finished_at = None;
        run.final_output = None;
        store.save(run).await?;
        store
            .append_event(
                run_id,
                RunEvent::SubagentRun {
                    call_id: Some("background-call".to_string()),
                    agent_name: "reviewer".to_string(),
                    task: "late result".to_string(),
                    outcome: "completed".to_string(),
                },
            )
            .await?;
        assert!(
            store
                .finalize_run(run_id, RunStatus::Failed, None, Some("parent failed"),)
                .await?
        );
        assert!(
            store
                .finalize_run(run_id, RunStatus::Failed, None, Some("duplicate finalize"),)
                .await?
        );
        let finalized = store
            .load(run_id)
            .await?
            .ok_or_else(|| ReactError::Other("finalized run missing".to_string()))?;
        assert_eq!(finalized.status, RunStatus::Failed);
        assert_eq!(finalized.error.as_deref(), Some("parent failed"));
        assert_eq!(
            finalized
                .events
                .iter()
                .filter(|event| matches!(event, RunEvent::SubagentRun { .. }))
                .count(),
            1
        );
        assert_eq!(
            finalized
                .events
                .iter()
                .filter(|event| matches!(event, RunEvent::Error { .. }))
                .count(),
            1
        );
        Ok(())
    }

    #[tokio::test]
    async fn built_in_run_stores_finalize_under_the_append_authority() -> Result<()> {
        assert_atomic_finalization_preserves_late_event(
            Arc::new(InMemoryRunStore::new()),
            "memory-finalize-race",
        )
        .await?;
        let directory = temp_dir();
        assert_atomic_finalization_preserves_late_event(
            Arc::new(JsonlRunStore::new(&directory)?),
            "jsonl-finalize-race",
        )
        .await?;
        std::fs::remove_dir_all(directory)?;
        Ok(())
    }

    #[tokio::test]
    async fn run_stores_reject_nonterminal_finalization_without_mutation() -> Result<()> {
        let directory = temp_dir();
        let stores: [Arc<dyn RunStore>; 2] = [
            Arc::new(InMemoryRunStore::new()),
            Arc::new(JsonlRunStore::new(&directory)?),
        ];
        for (index, store) in stores.into_iter().enumerate() {
            let run_id = format!("invalid-finalize-{index}");
            let mut run = make_run(&run_id, "session-invalid-finalize");
            run.status = RunStatus::Running;
            run.finished_at = None;
            run.final_output = None;
            store.save(run).await?;

            assert!(
                store
                    .finalize_run(&run_id, RunStatus::Running, Some("invalid"), None)
                    .await
                    .is_err()
            );
            let unchanged = store
                .load(&run_id)
                .await?
                .ok_or_else(|| ReactError::Other("run missing after rejected finalize".into()))?;
            assert_eq!(unchanged.status, RunStatus::Running);
            assert!(unchanged.finished_at.is_none());
            assert!(unchanged.final_output.is_none());
        }
        std::fs::remove_dir_all(directory)?;
        Ok(())
    }

    async fn wait_for_cached_run(
        store: &JsonlRunStore,
        run_id: &str,
        predicate: impl Fn(&Run) -> bool,
    ) -> Result<Run> {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if let Some(run) = store.load(run_id).await?
                    && predicate(&run)
                {
                    return Ok(run);
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .map_err(|_| ReactError::Other("timed out waiting for owned trace publish".to_string()))?
    }

    #[tokio::test]
    async fn jsonl_append_and_finalize_publish_after_caller_cancellation() -> Result<()> {
        let directory = temp_dir();
        let mut store = JsonlRunStore::new(&directory)?;
        let run_id = "cancelled-caller-owned-mutation";
        let mut run = make_run(run_id, "session-owned-mutation");
        run.status = RunStatus::Running;
        run.finished_at = None;
        run.final_output = None;
        store.save(run).await?;

        let append_hook = Arc::new(JsonlMutationTestHook::default());
        store.set_mutation_test_hook(Some(Arc::clone(&append_hook)));
        let append_committed = append_hook.physically_committed.notified();
        let append_caller = tokio::spawn({
            let store = store.clone();
            async move {
                store
                    .append_event(
                        run_id,
                        RunEvent::Checkpoint {
                            id: "after-physical-append".to_string(),
                        },
                    )
                    .await
            }
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), append_committed)
            .await
            .map_err(|_| ReactError::Other("append did not physically commit".to_string()))?;
        append_caller.abort();
        let append_join = append_caller.await;
        assert!(append_join.is_err());
        append_hook.release_publish.notify_one();
        let appended = wait_for_cached_run(&store, run_id, |run| {
            run.events.iter().any(|event| {
                matches!(event, RunEvent::Checkpoint { id } if id == "after-physical-append")
            })
        })
        .await?;
        assert_eq!(appended.status, RunStatus::Running);

        let finalize_hook = Arc::new(JsonlMutationTestHook::default());
        store.set_mutation_test_hook(Some(Arc::clone(&finalize_hook)));
        let finalize_committed = finalize_hook.physically_committed.notified();
        let finalize_caller = tokio::spawn({
            let store = store.clone();
            async move {
                store
                    .finalize_run(run_id, RunStatus::Failed, None, Some("caller disappeared"))
                    .await
            }
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), finalize_committed)
            .await
            .map_err(|_| ReactError::Other("finalize did not physically commit".to_string()))?;
        finalize_caller.abort();
        let finalize_join = finalize_caller.await;
        assert!(finalize_join.is_err());
        finalize_hook.release_publish.notify_one();
        let finalized =
            wait_for_cached_run(&store, run_id, |run| run.status == RunStatus::Failed).await?;
        store.set_mutation_test_hook(None);

        assert!(finalized.events.iter().any(|event| {
            matches!(event, RunEvent::Checkpoint { id } if id == "after-physical-append")
        }));
        assert_eq!(finalized.error.as_deref(), Some("caller disappeared"));
        let durable = JsonlRunStore::load_run(&store.run_path(run_id)?)?;
        assert_eq!(durable.status, RunStatus::Failed);
        assert_eq!(
            serde_json::to_value(&durable.events)?,
            serde_json::to_value(&finalized.events)?
        );
        std::fs::remove_dir_all(directory)?;
        Ok(())
    }

    #[tokio::test]
    async fn test_in_memory_store_save_and_load() {
        let store = InMemoryRunStore::new();
        let run = make_run("r1", "s1");
        store.save(run).await.unwrap();

        let loaded = store.load("r1").await.unwrap().unwrap();
        assert_eq!(loaded.run_id, "r1");
        assert_eq!(loaded.session_id, "s1");
    }

    #[tokio::test]
    async fn test_in_memory_store_list_by_session() {
        let store = InMemoryRunStore::new();
        store.save(make_run("r1", "s1")).await.unwrap();
        store.save(make_run("r2", "s1")).await.unwrap();
        store.save(make_run("r3", "s2")).await.unwrap();

        let s1_runs = store.list_by_session("s1").await.unwrap();
        assert_eq!(s1_runs.len(), 2);

        let s2_runs = store.list_by_session("s2").await.unwrap();
        assert_eq!(s2_runs.len(), 1);
    }

    #[test]
    fn test_token_usage_add() {
        let mut usage = TokenUsage::default();
        usage.add(100, 50);
        assert_eq!(usage.prompt_tokens, 100);
        assert_eq!(usage.completion_tokens, 50);
        assert_eq!(usage.total_tokens, 150);

        usage.add(30, 20);
        assert_eq!(usage.prompt_tokens, 130);
        assert_eq!(usage.completion_tokens, 70);
        assert_eq!(usage.total_tokens, 200);
    }

    #[test]
    fn token_usage_tracks_cache_and_missing_usage() {
        let mut usage = TokenUsage::default();
        usage.add_llm_call(1000, 50, 800, 20, true);
        usage.add_llm_call(0, 0, 0, 0, false);

        assert_eq!(usage.prompt_tokens, 1000);
        assert_eq!(usage.cached_prompt_tokens, 800);
        assert_eq!(usage.cache_creation_prompt_tokens, 20);
        assert_eq!(usage.usage_reported_calls, 1);
        assert_eq!(usage.usage_missing_calls, 1);
        assert_eq!(usage.cache_hit_rate(), Some(0.8));
    }

    #[tokio::test]
    async fn append_llm_event_updates_run_aggregates()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let store = InMemoryRunStore::new();
        store.save(make_run("r-usage", "s-usage")).await?;
        store
            .append_event(
                "r-usage",
                RunEvent::LlmCall {
                    messages: 4,
                    prompt_tokens: 1000,
                    completion_tokens: 80,
                    cached_prompt_tokens: 750,
                    cache_creation_prompt_tokens: 20,
                    usage_reported: true,
                    estimated_context_tokens: 980,
                    protected_context_tokens: 240,
                    protected_message_count: 3,
                    context_limit_tokens: 0,
                    context_breakdown: Default::default(),
                    cache_fingerprint: Default::default(),
                    duration_ms: 125,
                },
            )
            .await?;

        let run = store
            .load("r-usage")
            .await?
            .ok_or("run missing after append")?;
        assert_eq!(run.token_usage.prompt_tokens, 1000);
        assert_eq!(run.token_usage.cached_prompt_tokens, 750);
        assert_eq!(run.timings.llm_duration_ms, 125);
        Ok(())
    }

    #[test]
    fn legacy_llm_call_defaults_observability_metrics()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let parsed = serde_json::from_str::<RunEvent>(
            r#"{"type":"llm_call","messages":2,"prompt_tokens":10,"completion_tokens":3,"duration_ms":5}"#,
        )?;
        if let RunEvent::LlmCall {
            cached_prompt_tokens,
            cache_creation_prompt_tokens,
            usage_reported,
            estimated_context_tokens,
            protected_context_tokens,
            protected_message_count,
            ..
        } = parsed
        {
            assert_eq!(cached_prompt_tokens, 0);
            assert_eq!(cache_creation_prompt_tokens, 0);
            assert!(!usage_reported);
            assert_eq!(estimated_context_tokens, 0);
            assert_eq!(protected_context_tokens, 0);
            assert_eq!(protected_message_count, 0);
        } else {
            return Err("legacy payload did not parse as LlmCall".into());
        }
        Ok(())
    }

    #[test]
    fn legacy_tool_result_defaults_output_metrics()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let raw = r#"{
            "type":"tool_result",
            "call_id":"c1",
            "name":"read_file",
            "success":true,
            "output_preview":"ok",
            "output_truncated":false,
            "duration_ms":3
        }"#;
        let parsed = serde_json::from_str::<RunEvent>(raw)?;
        if let RunEvent::ToolResult {
            original_bytes,
            returned_bytes,
            estimated_tokens,
            output_handling,
            ..
        } = parsed
        {
            assert_eq!(original_bytes, 0);
            assert_eq!(returned_bytes, 0);
            assert_eq!(estimated_tokens, 0);
            assert_eq!(output_handling, None);
        } else {
            return Err("legacy payload did not parse as ToolResult".into());
        }
        Ok(())
    }

    #[tokio::test]
    async fn test_jsonl_store_save_and_load() {
        let dir = temp_dir();
        let store = JsonlRunStore::new(&dir).unwrap();
        let run = make_run("r1", "s1");
        store.save(run.clone()).await.unwrap();

        let loaded = store.load("r1").await.unwrap().unwrap();
        assert_eq!(loaded.run_id, "r1");
        assert_eq!(loaded.session_id, "s1");
    }

    #[tokio::test]
    async fn test_jsonl_store_list_by_session() {
        let dir = temp_dir();
        let store = JsonlRunStore::new(&dir).unwrap();
        store.save(make_run("r1", "s1")).await.unwrap();
        store.save(make_run("r2", "s1")).await.unwrap();
        store.save(make_run("r3", "s2")).await.unwrap();

        let s1 = store.list_by_session("s1").await.unwrap();
        assert_eq!(s1.len(), 2);
        let s2 = store.list_by_session("s2").await.unwrap();
        assert_eq!(s2.len(), 1);
    }

    #[tokio::test]
    async fn test_jsonl_store_list_all() {
        let dir = temp_dir();
        let store = JsonlRunStore::new(&dir).unwrap();
        store.save(make_run("r1", "s1")).await.unwrap();
        store.save(make_run("r2", "s2")).await.unwrap();
        store.save(make_run("r3", "s3")).await.unwrap();

        let all = store.list_all(2).await.unwrap();
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn test_jsonl_store_append_event() {
        let dir = temp_dir();
        let store = JsonlRunStore::new(&dir).unwrap();
        store.save(make_run("r1", "s1")).await.unwrap();

        store
            .append_event(
                "r1",
                RunEvent::ToolCall {
                    call_id: "c1".into(),
                    name: "read_file".into(),
                    args: None,
                    risk: None,
                    duration_ms: 100,
                },
            )
            .await
            .unwrap();

        let loaded = store.load("r1").await.unwrap().unwrap();
        assert_eq!(loaded.events.len(), 1);
        let lines = std::fs::read_to_string(dir.join("r1.jsonl"))
            .unwrap_or_default()
            .lines()
            .count();
        assert_eq!(lines, 2, "one snapshot plus one event line");
    }

    #[tokio::test]
    async fn jsonl_store_replays_events_and_ignores_only_truncated_tail() -> Result<()> {
        let dir = temp_dir();
        let store = JsonlRunStore::new(&dir)?;
        store.save(make_run("replay", "session")).await?;
        store
            .append_event("replay", RunEvent::Checkpoint { id: "one".into() })
            .await?;
        drop(store);
        use std::io::Write;
        std::fs::OpenOptions::new()
            .append(true)
            .open(dir.join("replay.jsonl"))?
            .write_all(b"{truncated")?;

        let reopened = JsonlRunStore::new(&dir)?;
        let run = reopened
            .load("replay")
            .await?
            .ok_or_else(|| crate::error::ReactError::Other("missing replayed run".into()))?;
        assert_eq!(run.events.len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn jsonl_store_isolates_complete_corrupt_event_tail() -> Result<()> {
        let dir = temp_dir();
        let run = make_run("complete-corrupt", "session");
        let mut data = serde_json::to_string(&run)?;
        data.push_str("\n{not-an-event}\n");
        std::fs::write(dir.join("complete-corrupt.jsonl"), data)?;

        let store = JsonlRunStore::new(&dir)?;
        assert!(store.shared.cache.read().await.is_empty());
        assert!(!dir.join("complete-corrupt.jsonl").exists());
        assert!(std::fs::read_dir(&dir)?.any(|entry| {
            entry
                .ok()
                .and_then(|entry| entry.file_name().to_str().map(str::to_string))
                .is_some_and(|name| name.starts_with("complete-corrupt.jsonl.corrupt-"))
        }));
        Ok(())
    }

    #[tokio::test]
    async fn jsonl_store_prunes_oldest_run_files_and_cache() -> Result<()> {
        let dir = temp_dir();
        let store = JsonlRunStore::new(&dir)?.with_max_runs(2);
        for id in ["r1", "r2", "r3"] {
            store.save(make_run(id, "session")).await?;
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        assert!(store.load("r1").await?.is_none());
        assert!(store.load("r2").await?.is_some());
        assert!(store.load("r3").await?.is_some());
        assert!(!dir.join("r1.jsonl").exists());
        Ok(())
    }

    #[tokio::test]
    async fn test_jsonl_store_persistence_across_instances() {
        let dir = temp_dir();
        let store = JsonlRunStore::new(&dir).unwrap();
        store.save(make_run("r1", "s1")).await.unwrap();
        drop(store);

        // New instance should load from disk
        let store2 = JsonlRunStore::new(&dir).unwrap();
        let loaded = store2.load("r1").await.unwrap();
        assert!(loaded.is_some());
    }

    #[tokio::test]
    async fn jsonl_store_rejects_run_id_path_escape() -> Result<()> {
        let dir = temp_dir();
        let store = JsonlRunStore::new(&dir)?;
        for id in ["../outside", "/tmp/outside", "a/b", "a\\b"] {
            assert!(store.save(make_run(id, "s1")).await.is_err());
            assert!(store.load(id).await.is_err());
        }
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn jsonl_store_rejects_symlink_run_file() -> Result<()> {
        use std::os::unix::fs::symlink;

        let dir = temp_dir();
        let outside = dir.with_extension("outside.jsonl");
        let outside_bytes = serde_json::to_vec(&make_run("linked", "s1"))?;
        std::fs::write(&outside, &outside_bytes)?;
        symlink(&outside, dir.join("linked.jsonl"))?;

        assert!(JsonlRunStore::new(&dir).is_err());
        assert_eq!(std::fs::read(&outside)?, outside_bytes);

        std::fs::remove_file(dir.join("linked.jsonl"))?;
        std::fs::remove_file(outside)?;
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn jsonl_store_append_rejects_runtime_symlink_swap() -> Result<()> {
        use std::os::unix::fs::symlink;

        let dir = temp_dir();
        let store = JsonlRunStore::new(&dir)?;
        store.save(make_run("linked", "s1")).await?;
        let run_path = dir.join("linked.jsonl");
        let original = dir.join("linked.original.jsonl");
        let outside = dir.with_extension("outside-runtime.jsonl");
        std::fs::rename(&run_path, &original)?;
        std::fs::write(&outside, b"outside\n")?;
        symlink(&outside, &run_path)?;

        assert!(
            store
                .append_event("linked", RunEvent::Checkpoint { id: "one".into() })
                .await
                .is_err()
        );
        assert_eq!(std::fs::read(&outside)?, b"outside\n");

        std::fs::remove_file(run_path)?;
        std::fs::remove_file(original)?;
        std::fs::remove_file(outside)?;
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[tokio::test]
    async fn jsonl_store_redacts_and_bounds_all_persisted_content() -> Result<()> {
        let dir = temp_dir();
        let store = JsonlRunStore::new(&dir)?.with_retention_policy(
            echo_core::utils::retention::ContentRetentionPolicy {
                max_string_chars: 64,
                max_array_items: 10,
            },
        );
        let mut run = make_run("redacted", "session");
        run.input = "Bearer abcdefghijklmnopqrstuvwxyz".to_string();
        run.final_output = Some("中文字符".repeat(40));
        run.error = Some("token: secretvalue123456".to_string());
        run.events.push(RunEvent::ToolCall {
            call_id: "call".to_string(),
            name: "shell".to_string(),
            args: Some(serde_json::json!({
                "nested": {"key": "sk-abcdefghijklmnopqrstuvwxyz"}
            })),
            risk: None,
            duration_ms: 1,
        });
        store.save(run).await?;

        let bytes = tokio::fs::read_to_string(dir.join("redacted.jsonl")).await?;
        assert!(!bytes.contains("abcdefghijklmnopqrstuvwxyz"));
        assert!(!bytes.contains("secretvalue123456"));
        assert!(bytes.contains("[REDACTED]"));
        assert!(bytes.contains("[TRUNCATED]"));
        let loaded = store
            .load("redacted")
            .await?
            .ok_or_else(|| crate::error::ReactError::Other("missing redacted run".to_string()))?;
        assert!(!loaded.input.contains("abcdefghijklmnopqrstuvwxyz"));
        Ok(())
    }

    #[tokio::test]
    async fn in_memory_and_jsonl_stores_apply_matching_retention() -> Result<()> {
        let secret_input = "password: hunter2-verylongvalue";
        let private_key_body = "MIIEvQIBADANBgkqhkiG9w0B";
        let mut run = make_run("parity", "session");
        run.input = format!(
            "{secret_input}\n-----BEGIN PRIVATE KEY-----\n{private_key_body}\n-----END PRIVATE KEY-----"
        );
        run.events.push(RunEvent::ToolCall {
            call_id: "parity-call".into(),
            name: "shell".into(),
            args: Some(serde_json::json!({"nested": {"token": "shhh-123456"}})),
            risk: Some("low".into()),
            duration_ms: 7,
        });

        let memory = InMemoryRunStore::new();
        memory.save(run.clone()).await?;
        let memory_loaded = memory
            .load("parity")
            .await?
            .ok_or_else(|| crate::error::ReactError::Other("missing memory run".to_string()))?;

        let dir = temp_dir();
        let jsonl = JsonlRunStore::new(&dir)?;
        jsonl.save(run).await?;
        let jsonl_loaded = jsonl
            .load("parity")
            .await?
            .ok_or_else(|| crate::error::ReactError::Other("missing jsonl run".to_string()))?;

        assert_eq!(memory_loaded.input, jsonl_loaded.input);
        assert!(!memory_loaded.input.contains("hunter2"));
        assert!(!memory_loaded.input.contains(private_key_body));
        assert_eq!(
            serde_json::to_string(&memory_loaded.events)?,
            serde_json::to_string(&jsonl_loaded.events)?
        );
        let disk = tokio::fs::read_to_string(dir.join("parity.jsonl")).await?;
        assert!(!disk.contains("hunter2"));
        assert!(!disk.contains(private_key_body));
        assert!(!disk.contains("shhh-123456"));
        Ok(())
    }

    #[tokio::test]
    async fn secret_like_identities_remain_addressable_after_retention() -> Result<()> {
        let run_id = "sk-abcdefghijklmnopqrstuvwxyz123456";
        let session_id = "npm_abcdefghijklmnopqrstuvwxyz1234567890";
        let mut run = make_run(run_id, session_id);
        run.events.push(RunEvent::Checkpoint {
            id: "ghp_abcdefghijklmnopqrstuvwxyz1234567890".to_string(),
        });
        let store = InMemoryRunStore::new();
        store.save(run).await?;

        let loaded = store
            .load(run_id)
            .await?
            .ok_or_else(|| ReactError::Other("identity-addressed run missing".to_string()))?;
        assert_eq!(loaded.run_id, run_id);
        assert_eq!(loaded.session_id, session_id);
        assert!(matches!(
            loaded.events.first(),
            Some(RunEvent::Checkpoint { id }) if id == "ghp_abcdefghijklmnopqrstuvwxyz1234567890"
        ));
        assert_eq!(store.list_by_session(session_id).await?.len(), 1);
        store
            .append_event(
                run_id,
                RunEvent::Checkpoint {
                    id: "second".to_string(),
                },
            )
            .await?;
        assert_eq!(
            store.load(run_id).await?.map(|run| run.events.len()),
            Some(2)
        );
        Ok(())
    }

    #[tokio::test]
    async fn retention_never_drops_records_or_token_counters() -> Result<()> {
        let policy = echo_core::utils::retention::ContentRetentionPolicy {
            max_string_chars: 8,
            max_array_items: 1,
        };
        let mut run = make_run("bounded", "session");
        run.push_event(RunEvent::LlmCall {
            messages: 3,
            prompt_tokens: 111,
            completion_tokens: 22,
            cached_prompt_tokens: 5,
            cache_creation_prompt_tokens: 6,
            usage_reported: true,
            estimated_context_tokens: 0,
            protected_context_tokens: 0,
            protected_message_count: 0,
            context_limit_tokens: 0,
            context_breakdown: LlmContextBreakdown::default(),
            cache_fingerprint: echo_core::llm::cache::PromptCacheFingerprint::default(),
            duration_ms: 9,
        });
        run.events.push(RunEvent::ToolResult {
            call_id: "result-call".into(),
            name: "shell".into(),
            success: true,
            output_preview: Some("Bearer abcdefghijklmnopqrstuvwxyz".into()),
            output_truncated: true,
            duration_ms: 4,
            original_bytes: 34,
            returned_bytes: 8,
            estimated_tokens: 10,
            output_handling: Some("truncated".into()),
            artifact: None,
        });
        apply_run_retention(&mut run, &policy);

        assert_eq!(run.events.len(), 2);
        assert_eq!(run.token_usage.prompt_tokens, 111);
        assert_eq!(run.token_usage.completion_tokens, 22);
        assert_eq!(run.token_usage.total_tokens, 133);
        assert_eq!(run.token_usage.usage_reported_calls, 1);
        let bounded_chars = |text: &str| text.chars().count() <= 8 + "...[TRUNCATED]".len();
        assert!(bounded_chars(&run.input));
        for event in &run.events {
            match event {
                RunEvent::LlmCall { prompt_tokens, .. } => assert_eq!(*prompt_tokens, 111),
                RunEvent::ToolResult {
                    output_preview,
                    call_id,
                    ..
                } => {
                    assert_eq!(call_id, "result-call");
                    assert!(output_preview.as_deref().is_some_and(&bounded_chars));
                }
                other => {
                    return Err(ReactError::Other(format!("unexpected event: {other:?}")));
                }
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn retention_zero_limits_keep_all_run_records_and_counters() -> Result<()> {
        let policy = ContentRetentionPolicy {
            max_string_chars: 0,
            max_array_items: 0,
        };
        let mut run = make_run("zero", "session");
        run.push_event(RunEvent::LlmCall {
            messages: 2,
            prompt_tokens: 50,
            completion_tokens: 10,
            cached_prompt_tokens: 4,
            cache_creation_prompt_tokens: 2,
            usage_reported: true,
            estimated_context_tokens: 0,
            protected_context_tokens: 0,
            protected_message_count: 0,
            context_limit_tokens: 0,
            context_breakdown: LlmContextBreakdown::default(),
            cache_fingerprint: echo_core::llm::cache::PromptCacheFingerprint::default(),
            duration_ms: 3,
        });
        run.push_event(RunEvent::ToolCall {
            call_id: "zero-call".into(),
            name: "shell".into(),
            args: Some(serde_json::json!({"nested": [{"secret": "tok-abc12345"}]})),
            risk: None,
            duration_ms: 1,
        });
        apply_run_retention(&mut run, &policy);

        assert_eq!(run.events.len(), 2);
        assert_eq!(run.token_usage.prompt_tokens, 50);
        assert_eq!(run.token_usage.completion_tokens, 10);
        assert_eq!(run.token_usage.total_tokens, 60);
        assert_eq!(run.token_usage.usage_reported_calls, 1);
        assert_eq!(run.run_id, "zero");
        for event in &run.events {
            match event {
                RunEvent::LlmCall {
                    prompt_tokens,
                    completion_tokens,
                    cache_fingerprint,
                    ..
                } => {
                    assert_eq!(*prompt_tokens, 50);
                    assert_eq!(*completion_tokens, 10);
                    assert_eq!(cache_fingerprint.stable_prefix_hash, "");
                }
                RunEvent::ToolCall {
                    call_id,
                    args,
                    duration_ms,
                    ..
                } => {
                    assert_eq!(call_id, "zero-call");
                    assert_eq!(*duration_ms, 1);
                    let encoded = serde_json::to_string(
                        args.as_ref()
                            .ok_or_else(|| ReactError::Other("args dropped".into()))?,
                    )?;
                    assert!(!encoded.contains("tok-abc12345"));
                    assert!(encoded.contains("[TRUNCATED"));
                }
                other => {
                    return Err(ReactError::Other(format!("unexpected event: {other:?}")));
                }
            }
        }
        Ok(())
    }

    #[test]
    fn retention_handles_multibyte_text_without_panicking() -> Result<()> {
        let mut run = make_run("utf8", "session");
        run.input = "天空 emoji 🌍🌍🌍 世界".to_string();
        run.final_output = Some("🌍".repeat(300));
        run.events.push(RunEvent::SubagentRun {
            call_id: None,
            agent_name: "subagent-name".into(),
            task: "翻译 🌍 内容".into(),
            outcome: "completed".into(),
        });
        let policy = ContentRetentionPolicy {
            max_string_chars: 10,
            max_array_items: 4,
        };
        apply_run_retention(&mut run, &policy);
        assert!(run.input.chars().count() <= 10 + "...[TRUNCATED]".len());
        assert!(run.input.contains("🌍") || run.input.contains("世界"));
        Ok(())
    }

    #[test]
    fn old_subagent_trace_event_without_call_id_still_decodes() -> Result<()> {
        let event: RunEvent = serde_json::from_value(serde_json::json!({
            "type": "subagent_run",
            "agent_name": "reviewer",
            "task": "review",
            "outcome": "completed"
        }))?;
        match event {
            RunEvent::SubagentRun { call_id, .. } => assert!(call_id.is_none()),
            other => return Err(ReactError::Other(format!("unexpected event: {other:?}"))),
        }
        Ok(())
    }

    #[test]
    fn old_test_run_without_exact_count_decodes_as_unknown() -> Result<()> {
        let event: RunEvent = serde_json::from_value(serde_json::json!({
            "type": "test_run",
            "command": "cargo test",
            "passed": false
        }))?;
        assert!(matches!(
            event,
            RunEvent::TestRun {
                failure_count: None,
                ..
            }
        ));
        Ok(())
    }

    #[tokio::test]
    async fn custom_run_store_caller_receives_sanitized_input() -> Result<()> {
        let mut agent = crate::agent::ReactAgent::new(crate::agent::AgentConfig::new(
            "model", "agent", "system",
        ));
        agent.run_store = Some(Arc::new(InMemoryRunStore::new()));
        let legacy = agent.capture_legacy_external_context();
        let run_id = agent
            .start_legacy_trace_run("password: raw-secret-input-value", &legacy)
            .await
            .ok_or_else(|| ReactError::Other("trace start did not return an id".to_string()))?;
        let stored = agent
            .run_store
            .as_ref()
            .ok_or_else(|| ReactError::Other("missing run store".to_string()))?
            .load(&run_id)
            .await?
            .ok_or_else(|| ReactError::Other("missing traced run".to_string()))?;
        assert!(!stored.input.contains("raw-secret-input-value"));
        assert!(stored.input.contains("[REDACTED]"));
        Ok(())
    }

    #[tokio::test]
    async fn jsonl_store_serializes_instances_and_preserves_terminal_state() -> Result<()> {
        let dir = temp_dir();
        let first = Arc::new(JsonlRunStore::new(&dir)?);
        let second = Arc::new(JsonlRunStore::new(&dir)?);
        let mut running = make_run("shared", "session");
        running.status = RunStatus::Running;
        running.finished_at = None;
        first.save(running.clone()).await?;

        let left = Arc::clone(&first);
        let right = Arc::clone(&second);
        let append_left = tokio::spawn(async move {
            left.append_event(
                "shared",
                RunEvent::ToolCall {
                    call_id: "left".into(),
                    name: "read_file".into(),
                    args: None,
                    risk: None,
                    duration_ms: 1,
                },
            )
            .await
        });
        let append_right = tokio::spawn(async move {
            right
                .append_event(
                    "shared",
                    RunEvent::ToolCall {
                        call_id: "right".into(),
                        name: "read_file".into(),
                        args: None,
                        risk: None,
                        duration_ms: 1,
                    },
                )
                .await
        });
        append_left
            .await
            .map_err(|error| crate::error::ReactError::Other(error.to_string()))??;
        append_right
            .await
            .map_err(|error| crate::error::ReactError::Other(error.to_string()))??;

        let mut completed = running.clone();
        completed.status = RunStatus::Completed;
        completed.final_output = Some("done".into());
        completed.finished_at = Some(Utc::now());
        first.save(completed).await?;
        second.save(running).await?;

        let loaded = first
            .load("shared")
            .await?
            .ok_or_else(|| crate::error::ReactError::Other("missing run".into()))?;
        assert_eq!(loaded.events.len(), 2);
        assert_eq!(loaded.status, RunStatus::Completed);
        assert_eq!(loaded.final_output.as_deref(), Some("done"));
        Ok(())
    }

    #[tokio::test]
    async fn jsonl_store_isolates_corrupt_existing_run() -> Result<()> {
        let dir = temp_dir();
        std::fs::write(dir.join("broken.jsonl"), b"{not-json}\n")?;
        let store = JsonlRunStore::new(&dir)?;
        assert!(store.shared.cache.read().await.is_empty());
        assert!(!dir.join("broken.jsonl").exists());
        Ok(())
    }
}
