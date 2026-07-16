//! TraceAnalyzer — observability over stored runs.
//!
//! Provides session-level summarization, tool usage statistics, token
//! breakdowns, slow-tool detection, and error-pattern analysis. All methods
//! operate on a shared [`RunStore`] backend, making them independent of the
//! storage implementation (in-memory, JSONL, etc.).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
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
    #[serde(with = "crate::utils::time::option_local_rfc3339")]
    pub first_started_at: Option<DateTime<Utc>>,
    /// Latest `finished_at` in the session.
    #[serde(with = "crate::utils::time::option_local_rfc3339")]
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
    #[serde(with = "crate::utils::time::option_local_rfc3339")]
    pub first_seen: Option<DateTime<Utc>>,
    /// Last occurrence timestamp.
    #[serde(with = "crate::utils::time::option_local_rfc3339")]
    pub last_seen: Option<DateTime<Utc>>,
}

/// Backward-compatible name for the runtime tool failure category.
pub type ToolFailureClass = crate::tools::ToolFailureCategory;

/// A recurring tool failure grouped without exposing raw tool arguments.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolFailurePattern {
    /// Tool that failed.
    pub tool_name: String,
    /// Deterministic failure class.
    pub error_class: ToolFailureClass,
    /// Normalized error preview used for grouping.
    pub pattern: String,
    /// Structural shape of the input, excluding argument values.
    pub input_shape: String,
    /// Number of failure occurrences.
    pub occurrence_count: usize,
    /// Number of distinct runs containing the failure.
    pub distinct_run_count: usize,
    /// Run IDs containing the failure.
    pub run_ids: Vec<String>,
    /// Repeated identical attempts after the first failure in the same run.
    pub ineffective_retry_count: usize,
    /// First run start containing the pattern.
    #[serde(with = "crate::utils::time::option_local_rfc3339")]
    pub first_seen: Option<DateTime<Utc>>,
    /// Last run finish containing the pattern.
    #[serde(with = "crate::utils::time::option_local_rfc3339")]
    pub last_seen: Option<DateTime<Utc>>,
}

