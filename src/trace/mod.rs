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
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;
use tokio::sync::RwLock;

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

    /// Parent run ID for sub-agent invocations.
    pub parent_run_id: Option<String>,

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolOutputArtifactTrace {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
    pub retention: String,
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
        /// Elapsed milliseconds for this LLM call.
        duration_ms: u64,
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
        artifact: Option<ToolOutputArtifactTrace>,
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
    #[allow(dead_code)]
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
    /// A tool permission decision was made.
    PermissionDecision {
        /// Tool name.
        tool: String,
        /// Decision: "allow", "deny", "ask".
        decision: String,
        /// Reason for the decision.
        reason: String,
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
        /// Whether all tests passed.
        passed: bool,
        /// Number of failing tests.
        failure_count: usize,
    },
    /// Agent turn phase transition.
    PhaseTransition {
        /// Phase name (e.g., "receive_input", "think", "act").
        phase: String,
        /// Iteration count at transition.
        iteration: usize,
    },
    /// A sub-agent was dispatched.
    SubAgentRun {
        /// Sub-agent name.
        agent_name: String,
        /// Task given to the sub-agent.
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
        let safe_args = args.map(|v| {
            let s = serde_json::to_string(&v).unwrap_or_default();
            let redacted = crate::security::redact_secrets(&s);
            serde_json::from_str(&redacted).unwrap_or(v)
        });
        Self::ToolCall {
            call_id,
            name,
            args: safe_args,
            risk,
            duration_ms,
        }
    }
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
    pub session_id: String,
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

    /// Append a single event to an existing run (without rewriting the entire run).
    ///
    /// The default implementation loads, modifies, and saves. Implementations
    /// that support efficient append (e.g. JSONL) should override this.
    async fn append_event(&self, run_id: &str, event: RunEvent) -> Result<()> {
        if let Some(mut run) = self.load(run_id).await? {
            run.push_event(event);
            self.save(run).await?;
        }
        Ok(())
    }
}

// ── InMemoryRunStore ─────────────────────────────────────────────────

/// In-memory [`RunStore`] implementation backed by a `HashMap`.
///
/// Suitable for testing and short-lived sessions. Runs are not persisted
/// across restarts.
pub struct InMemoryRunStore {
    runs: RwLock<HashMap<String, Run>>,
}

impl InMemoryRunStore {
    /// Create a new empty store.
    pub fn new() -> Self {
        Self {
            runs: RwLock::new(HashMap::new()),
        }
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
        self.runs.write().await.insert(run.run_id.clone(), run);
        Ok(())
    }

    async fn load(&self, run_id: &str) -> Result<Option<Run>> {
        Ok(self.runs.read().await.get(run_id).cloned())
    }

    async fn list_by_session(&self, session_id: &str) -> Result<Vec<RunSummary>> {
        let runs = self.runs.read().await;
        let mut summaries: Vec<RunSummary> = runs
            .values()
            .filter(|r| r.session_id == session_id)
            .map(|r| RunSummary {
                run_id: r.run_id.clone(),
                session_id: r.session_id.clone(),
                status: r.status,
                input_preview: r.input.chars().take(80).collect(),
                started_at: r.started_at,
                finished_at: r.finished_at,
                token_usage: r.token_usage,
                total_duration_ms: r.timings.total_duration_ms,
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
            .map(|r| RunSummary {
                run_id: r.run_id.clone(),
                session_id: r.session_id.clone(),
                status: r.status,
                input_preview: r.input.chars().take(80).collect(),
                started_at: r.started_at,
                finished_at: r.finished_at,
                token_usage: r.token_usage,
                total_duration_ms: r.timings.total_duration_ms,
            })
            .collect();
        summaries.sort_by_key(|s| s.started_at);
        summaries.reverse();
        summaries.truncate(limit);
        Ok(summaries)
    }
}

// ── JsonlRunStore ────────────────────────────────────────────────────

/// File-based [`RunStore`] that persists each run as a JSONL file.
///
/// Each run is stored in `{dir}/{run_id}.jsonl`. Every call to [`save`]
/// appends a complete JSON line, so the latest line always represents the
/// current run state. An in-memory cache avoids re-reading files on every
/// query.
///
/// Suitable for production use with persistent storage across restarts.
pub struct JsonlRunStore {
    dir: PathBuf,
    /// In-memory cache: run_id → Run (newest state)
    cache: RwLock<HashMap<String, Run>>,
}

impl JsonlRunStore {
    /// Create a new store rooted at `dir`. The directory is created if it
    /// does not exist. Existing `.jsonl` files are scanned to populate the
    /// in-memory cache (only the last line of each file is loaded).
    pub fn new(dir: impl Into<PathBuf>) -> Result<Self> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        let mut cache = HashMap::new();

        // Populate cache from existing files
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|ext| ext == "jsonl")
                    && let Some(run) = Self::load_last_line(&path)
                {
                    cache.insert(run.run_id.clone(), run);
                }
            }
        }

        Ok(Self {
            dir,
            cache: RwLock::new(cache),
        })
    }

    /// Return the file path for a given run ID.
    fn run_path(&self, run_id: &str) -> PathBuf {
        self.dir.join(format!("{run_id}.jsonl"))
    }

    /// Read only the **last** line of a JSONL file and deserialize it as a `Run`.
    fn load_last_line(path: &Path) -> Option<Run> {
        let data = std::fs::read_to_string(path).ok()?;
        // Find the last non-empty line
        let last_line = data.lines().rfind(|l| !l.trim().is_empty())?;
        serde_json::from_str::<Run>(last_line).ok()
    }

    /// Async version of `load_last_line` for use in async contexts.
    async fn load_last_line_async(path: &Path) -> Option<Run> {
        let data = tokio::fs::read_to_string(path).await.ok()?;
        let last_line = data.lines().rfind(|l| !l.trim().is_empty())?;
        serde_json::from_str::<Run>(last_line).ok()
    }
}

