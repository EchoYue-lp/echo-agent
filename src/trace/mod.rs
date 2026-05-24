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

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

pub use crate::error::Result;

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
    pub started_at: DateTime<Utc>,

    /// When the run finished (set on completion, failure, or cancellation).
    pub finished_at: Option<DateTime<Utc>>,
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

// ── RunEvent ─────────────────────────────────────────────────────────

/// A discrete event within a run's execution timeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunEvent {
    /// An LLM call was made.
    LlmCall {
        /// Number of messages in the request.
        messages: usize,
        /// Prompt tokens consumed.
        prompt_tokens: u32,
        /// Completion tokens received.
        completion_tokens: u32,
        /// Elapsed milliseconds for this LLM call.
        duration_ms: u64,
    },
    /// A tool was called.
    ToolCall {
        /// Tool name.
        name: String,
        /// Duration of the tool execution in milliseconds.
        duration_ms: u64,
    },
    /// A tool returned a result.
    ToolResult {
        /// Tool name.
        name: String,
        /// Whether the output was truncated.
        output_truncated: bool,
    },
    /// A tool returned an error.
    ToolError {
        /// Tool name.
        name: String,
        /// Error message.
        message: String,
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
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub token_usage: TokenUsage,
    pub total_duration_ms: u64,
}

// ── RunStore trait ───────────────────────────────────────────────────

/// Persistence backend for execution traces.
///
/// Built-in implementations:
/// - [`InMemoryRunStore`] — in-memory (testing, short-lived sessions)
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
}
