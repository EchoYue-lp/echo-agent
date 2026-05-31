//! TraceAnalyzer — observability over stored runs.
//!
//! Provides session-level summarization, tool usage statistics, token
//! breakdowns, slow-tool detection, and error-pattern analysis. All methods
//! operate on a shared [`RunStore`] backend, making them independent of the
//! storage implementation (in-memory, JSONL, etc.).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use crate::trace::{RunEvent, RunStatus, RunStore, TokenUsage};

// ── Analysis result structs ────────────────────────────────────────────

/// Summary for a single session (aggregation of all runs in that session).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    /// Session identifier.
    pub session_id: String,
    /// Number of runs in the session.
    pub run_count: usize,
    /// Number of completed runs.
    pub completed_count: usize,
    /// Number of failed runs.
    pub failed_count: usize,
    /// Number of cancelled runs.
    pub cancelled_count: usize,
    /// Total prompt tokens across all runs.
    pub total_prompt_tokens: u32,
    /// Total completion tokens across all runs.
    pub total_completion_tokens: u32,
    /// Total tokens across all runs.
    pub total_tokens: u32,
    /// Cumulative wall-clock duration (ms) across all runs.
    pub total_duration_ms: u64,
    /// Cumulative LLM call duration (ms).
    pub total_llm_duration_ms: u64,
    /// Cumulative tool execution duration (ms).
    pub total_tool_duration_ms: u64,
    /// Earliest `started_at` in the session.
    pub first_started_at: Option<DateTime<Utc>>,
    /// Latest `finished_at` in the session.
    pub last_finished_at: Option<DateTime<Utc>>,
    /// Tool names used across all runs in this session.
    pub tools_used: Vec<String>,
    /// Number of LLM calls across all runs.
    pub llm_call_count: usize,
}

/// Per-tool usage statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolUsageStats {
    /// Tool name.
    pub name: String,
    /// Number of times the tool was called.
    pub call_count: usize,
    /// Number of successful calls.
    pub success_count: usize,
    /// Number of failed calls.
    pub failure_count: usize,
    /// Average duration per call (ms).
    pub avg_duration_ms: u64,
    /// Minimum duration (ms).
    pub min_duration_ms: u64,
    /// Maximum duration (ms).
    pub max_duration_ms: u64,
    /// Total duration (ms).
    pub total_duration_ms: u64,
    /// Percentage of total tool time (relative to all tools).
    pub pct_of_total_time: f64,
}

/// Token usage breakdown across runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenBreakdown {
    /// Total prompt tokens.
    pub prompt_tokens: u32,
    /// Total completion tokens.
    pub completion_tokens: u32,
    /// Total tokens (prompt + completion).
    pub total_tokens: u32,
    /// Per-run breakdown (run_id -> TokenUsage).
    pub per_run: HashMap<String, TokenUsage>,
    /// Per-LLM-call breakdown (run_id -> list of (prompt, completion)).
    pub per_llm_call: HashMap<String, Vec<(u32, u32)>>,
}

/// A recurring error pattern identified across runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorPattern {
    /// Normalized error message (grouped by similarity).
    pub pattern: String,
    /// Number of occurrences.
    pub occurrence_count: usize,
    /// Run IDs where this pattern was seen.
    pub run_ids: Vec<String>,
    /// Tool names most frequently associated with this error.
    pub associated_tools: Vec<String>,
    /// First occurrence timestamp.
    pub first_seen: Option<DateTime<Utc>>,
    /// Last occurrence timestamp.
    pub last_seen: Option<DateTime<Utc>>,
}

// ── TraceAnalyzer ──────────────────────────────────────────────────────

/// Observability analyzer for execution traces.
///
/// Wraps a [`RunStore`] and provides analytics methods that aggregate data
/// across runs, sessions, and events. All methods are async to accommodate
/// potentially slow storage backends (e.g. disk I/O for JSONL stores).
pub struct TraceAnalyzer {
    run_store: Arc<dyn RunStore>,
}