#[async_trait::async_trait]
impl RunStore for JsonlRunStore {
    async fn save(&self, run: Run) -> Result<()> {
        let run_id = run.run_id.clone();
        let path = self.run_path(&run_id);
        let line = serde_json::to_string(&run)?;

        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .write(true)
            .open(&path)
            .await?;
        file.write_all(line.as_bytes()).await?;
        file.write_all(b"\n").await?;

        // Update in-memory cache
        self.cache.write().await.insert(run_id, run);
        Ok(())
    }

    async fn load(&self, run_id: &str) -> Result<Option<Run>> {
        // Check cache first
        if let Some(run) = self.cache.read().await.get(run_id) {
            return Ok(Some(run.clone()));
        }
        // Fall back to disk (async)
        let path = self.run_path(run_id);
        if tokio::fs::try_exists(&path).await.unwrap_or(false)
            && let Some(run) = Self::load_last_line_async(&path).await
        {
            self.cache
                .write()
                .await
                .insert(run_id.to_string(), run.clone());
            return Ok(Some(run));
        }
        Ok(None)
    }

    async fn list_by_session(&self, session_id: &str) -> Result<Vec<RunSummary>> {
        let cache = self.cache.read().await;
        let mut summaries: Vec<RunSummary> = cache
            .values()
            .filter(|r| r.session_id == session_id)
            .map(|r| RunSummary {
                run_id: r.run_id.clone(),
                session_id: r.session_id.clone(),
                status: r.status,
                input_preview: r.input.chars().take(80).collect(),
                started_at: r.started_at,
                finished_at: r.finished_at,
                token_usage: r.token_usage,
                total_duration_ms: r.timings.total_duration_ms,
            })
            .collect();
        summaries.sort_by_key(|s| s.started_at);
        summaries.reverse();
        Ok(summaries)
    }

    async fn list_all(&self, limit: usize) -> Result<Vec<RunSummary>> {
        let cache = self.cache.read().await;
        let mut summaries: Vec<RunSummary> = cache
            .values()
            .map(|r| RunSummary {
                run_id: r.run_id.clone(),
                session_id: r.session_id.clone(),
                status: r.status,
                input_preview: r.input.chars().take(80).collect(),
                started_at: r.started_at,
                finished_at: r.finished_at,
                token_usage: r.token_usage,
                total_duration_ms: r.timings.total_duration_ms,
            })
            .collect();
        summaries.sort_by_key(|s| s.started_at);
        summaries.reverse();
        summaries.truncate(limit);
        Ok(summaries)
    }

    /// Append a single event by writing an updated line to the JSONL file.
    async fn append_event(&self, run_id: &str, event: RunEvent) -> Result<()> {
        // Load current state, append event, save back
        let mut run = match self.load(run_id).await? {
            Some(run) => run,
            None => return Ok(()),
        };
        run.push_event(event);
        self.save(run).await
    }
}

// ── Unit tests ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_run(id: &str, session: &str) -> Run {
        Run {
            run_id: id.to_string(),
            parent_run_id: None,
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
}