/// Tool reliability summary across a bounded set of stored runs.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolReliabilityReport {
    pub run_count: usize,
    pub total_calls: usize,
    pub success_count: usize,
    pub failure_count: usize,
    pub ineffective_retry_count: usize,
    pub failure_patterns: Vec<ToolFailurePattern>,
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
    pub async fn summarize_session(
        &self,
        session_id: &str,
    ) -> crate::error::Result<SessionSummary> {
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
            total_prompt_tokens =
                total_prompt_tokens.saturating_add(summary.token_usage.prompt_tokens);
            total_completion_tokens =
                total_completion_tokens.saturating_add(summary.token_usage.completion_tokens);
            total_tokens = total_tokens.saturating_add(summary.token_usage.total_tokens);
            total_duration_ms += summary.total_duration_ms;

            if first_started_at
                .as_ref()
                .is_none_or(|first| summary.started_at < *first)
            {
                first_started_at = Some(summary.started_at);
            }
            if let Some(fa) = summary.finished_at
                && last_finished_at.as_ref().is_none_or(|last| fa > *last)
            {
                last_finished_at = Some(fa);
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
    pub async fn tool_usage_stats(
        &self,
        limit: usize,
    ) -> crate::error::Result<Vec<ToolUsageStats>> {
        let summaries = self.run_store.list_all(limit).await?;

        // Accumulate per-tool data
        let mut tool_data: HashMap<String, ToolAccumulator> = HashMap::new();

        for summary in &summaries {
            if let Some(run) = self.run_store.load(&summary.run_id).await? {
                let mut failed_call_ids = HashSet::new();
                for event in &run.events {
                    match event {
                        RunEvent::ToolCall {
                            name, duration_ms, ..
                        } => {
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
                        RunEvent::ToolResult {
                            call_id,
                            name,
                            success,
                            ..
                        } => {
                            if let Some(acc) = tool_data.get_mut(name) {
                                if *success {
                                    acc.success_count += 1;
                                } else if failed_call_ids.insert(call_id.clone()) {
                                    acc.failure_count += 1;
                                }
                            }
                        }
                        RunEvent::ToolError { call_id, name, .. } => {
                            if failed_call_ids.insert(call_id.clone())
                                && let Some(acc) = tool_data.get_mut(name)
                            {
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
        stats.sort_by_key(|b| std::cmp::Reverse(b.total_duration_ms));
        Ok(stats)
    }

    /// Compute token usage breakdown across all runs (up to `limit`).
    pub async fn token_usage_breakdown(
        &self,
        limit: usize,
    ) -> crate::error::Result<TokenBreakdown> {
        let summaries = self.run_store.list_all(limit).await?;

        let mut prompt_tokens = 0u32;
        let mut completion_tokens = 0u32;
        let mut total_tokens = 0u32;
        let mut per_run: HashMap<String, TokenUsage> = HashMap::new();
        let mut per_llm_call: HashMap<String, Vec<(u32, u32)>> = HashMap::new();

        for summary in &summaries {
            prompt_tokens = prompt_tokens.saturating_add(summary.token_usage.prompt_tokens);
            completion_tokens =
                completion_tokens.saturating_add(summary.token_usage.completion_tokens);
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
    pub async fn error_pattern_analysis(
        &self,
        limit: usize,
    ) -> crate::error::Result<Vec<ErrorPattern>> {
        let summaries = self.run_store.list_all(limit).await?;

        // pattern_key -> accumulator
        let mut patterns: HashMap<String, ErrorAccumulator> = HashMap::new();

        for summary in &summaries {
            if let Some(run) = self.run_store.load(&summary.run_id).await? {
                // Run-level error
                if run.status == RunStatus::Failed
                    && let Some(ref error_msg) = run.error
                {
                    let key = normalize_error(error_msg);
                    let acc = patterns.entry(key.clone()).or_default();
                    acc.occurrence_count += 1;
                    acc.run_ids.push(run.run_id.clone());
                    acc.update_time(run.started_at, run.finished_at);
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
        result.sort_by_key(|b| std::cmp::Reverse(b.occurrence_count));
        Ok(result)
    }

    /// Analyze tool reliability for runs at or after `after`.
    ///
    /// Tool arguments are used only to detect repeated identical attempts in one
    /// run. Returned patterns expose a structural input shape, never raw values.
    pub async fn tool_reliability_report(
        &self,
        limit: usize,
        after: Option<DateTime<Utc>>,
    ) -> crate::error::Result<ToolReliabilityReport> {
        let summaries = self.run_store.list_all(limit).await?;
        let mut report = ToolReliabilityReport::default();
        let mut patterns: HashMap<String, ToolFailureAccumulator> = HashMap::new();

        for summary in summaries
            .iter()
            .filter(|summary| after.is_none_or(|after| summary.started_at >= after))
        {
            let Some(run) = self.run_store.load(&summary.run_id).await? else {
                continue;
            };
            report.run_count = report.run_count.saturating_add(1);
            let mut calls: HashMap<String, ToolCallContext> = HashMap::new();
            let mut attempts: HashMap<String, usize> = HashMap::new();
            let mut failed_call_ids = HashSet::new();

            for event in &run.events {
                match event {
                    RunEvent::ToolCall {
                        call_id,
                        name,
                        args,
                        ..
                    } => {
                        report.total_calls = report.total_calls.saturating_add(1);
                        calls.insert(
                            call_id.clone(),
                            ToolCallContext {
                                tool_name: name.clone(),
                                input_shape: json_shape(args.as_ref()),
                                args_key: json_identity(args.as_ref()),
                            },
                        );
                    }
                    RunEvent::ToolResult { success: true, .. } => {
                        report.success_count = report.success_count.saturating_add(1);
                    }
                    RunEvent::ToolError {
                        call_id,
                        name,
                        message,
                        failure,
                    } if failed_call_ids.insert(call_id.clone()) => {
                        report.failure_count = report.failure_count.saturating_add(1);
                        record_tool_failure(
                            &mut patterns,
                            &mut attempts,
                            &run,
                            calls.get(call_id),
                            name,
                            message,
                            failure.as_ref(),
                        );
                    }
                    _ => {}
                }
            }
        }

        let mut failure_patterns = patterns
            .into_values()
            .map(|accumulator| accumulator.into_pattern())
            .collect::<Vec<_>>();
        failure_patterns.sort_by_key(|pattern| std::cmp::Reverse(pattern.occurrence_count));
        report.ineffective_retry_count = failure_patterns
            .iter()
            .map(|pattern| pattern.ineffective_retry_count)
            .sum();
        report.failure_patterns = failure_patterns;
        Ok(report)
    }

    /// List all sessions (unique session IDs from stored runs).
    pub async fn list_sessions(&self, limit: usize) -> crate::error::Result<Vec<String>> {
        let summaries = self.run_store.list_all(limit).await?;
        let mut session_ids: Vec<String> = summaries.iter().map(|s| s.session_id.clone()).collect();
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
#[derive(Default)]
struct ErrorAccumulator {
    occurrence_count: usize,
    run_ids: Vec<String>,
    associated_tools: HashMap<String, ()>,
    first_seen: Option<DateTime<Utc>>,
    last_seen: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
struct ToolCallContext {
    tool_name: String,
    input_shape: String,
    args_key: String,
}

#[derive(Default)]
struct ToolFailureAccumulator {
    tool_name: String,
    error_class: Option<ToolFailureClass>,
    pattern: String,
    input_shape: String,
    occurrence_count: usize,
    run_ids: HashMap<String, ()>,
    ineffective_retry_count: usize,
    first_seen: Option<DateTime<Utc>>,
    last_seen: Option<DateTime<Utc>>,
}

impl ToolFailureAccumulator {
    fn into_pattern(self) -> ToolFailurePattern {
        let mut run_ids = self.run_ids.into_keys().collect::<Vec<_>>();
        run_ids.sort();
        ToolFailurePattern {
            tool_name: self.tool_name,
            error_class: self.error_class.unwrap_or(ToolFailureClass::Permanent),
            pattern: self.pattern,
            input_shape: self.input_shape,
            occurrence_count: self.occurrence_count,
            distinct_run_count: run_ids.len(),
            run_ids,
            ineffective_retry_count: self.ineffective_retry_count,
            first_seen: self.first_seen,
            last_seen: self.last_seen,
        }
    }
}

impl ErrorAccumulator {
    fn update_time(&mut self, started_at: DateTime<Utc>, finished_at: Option<DateTime<Utc>>) {
        if self
            .first_seen
            .as_ref()
            .is_none_or(|first| started_at < *first)
        {
            self.first_seen = Some(started_at);
        }
        if let Some(fa) = finished_at
            && self.last_seen.as_ref().is_none_or(|last| fa > *last)
        {
            self.last_seen = Some(fa);
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

fn classify_tool_failure(message: &str) -> ToolFailureClass {
    let normalized = message.to_lowercase();
    if normalized.contains("timeout")
        || normalized.contains("timed out")
        || normalized.contains("超时")
    {
        ToolFailureClass::Timeout
    } else if normalized.contains("permission denied")
        || normalized.contains("access denied")
        || normalized.contains("not permitted")
        || normalized.contains("权限不足")
        || normalized.contains("拒绝访问")
    {
        ToolFailureClass::Permanent
    } else if normalized.contains("not found")
        || normalized.contains("no such file")
        || normalized.contains("未找到")
        || normalized.contains("不存在")
        || normalized.contains("invalid parameter")
        || normalized.contains("missing parameter")
        || normalized.contains("invalid argument")
        || normalized.contains("无效参数")
        || normalized.contains("缺少参数")
    {
        ToolFailureClass::InvalidArguments
    } else if normalized.contains("network")
        || normalized.contains("connection")
        || normalized.contains("dns")
        || normalized.contains("网络")
        || normalized.contains("连接")
    {
        ToolFailureClass::Transient
    } else if normalized.contains("dependency")
        || normalized.contains("module")
        || normalized.contains("package")
        || normalized.contains("依赖")
        || normalized.contains("模块")
        || normalized.contains("软件包")
    {
        ToolFailureClass::Unavailable
    } else if normalized.contains("cancelled")
        || normalized.contains("canceled")
        || normalized.contains("取消")
    {
        ToolFailureClass::Cancelled
    } else {
        ToolFailureClass::Permanent
    }
}

fn json_shape(value: Option<&serde_json::Value>) -> String {
    json_shape_at(value, 0)
}

fn json_shape_at(value: Option<&serde_json::Value>, depth: usize) -> String {
    if depth >= 4 {
        return "nested".to_string();
    }
    match value {
        None | Some(serde_json::Value::Null) => "none".to_string(),
        Some(serde_json::Value::Bool(_)) => "bool".to_string(),
        Some(serde_json::Value::Number(_)) => "number".to_string(),
        Some(serde_json::Value::String(_)) => "string".to_string(),
        Some(serde_json::Value::Array(values)) => {
            let item_shape = values
                .first()
                .map(|value| json_shape_at(Some(value), depth.saturating_add(1)))
                .unwrap_or_else(|| "empty".to_string());
            format!("array<{item_shape}>")
        }
        Some(serde_json::Value::Object(values)) => {
            let mut fields = values
                .iter()
                .map(|(key, value)| {
                    format!(
                        "{key}:{}",
                        json_shape_at(Some(value), depth.saturating_add(1))
                    )
                })
                .collect::<Vec<_>>();
            fields.sort();
            fields.truncate(24);
            format!("object{{{}}}", fields.join(","))
        }
    }
}

fn json_identity(value: Option<&serde_json::Value>) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    hash_json_value(value, 0, &mut hasher);
    format!("{:016x}", hasher.finish())
}

fn hash_json_value<H: Hasher>(value: Option<&serde_json::Value>, depth: usize, hasher: &mut H) {
    if depth >= 8 {
        "nested".hash(hasher);
        return;
    }
    match value {
        None | Some(serde_json::Value::Null) => "null".hash(hasher),
        Some(serde_json::Value::Bool(value)) => value.hash(hasher),
        Some(serde_json::Value::Number(value)) => value.to_string().hash(hasher),
        Some(serde_json::Value::String(value)) => value.hash(hasher),
        Some(serde_json::Value::Array(values)) => {
            "array".hash(hasher);
            values.len().hash(hasher);
            for value in values.iter().take(32) {
                hash_json_value(Some(value), depth.saturating_add(1), hasher);
            }
        }
        Some(serde_json::Value::Object(values)) => {
            "object".hash(hasher);
            values.len().hash(hasher);
            let mut fields = values.iter().collect::<Vec<_>>();
            fields.sort_by_key(|(left, _)| *left);
            for (key, value) in fields.into_iter().take(32) {
                key.hash(hasher);
                hash_json_value(Some(value), depth.saturating_add(1), hasher);
            }
        }
    }
}

fn record_tool_failure(
    patterns: &mut HashMap<String, ToolFailureAccumulator>,
    attempts: &mut HashMap<String, usize>,
    run: &crate::trace::Run,
    call: Option<&ToolCallContext>,
    event_tool_name: &str,
    message: &str,
    failure: Option<&crate::tools::ToolFailure>,
) {
    let tool_name = call
        .map(|context| context.tool_name.as_str())
        .unwrap_or(event_tool_name);
    let input_shape = call
        .map(|context| context.input_shape.as_str())
        .unwrap_or("unknown");
    let args_key = call.map(|context| context.args_key.as_str()).unwrap_or("");
    let error_class = failure
        .map(|failure| failure.category)
        .unwrap_or_else(|| classify_tool_failure(message));
    let pattern = normalize_error(message)
        .chars()
        .take(200)
        .collect::<String>();
    let grouping_pattern = "classified";
    let key = format!("{tool_name}|{error_class:?}|{input_shape}|{grouping_pattern}");
    let attempt_key = format!("{key}|{args_key}");
    let prior_attempts = attempts.entry(attempt_key).or_insert(0);
    let is_retry = *prior_attempts > 0;
    *prior_attempts = prior_attempts.saturating_add(1);

    let accumulator = patterns.entry(key).or_default();
    accumulator.tool_name = tool_name.to_string();
    accumulator.error_class = Some(error_class);
    accumulator.pattern = pattern;
    accumulator.input_shape = input_shape.to_string();
    accumulator.occurrence_count = accumulator.occurrence_count.saturating_add(1);
    accumulator.run_ids.insert(run.run_id.clone(), ());
    if is_retry {
        accumulator.ineffective_retry_count = accumulator.ineffective_retry_count.saturating_add(1);
    }
    accumulator.first_seen = match accumulator.first_seen {
        Some(first_seen) => Some(first_seen.min(run.started_at)),
        None => Some(run.started_at),
    };
    let last_seen = run.finished_at.unwrap_or(run.started_at);
    accumulator.last_seen = match accumulator.last_seen {
        Some(current) => Some(current.max(last_seen)),
        None => Some(last_seen),
    };
}

// ── Unit tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace::{InMemoryRunStore, Run};

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
                    cached_prompt_tokens: 80,
                    cache_creation_prompt_tokens: 0,
                    usage_reported: true,
                    estimated_context_tokens: 95,
                    protected_context_tokens: 20,
                    protected_message_count: 1,
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
                    original_bytes: 0,
                    returned_bytes: 0,
                    estimated_tokens: 0,
                    output_handling: None,
                    artifact: None,
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
                ..Default::default()
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
                    failure: None,
                },
            ],
            final_output: None,
            error: Some("something went wrong".to_string()),
            token_usage: TokenUsage {
                prompt_tokens: 80,
                completion_tokens: 40,
                total_tokens: 120,
                ..Default::default()
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
        store
            .save(make_run("r1", "s1", RunStatus::Completed))
            .await
            .unwrap();
        store
            .save(make_run("r2", "s1", RunStatus::Failed))
            .await
            .unwrap();

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
        store
            .save(make_run("r1", "s1", RunStatus::Completed))
            .await
            .unwrap();

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
        store
            .save(make_run("r1", "s1", RunStatus::Completed))
            .await
            .unwrap();

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
        store
            .save(make_run("r1", "s1", RunStatus::Completed))
            .await
            .unwrap();

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
        store
            .save(make_failed_run_with_tool_error("r1", "s1"))
            .await
            .unwrap();

        let analyzer = TraceAnalyzer::new(store);
        let patterns = analyzer.error_pattern_analysis(100).await.unwrap();
        assert!(!patterns.is_empty());

        // "permission denied" only appears in the ToolError event, not in the
        // run-level error message ("something went wrong"), so count is 1.
        let perm_pattern = patterns
            .iter()
            .find(|p| p.pattern.contains("permission denied"))
            .unwrap();
        assert_eq!(perm_pattern.occurrence_count, 1);
        assert!(
            perm_pattern
                .associated_tools
                .contains(&"write_file".to_string())
        );
    }

    #[tokio::test]
    async fn tool_reliability_groups_cross_run_failures_and_retries() -> crate::error::Result<()> {
        let store = Arc::new(InMemoryRunStore::new());
        let args = serde_json::json!({"path": "/protected/config"});
        let mut first = make_run("r1", "s1", RunStatus::Failed);
        first.events = vec![
            RunEvent::ToolCall {
                call_id: "c1".into(),
                name: "write_file".into(),
                args: Some(args.clone()),
                risk: None,
                duration_ms: 10,
            },
            RunEvent::ToolResult {
                call_id: "c1".into(),
                name: "write_file".into(),
                success: false,
                output_preview: Some("Permission denied: /protected/config".into()),
                output_truncated: false,
                duration_ms: 10,
                original_bytes: 0,
                returned_bytes: 0,
                estimated_tokens: 0,
                output_handling: None,
                artifact: None,
            },
            RunEvent::ToolError {
                call_id: "c1".into(),
                name: "write_file".into(),
                message: "Permission denied: /protected/config".into(),
                failure: None,
            },
            RunEvent::ToolCall {
                call_id: "c2".into(),
                name: "write_file".into(),
                args: Some(args),
                risk: None,
                duration_ms: 10,
            },
            RunEvent::ToolResult {
                call_id: "c2".into(),
                name: "write_file".into(),
                success: false,
                output_preview: Some("Permission denied: /protected/config".into()),
                output_truncated: false,
                duration_ms: 10,
                original_bytes: 0,
                returned_bytes: 0,
                estimated_tokens: 0,
                output_handling: None,
                artifact: None,
            },
            RunEvent::ToolError {
                call_id: "c2".into(),
                name: "write_file".into(),
                message: "Permission denied: /protected/config".into(),
                failure: None,
            },
        ];
        let mut second = make_run("r2", "s1", RunStatus::Failed);
        second.events = vec![
            RunEvent::ToolCall {
                call_id: "c3".into(),
                name: "write_file".into(),
                args: Some(serde_json::json!({"path": "/other/config"})),
                risk: None,
                duration_ms: 10,
            },
            RunEvent::ToolResult {
                call_id: "c3".into(),
                name: "write_file".into(),
                success: false,
                output_preview: Some("Access denied: /other/config".into()),
                output_truncated: false,
                duration_ms: 10,
                original_bytes: 0,
                returned_bytes: 0,
                estimated_tokens: 0,
                output_handling: None,
                artifact: None,
            },
            RunEvent::ToolError {
                call_id: "c3".into(),
                name: "write_file".into(),
                message: "Access denied: /other/config".into(),
                failure: None,
            },
        ];
        store.save(first).await?;
        store.save(second).await?;

        let analyzer = TraceAnalyzer::new(store);
        let usage = analyzer.tool_usage_stats(100).await?;
        let write_stats = usage
            .iter()
            .find(|stats| stats.name == "write_file")
            .ok_or_else(|| crate::error::ReactError::Other("missing write_file stats".into()))?;
        assert_eq!(write_stats.failure_count, 3);

        let report = analyzer.tool_reliability_report(100, None).await?;
        assert_eq!(report.total_calls, 3);
        assert_eq!(report.failure_count, 3);
        assert_eq!(report.ineffective_retry_count, 1);
        let pattern = report
            .failure_patterns
            .first()
            .ok_or_else(|| crate::error::ReactError::Other("missing failure pattern".into()))?;
        assert_eq!(pattern.error_class, ToolFailureClass::Permanent);
        assert_eq!(pattern.occurrence_count, 3);
        assert_eq!(pattern.distinct_run_count, 2);
        assert_eq!(pattern.ineffective_retry_count, 1);
        assert!(!pattern.input_shape.contains("protected"));
        Ok(())
    }

    #[tokio::test]
    async fn tool_reliability_prefers_structured_failure_category() -> crate::error::Result<()> {
        let store = Arc::new(InMemoryRunStore::new());
        let mut run = make_run("structured", "s1", RunStatus::Failed);
        run.events = vec![
            RunEvent::ToolCall {
                call_id: "c1".into(),
                name: "web_search".into(),
                args: Some(serde_json::json!({"query": "rust"})),
                risk: None,
                duration_ms: 5,
            },
            RunEvent::ToolError {
                call_id: "c1".into(),
                name: "web_search".into(),
                message: "permission denied wording from an upstream proxy".into(),
                failure: Some(crate::tools::ToolFailure::new(
                    crate::tools::ToolFailureCategory::Transient,
                )),
            },
        ];
        store.save(run).await?;

        let report = TraceAnalyzer::new(store)
            .tool_reliability_report(10, None)
            .await?;
        let pattern = report
            .failure_patterns
            .first()
            .ok_or_else(|| crate::error::ReactError::Other("missing failure pattern".into()))?;

        assert_eq!(pattern.error_class, ToolFailureClass::Transient);
        Ok(())
    }

    #[tokio::test]
    async fn test_list_sessions() {
        let store = Arc::new(InMemoryRunStore::new());
        store
            .save(make_run("r1", "s1", RunStatus::Completed))
            .await
            .unwrap();
        store
            .save(make_run("r2", "s2", RunStatus::Completed))
            .await
            .unwrap();

        let analyzer = TraceAnalyzer::new(store);
        let sessions = analyzer.list_sessions(100).await.unwrap();
        assert_eq!(sessions.len(), 2);
        assert!(sessions.contains(&"s1".to_string()));
        assert!(sessions.contains(&"s2".to_string()));
    }

    #[test]
    fn test_normalize_error() {
        assert_eq!(
            normalize_error("  Permission   DENIED:  /etc  "),
            "permission denied: /etc"
        );
        assert_eq!(normalize_error("timeout"), "timeout");
        assert_eq!(normalize_error(""), "");
    }
}