impl TraceAnalyzer {
    /// Create a new analyzer backed by the given [`RunStore`].
    pub fn new(run_store: Arc<dyn RunStore>) -> Self {
        Self { run_store }
    }

    /// Produce a summary for a single session by aggregating all its runs.
    pub async fn summarize_session(&self, session_id: &str) -> crate::error::Result<SessionSummary> {
        let summaries = self.run_store.list_by_session(session_id).await?;

        let mut completed_count = 0;
        let mut failed_count = 0;
        let mut cancelled_count = 0;
        let mut total_prompt_tokens = 0u32;
        let mut total_completion_tokens = 0u32;
        let mut total_tokens = 0u32;
        let mut total_duration_ms = 0u64;
        let mut total_llm_duration_ms = 0u64;
        let mut total_tool_duration_ms = 0u64;
        let mut first_started_at: Option<DateTime<Utc>> = None;
        let mut last_finished_at: Option<DateTime<Utc>> = None;
        let mut tools_used_set: HashMap<String, ()> = HashMap::new();
        let mut llm_call_count = 0usize;

        for summary in &summaries {
            match summary.status {
                RunStatus::Completed => completed_count += 1,
                RunStatus::Failed => failed_count += 1,
                RunStatus::Cancelled => cancelled_count += 1,
                _ => {}
            }
            total_prompt_tokens = total_prompt_tokens.saturating_add(summary.token_usage.prompt_tokens);
            total_completion_tokens = total_completion_tokens.saturating_add(summary.token_usage.completion_tokens);
            total_tokens = total_tokens.saturating_add(summary.token_usage.total_tokens);
            total_duration_ms += summary.total_duration_ms;

            if first_started_at.is_none() || summary.started_at < first_started_at.unwrap() {
                first_started_at = Some(summary.started_at);
            }
            if let Some(fa) = summary.finished_at {
                if last_finished_at.is_none() || fa > last_finished_at.unwrap() {
                    last_finished_at = Some(fa);
                }
            }

            // Load full run to extract tool names and LLM call count
            if let Some(run) = self.run_store.load(&summary.run_id).await? {
                total_llm_duration_ms += run.timings.llm_duration_ms;
                total_tool_duration_ms += run.timings.tool_duration_ms;
                for event in &run.events {
                    match event {
                        RunEvent::ToolCall { name, .. } => {
                            tools_used_set.insert(name.clone(), ());
                        }
                        RunEvent::LlmCall { .. } => {
                            llm_call_count += 1;
                        }
                        _ => {}
                    }
                }
            }
        }

        Ok(SessionSummary {
            session_id: session_id.to_string(),
            run_count: summaries.len(),
            completed_count,
            failed_count,
            cancelled_count,
            total_prompt_tokens,
            total_completion_tokens,
            total_tokens,
            total_duration_ms,
            total_llm_duration_ms,
            total_tool_duration_ms,
            first_started_at,
            last_finished_at,
            tools_used: tools_used_set.keys().cloned().collect(),
            llm_call_count,
        })
    }

    /// Compute per-tool usage statistics across all runs (up to `limit`).
    pub async fn tool_usage_stats(&self, limit: usize) -> crate::error::Result<Vec<ToolUsageStats>> {
        let summaries = self.run_store.list_all(limit).await?;

        // Accumulate per-tool data
        let mut tool_data: HashMap<String, ToolAccumulator> = HashMap::new();

        for summary in &summaries {
            if let Some(run) = self.run_store.load(&summary.run_id).await? {
                for event in &run.events {
                    match event {
                        RunEvent::ToolCall { name, duration_ms, .. } => {
                            let acc = tool_data.entry(name.clone()).or_default();
                            acc.call_count += 1;
                            acc.total_duration_ms += *duration_ms;
                            if *duration_ms < acc.min_duration_ms {
                                acc.min_duration_ms = *duration_ms;
                            }
                            if *duration_ms > acc.max_duration_ms {
                                acc.max_duration_ms = *duration_ms;
                            }
                        }
                        RunEvent::ToolResult { name, success, .. } => {
                            if let Some(acc) = tool_data.get_mut(name) {
                                if *success {
                                    acc.success_count += 1;
                                } else {
                                    acc.failure_count += 1;
                                }
                            }
                        }
                        RunEvent::ToolError { name, .. } => {
                            if let Some(acc) = tool_data.get_mut(name) {
                                acc.failure_count += 1;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        // Compute total tool time for percentage calculation
        let grand_total_ms: u64 = tool_data.values().map(|a| a.total_duration_ms).sum();

        let mut stats: Vec<ToolUsageStats> = tool_data
            .into_iter()
            .map(|(name, acc)| {
                let avg = if acc.call_count > 0 {
                    acc.total_duration_ms / acc.call_count as u64
                } else {
                    0
                };
                let pct = if grand_total_ms > 0 {
                    (acc.total_duration_ms as f64 / grand_total_ms as f64) * 100.0
                } else {
                    0.0
                };
                ToolUsageStats {
                    name,
                    call_count: acc.call_count,
                    success_count: acc.success_count,
                    failure_count: acc.failure_count,
                    avg_duration_ms: avg,
                    min_duration_ms: acc.min_duration_ms,
                    max_duration_ms: acc.max_duration_ms,
                    total_duration_ms: acc.total_duration_ms,
                    pct_of_total_time: pct,
                }
            })
            .collect();

        // Sort by total_duration descending
        stats.sort_by(|a, b| b.total_duration_ms.cmp(&a.total_duration_ms));
        Ok(stats)
    }

    /// Compute token usage breakdown across all runs (up to `limit`).
    pub async fn token_usage_breakdown(&self, limit: usize) -> crate::error::Result<TokenBreakdown> {
        let summaries = self.run_store.list_all(limit).await?;

        let mut prompt_tokens = 0u32;
        let mut completion_tokens = 0u32;
        let mut total_tokens = 0u32;
        let mut per_run: HashMap<String, TokenUsage> = HashMap::new();
        let mut per_llm_call: HashMap<String, Vec<(u32, u32)>> = HashMap::new();

        for summary in &summaries {
            prompt_tokens = prompt_tokens.saturating_add(summary.token_usage.prompt_tokens);
            completion_tokens = completion_tokens.saturating_add(summary.token_usage.completion_tokens);
            total_tokens = total_tokens.saturating_add(summary.token_usage.total_tokens);
            per_run.insert(summary.run_id.clone(), summary.token_usage);

            if let Some(run) = self.run_store.load(&summary.run_id).await? {
                let mut llm_calls: Vec<(u32, u32)> = Vec::new();
                for event in &run.events {
                    if let RunEvent::LlmCall {
                        prompt_tokens: pt,
                        completion_tokens: ct,
                        ..
                    } = event
                    {
                        llm_calls.push((*pt, *ct));
                    }
                }
                if !llm_calls.is_empty() {
                    per_llm_call.insert(summary.run_id.clone(), llm_calls);
                }
            }
        }

        Ok(TokenBreakdown {
            prompt_tokens,
            completion_tokens,
            total_tokens,
            per_run,
            per_llm_call,
        })
    }

    /// Find tools whose average duration exceeds `threshold_ms`.
    pub async fn find_slow_tools(
        &self,
        threshold_ms: u64,
        limit: usize,
    ) -> crate::error::Result<Vec<ToolUsageStats>> {
        let all_stats = self.tool_usage_stats(limit).await?;
        Ok(all_stats
            .into_iter()
            .filter(|s| s.avg_duration_ms >= threshold_ms)
            .collect())
    }

    /// Analyze error patterns across runs. Groups errors by normalized
    /// message (lowercased, trimmed) and reports occurrence counts, associated
    /// tools, and timing.
    pub async fn error_pattern_analysis(&self, limit: usize) -> crate::error::Result<Vec<ErrorPattern>> {
        let summaries = self.run_store.list_all(limit).await?;

        // pattern_key -> accumulator
        let mut patterns: HashMap<String, ErrorAccumulator> = HashMap::new();

        for summary in &summaries {
            if let Some(run) = self.run_store.load(&summary.run_id).await? {
                // Run-level error
                if run.status == RunStatus::Failed {
                    if let Some(ref error_msg) = run.error {
                        let key = normalize_error(error_msg);
                        let acc = patterns.entry(key.clone()).or_default();
                        acc.occurrence_count += 1;
                        acc.run_ids.push(run.run_id.clone());
                        acc.update_time(run.started_at, run.finished_at);
                    }
                }

                // Event-level errors
                for event in &run.events {
                    match event {
                        RunEvent::ToolError { name, message, .. } => {
                            let key = normalize_error(message);
                            let acc = patterns.entry(key.clone()).or_default();
                            acc.occurrence_count += 1;
                            acc.run_ids.push(run.run_id.clone());
                            acc.associated_tools.insert(name.clone(), ());
                            acc.update_time(run.started_at, run.finished_at);
                        }
                        RunEvent::Error { message } => {
                            let key = normalize_error(message);
                            let acc = patterns.entry(key.clone()).or_default();
                            acc.occurrence_count += 1;
                            acc.run_ids.push(run.run_id.clone());
                            acc.update_time(run.started_at, run.finished_at);
                        }
                        _ => {}
                    }
                }
            }
        }

        let mut result: Vec<ErrorPattern> = patterns
            .into_iter()
            .map(|(pattern, acc)| ErrorPattern {
                pattern,
                occurrence_count: acc.occurrence_count,
                run_ids: acc.run_ids,
                associated_tools: acc.associated_tools.keys().cloned().collect(),
                first_seen: acc.first_seen,
                last_seen: acc.last_seen,
            })
            .collect();

        // Sort by occurrence count descending
        result.sort_by(|a, b| b.occurrence_count.cmp(&a.occurrence_count));
        Ok(result)
    }

    /// List all sessions (unique session IDs from stored runs).
    pub async fn list_sessions(&self, limit: usize) -> crate::error::Result<Vec<String>> {
        let summaries = self.run_store.list_all(limit).await?;
        let mut session_ids: Vec<String> = summaries
            .iter()
            .map(|s| s.session_id.clone())
            .collect();
        session_ids.sort();
        session_ids.dedup();
        Ok(session_ids)
    }
}

// ── Internal accumulators ──────────────────────────────────────────────

/// Accumulator for per-tool duration counts.
struct ToolAccumulator {
    call_count: usize,
    success_count: usize,
    failure_count: usize,
    total_duration_ms: u64,
    min_duration_ms: u64,
    max_duration_ms: u64,
}

impl Default for ToolAccumulator {
    fn default() -> Self {
        Self {
            call_count: 0,
            success_count: 0,
            failure_count: 0,
            total_duration_ms: 0,
            min_duration_ms: u64::MAX,
            max_duration_ms: 0,
        }
    }
}

/// Accumulator for error pattern grouping.
struct ErrorAccumulator {
    occurrence_count: usize,
    run_ids: Vec<String>,
    associated_tools: HashMap<String, ()>,
    first_seen: Option<DateTime<Utc>>,
    last_seen: Option<DateTime<Utc>>,
}

impl Default for ErrorAccumulator {
    fn default() -> Self {
        Self {
            occurrence_count: 0,
            run_ids: Vec::new(),
            associated_tools: HashMap::new(),
            first_seen: None,
            last_seen: None,
        }
    }
}

impl ErrorAccumulator {
    fn update_time(&mut self, started_at: DateTime<Utc>, finished_at: Option<DateTime<Utc>>) {
        if self.first_seen.is_none() || started_at < self.first_seen.unwrap() {
            self.first_seen = Some(started_at);
        }
        if let Some(fa) = finished_at {
            if self.last_seen.is_none() || fa > self.last_seen.unwrap() {
                self.last_seen = Some(fa);
            }
        }
    }
}

/// Normalize an error message for pattern grouping: lowercase, trim, collapse
/// repeated whitespace.
fn normalize_error(msg: &str) -> String {
    let lower = msg.to_lowercase();
    // Collapse runs of whitespace into a single space
    let mut result = String::with_capacity(lower.len());
    let mut prev_was_space = false;
    for ch in lower.trim().chars() {
        if ch.is_whitespace() {
            if !prev_was_space {
                result.push(' ');
                prev_was_space = true;
            }
        } else {
            result.push(ch);
            prev_was_space = false;
        }
    }
    result
}

// ── Unit tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace::InMemoryRunStore;

    fn make_run(id: &str, session: &str, status: RunStatus) -> Run {
        Run {
            run_id: id.to_string(),
            parent_run_id: None,
            session_id: session.to_string(),
            status,
            input: "test input".to_string(),
            events: vec![
                RunEvent::LlmCall {
                    messages: 1,
                    prompt_tokens: 100,
                    completion_tokens: 50,
                    duration_ms: 200,
                },
                RunEvent::ToolCall {
                    call_id: "c1".into(),
                    name: "read_file".into(),
                    args: None,
                    risk: None,
                    duration_ms: 50,
                },
                RunEvent::ToolResult {
                    call_id: "c1".into(),
                    name: "read_file".into(),
                    success: true,
                    output_preview: Some("ok".into()),
                    output_truncated: false,
                    duration_ms: 50,
                },
            ],
            final_output: if status == RunStatus::Completed {
                Some("ok".to_string())
            } else {
                None
            },
            error: if status == RunStatus::Failed {
                Some("something went wrong".to_string())
            } else {
                None
            },
            token_usage: TokenUsage {
                prompt_tokens: 100,
                completion_tokens: 50,
                total_tokens: 150,
            },
            timings: crate::trace::RunTimings {
                total_duration_ms: 300,
                llm_duration_ms: 200,
                tool_duration_ms: 100,
            },
            started_at: Utc::now(),
            finished_at: Some(Utc::now()),
        }
    }

    fn make_failed_run_with_tool_error(id: &str, session: &str) -> Run {
        Run {
            run_id: id.to_string(),
            parent_run_id: None,
            session_id: session.to_string(),
            status: RunStatus::Failed,
            input: "test".to_string(),
            events: vec![
                RunEvent::ToolCall {
                    call_id: "c2".into(),
                    name: "write_file".into(),
                    args: None,
                    risk: None,
                    duration_ms: 150,
                },
                RunEvent::ToolError {
                    call_id: "c2".into(),
                    name: "write_file".into(),
                    message: "Permission denied: cannot write to /etc/config".to_string(),
                },
            ],
            final_output: None,
            error: Some("permission denied".to_string()),
            token_usage: TokenUsage {
                prompt_tokens: 80,
                completion_tokens: 40,
                total_tokens: 120,
            },
            timings: crate::trace::RunTimings {
                total_duration_ms: 200,
                llm_duration_ms: 0,
                tool_duration_ms: 150,
            },
            started_at: Utc::now(),
            finished_at: Some(Utc::now()),
        }
    }

    #[tokio::test]
    async fn test_summarize_session() {
        let store = Arc::new(InMemoryRunStore::new());
        store.save(make_run("r1", "s1", RunStatus::Completed)).await.unwrap();
        store.save(make_run("r2", "s1", RunStatus::Failed)).await.unwrap();

        let analyzer = TraceAnalyzer::new(store);
        let summary = analyzer.summarize_session("s1").await.unwrap();
        assert_eq!(summary.session_id, "s1");
        assert_eq!(summary.run_count, 2);
        assert_eq!(summary.completed_count, 1);
        assert_eq!(summary.failed_count, 1);
        assert_eq!(summary.total_prompt_tokens, 200);
        assert_eq!(summary.total_tokens, 300);
        assert!(summary.tools_used.contains(&"read_file".to_string()));
        assert_eq!(summary.llm_call_count, 2);
    }

    #[tokio::test]
    async fn test_tool_usage_stats() {
        let store = Arc::new(InMemoryRunStore::new());
        store.save(make_run("r1", "s1", RunStatus::Completed)).await.unwrap();

        let analyzer = TraceAnalyzer::new(store);
        let stats = analyzer.tool_usage_stats(100).await.unwrap();
        assert!(!stats.is_empty());

        let read_file_stat = stats.iter().find(|s| s.name == "read_file").unwrap();
        assert_eq!(read_file_stat.call_count, 1);
        assert_eq!(read_file_stat.success_count, 1);
        assert_eq!(read_file_stat.total_duration_ms, 50);
    }

    #[tokio::test]
    async fn test_token_usage_breakdown() {
        let store = Arc::new(InMemoryRunStore::new());
        store.save(make_run("r1", "s1", RunStatus::Completed)).await.unwrap();

        let analyzer = TraceAnalyzer::new(store);
        let breakdown = analyzer.token_usage_breakdown(100).await.unwrap();
        assert_eq!(breakdown.prompt_tokens, 100);
        assert_eq!(breakdown.completion_tokens, 50);
        assert_eq!(breakdown.total_tokens, 150);
        assert!(breakdown.per_run.contains_key("r1"));
        assert!(breakdown.per_llm_call.contains_key("r1"));
    }

    #[tokio::test]
    async fn test_find_slow_tools() {
        let store = Arc::new(InMemoryRunStore::new());
        store.save(make_run("r1", "s1", RunStatus::Completed)).await.unwrap();

        let analyzer = TraceAnalyzer::new(store);
        // read_file avg duration is 50ms, so threshold=40 should include it
        let slow = analyzer.find_slow_tools(40, 100).await.unwrap();
        assert!(slow.iter().any(|s| s.name == "read_file"));

        // threshold=100 should exclude read_file (avg=50)
        let not_slow = analyzer.find_slow_tools(100, 100).await.unwrap();
        assert!(not_slow.is_empty());
    }

    #[tokio::test]
    async fn test_error_pattern_analysis() {
        let store = Arc::new(InMemoryRunStore::new());
        store.save(make_failed_run_with_tool_error("r1", "s1")).await.unwrap();

        let analyzer = TraceAnalyzer::new(store);
        let patterns = analyzer.error_pattern_analysis(100).await.unwrap();
        assert!(!patterns.is_empty());

        // Should find the "permission denied" pattern
        let perm_pattern = patterns.iter().find(|p| p.pattern.contains("permission denied")).unwrap();
        assert_eq!(perm_pattern.occurrence_count, 2); // run-level + event-level
        assert!(perm_pattern.associated_tools.contains(&"write_file".to_string()));
    }

    #[tokio::test]
    async fn test_list_sessions() {
        let store = Arc::new(InMemoryRunStore::new());
        store.save(make_run("r1", "s1", RunStatus::Completed)).await.unwrap();
        store.save(make_run("r2", "s2", RunStatus::Completed)).await.unwrap();

        let analyzer = TraceAnalyzer::new(store);
        let sessions = analyzer.list_sessions(100).await.unwrap();
        assert_eq!(sessions.len(), 2);
        assert!(sessions.contains(&"s1".to_string()));
        assert!(sessions.contains(&"s2".to_string()));
    }

    #[test]
    fn test_normalize_error() {
        assert_eq!(normalize_error("  Permission   DENIED:  /etc  "), "permission denied: /etc");
        assert_eq!(normalize_error("timeout"), "timeout");
        assert_eq!(normalize_error(""), "");
    }
}