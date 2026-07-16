//! Trajectory replay — offline analysis of past agent runs.
//!
//! Given a [`Run`](crate::trace::Run) loaded from a [`RunStore`](crate::trace::RunStore),
//! the replay system extracts patterns, detects policy violations, and
//! generates eval metrics without re-running the agent.

use crate::eval::{EvalConstraints, EvalMetric, EvalResult};
use crate::tools::{is_read_tool, is_write_tool};
use crate::trace::{Run, RunEvent};

/// Replay analyzer for a completed run.
pub struct TrajectoryReplay {
    pub run: Run,
}

impl TrajectoryReplay {
    /// Create a replay from a loaded run.
    pub fn new(run: Run) -> Self {
        Self { run }
    }

    /// Count tool calls by type.
    pub fn tool_call_counts(&self) -> Vec<(&str, usize)> {
        let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for event in &self.run.events {
            if let RunEvent::ToolCall { name, .. } = event {
                *counts.entry(name.as_str()).or_default() += 1;
            }
        }
        let mut result: Vec<_> = counts.into_iter().collect();
        result.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
        result
    }

    /// Count total tool calls.
    pub fn total_tool_calls(&self) -> usize {
        self.run
            .events
            .iter()
            .filter(|e| matches!(e, RunEvent::ToolCall { .. }))
            .count()
    }

    /// Count errors (tool errors + run errors).
    pub fn error_count(&self) -> usize {
        self.run
            .events
            .iter()
            .filter(|e| matches!(e, RunEvent::ToolError { .. } | RunEvent::Error { .. }))
            .count()
    }

    /// Validate model-independent lifecycle invariants in the recorded run.
    pub fn contract_violations(&self) -> Vec<String> {
        let mut violations = Vec::new();
        let mut pending = std::collections::HashMap::<String, String>::new();
        let mut completed = std::collections::HashMap::<String, String>::new();
        let mut last_iteration = 0_usize;

        for event in &self.run.events {
            match event {
                RunEvent::ToolCall { call_id, name, .. } => {
                    if pending.insert(call_id.clone(), name.clone()).is_some()
                        || completed.contains_key(call_id)
                    {
                        violations.push(format!("duplicate tool call id: {call_id}"));
                    }
                }
                RunEvent::ToolResult {
                    call_id, name, ..
                } => match pending.remove(call_id) {
                    Some(expected_name) if expected_name == *name => {
                        if completed.insert(call_id.clone(), name.clone()).is_some() {
                            violations.push(format!("duplicate tool completion: {call_id}"));
                        }
                    }
                    Some(expected_name) => violations.push(format!(
                        "tool completion name mismatch for {call_id}: expected {expected_name}, got {name}"
                    )),
                    None => violations.push(format!("orphan tool completion: {call_id}")),
                },
                RunEvent::ToolError {
                    call_id, name, ..
                } => {
                    if let Some(completed_name) = completed.get(call_id) {
                        if completed_name != name {
                            violations.push(format!(
                                "tool error name mismatch for {call_id}: expected {completed_name}, got {name}"
                            ));
                        }
                    } else {
                        match pending.remove(call_id) {
                            Some(expected_name) if expected_name == *name => {
                                completed.insert(call_id.clone(), name.clone());
                            }
                            Some(expected_name) => violations.push(format!(
                                "tool error name mismatch for {call_id}: expected {expected_name}, got {name}"
                            )),
                            None => violations.push(format!("orphan tool error: {call_id}")),
                        }
                    }
                }
                RunEvent::PhaseTransition { iteration, .. } => {
                    if *iteration < last_iteration {
                        violations.push(format!(
                            "phase iteration regressed from {last_iteration} to {iteration}"
                        ));
                    }
                    last_iteration = *iteration;
                }
                RunEvent::SubAgentRun { outcome, .. }
                    if !matches!(outcome.as_str(), "completed" | "failed" | "cancelled") =>
                {
                    violations.push(format!("invalid subagent outcome: {outcome}"));
                }
                _ => {}
            }
        }

        let mut unfinished = pending.into_keys().collect::<Vec<_>>();
        unfinished.sort();
        for call_id in unfinished {
            violations.push(format!("tool call without completion: {call_id}"));
        }
        violations
    }

    /// Normalize a path for comparison (resolve ../, ./, //).
    fn normalize_path(path: &str) -> String {
        let p = path.replace('\\', "/").replace("//", "/");
        let parts: Vec<&str> = p.split('/').collect();
        let mut out: Vec<&str> = Vec::new();
        for part in parts {
            match part {
                "." | "" => {}
                ".." => {
                    out.pop();
                }
                _ => out.push(part),
            }
        }
        out.join("/")
    }

    /// Extract file paths that were written to, using ToolCall args.
    pub fn written_files(&self) -> Vec<String> {
        let mut files = Vec::new();
        for event in &self.run.events {
            if let RunEvent::ToolCall { name, args, .. } = event
                && (is_write_tool(name) || is_read_tool(name))
                && let Some(args) = args
                && let Some(path) = args.get("path").or_else(|| args.get("file_path"))
                && let Some(s) = path.as_str()
            {
                files.push(Self::normalize_path(s));
            }
        }
        files
    }

    /// Check if any writes happened without a prior read, using ToolCall args.
    /// Paths are normalized for comparison to handle ../, ./, symlink variations.
    pub fn detect_write_without_read(&self) -> Vec<String> {
        let mut violations = Vec::new();
        let mut read_files: std::collections::HashSet<String> = std::collections::HashSet::new();

        for event in &self.run.events {
            match event {
                RunEvent::ToolCall { name, args, .. } if is_read_tool(name) => {
                    if let Some(args) = args
                        && let Some(path) = args.get("path").or_else(|| args.get("file_path"))
                        && let Some(s) = path.as_str()
                    {
                        read_files.insert(Self::normalize_path(s));
                    }
                }
                RunEvent::ToolResult { name, success, .. } if is_read_tool(name) && *success => {
                    // Read confirmed successful — path already tracked from ToolCall
                }
                RunEvent::ToolCall { name, args, .. } if is_write_tool(name) => {
                    if let Some(args) = args
                        && let Some(path) = args.get("path").or_else(|| args.get("file_path"))
                        && let Some(s) = path.as_str()
                    {
                        let normalized = Self::normalize_path(s);
                        if !read_files.contains(&normalized) {
                            violations.push(format!("Write without read: {name} on {normalized}"));
                        }
                    }
                }
                _ => {}
            }
        }
        violations
    }

    /// Evaluate constraints against this run.
    pub fn evaluate_constraints(&self, constraints: &EvalConstraints) -> Vec<String> {
        let mut violations = self.contract_violations();

        // Check max tool calls
        if let Some(max) = constraints.max_tool_calls {
            let total = self.total_tool_calls();
            if total > max {
                violations.push(format!("Too many tool calls: {total} (max {max})"));
            }
        }

        // Check required read-before-edit
        if constraints.required_read_before_edit {
            let rbe_violations = self.detect_write_without_read();
            violations.extend(rbe_violations);
        }

        violations
    }

    /// Generate eval metrics from this trajectory.
    pub fn to_metrics(&self, constraints: &EvalConstraints) -> Vec<EvalMetric> {
        let total_calls = self.total_tool_calls();
        let errors = self.error_count();
        let efficiency = if total_calls > 0 {
            1.0 - (errors as f64 / total_calls as f64).min(1.0)
        } else {
            1.0
        };

        let violations = self.evaluate_constraints(constraints);
        let constraint_score = if violations.is_empty() {
            1.0
        } else {
            1.0 - (violations.len() as f64 * 0.2).min(1.0)
        };

        vec![
            EvalMetric {
                name: "tool_calls".into(),
                score: 1.0, // neutral
                detail: format!("{total_calls} total tool calls"),
            },
            EvalMetric {
                name: "error_rate".into(),
                score: efficiency,
                detail: format!("{errors} errors in {total_calls} calls"),
            },
            EvalMetric {
                name: "constraint_compliance".into(),
                score: constraint_score,
                detail: format!("{} violations", violations.len()),
            },
        ]
    }

    /// Evaluate this run against constraints and produce an EvalResult.
    pub fn evaluate(&self, case_id: &str, constraints: &EvalConstraints) -> EvalResult {
        let violations = self.evaluate_constraints(constraints);
        let metrics = self.to_metrics(constraints);
        let success = violations.is_empty();
        let score = if metrics.is_empty() {
            if success { 1.0 } else { 0.0 }
        } else {
            metrics.iter().map(|m| m.score).sum::<f64>() / metrics.len() as f64
        };

        EvalResult {
            case_id: case_id.to_string(),
            success,
            score,
            metrics,
            violations,
            run_id: Some(self.run.run_id.clone()),
            duration_ms: self.run.timings.total_duration_ms,
            tool_calls: self.total_tool_calls(),
            tokens_in: self.run.token_usage.prompt_tokens,
            tokens_out: self.run.token_usage.completion_tokens,
            cached_tokens_in: self.run.token_usage.cached_prompt_tokens,
            cache_creation_tokens_in: self.run.token_usage.cache_creation_prompt_tokens,
            cache_hit_rate: self.run.token_usage.cache_hit_rate(),
            tool_errors: self
                .run
                .events
                .iter()
                .filter(|event| matches!(event, RunEvent::ToolError { .. }))
                .count(),
            max_protected_context_tokens: self
                .run
                .events
                .iter()
                .filter_map(|event| match event {
                    RunEvent::LlmCall {
                        protected_context_tokens,
                        ..
                    } => Some(*protected_context_tokens),
                    _ => None,
                })
                .max()
                .unwrap_or(0),
            file_changes: self.written_files().len(),
            compile_ok: None,
            tests_pass: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace::{RunStatus, RunTimings, TokenUsage};
    use chrono::Utc;

    fn make_run(events: Vec<RunEvent>) -> Run {
        Run {
            run_id: "test_run".into(),
            parent_run_id: None,
            session_id: "test_session".into(),
            status: RunStatus::Completed,
            input: "test".into(),
            events,
            final_output: Some("ok".into()),
            error: None,
            token_usage: TokenUsage::default(),
            timings: RunTimings::default(),
            started_at: Utc::now(),
            finished_at: Some(Utc::now()),
        }
    }

    #[test]
    fn test_tool_call_counts() {
        let run = make_run(vec![
            RunEvent::ToolCall {
                call_id: "test_call".into(),
                name: "read_file".into(),
                args: None,
                risk: None,
                duration_ms: 10,
            },
            RunEvent::ToolResult {
                call_id: "test_call".into(),
                name: "read_file".into(),
                success: true,
                output_preview: None,
                output_truncated: false,
                duration_ms: 0,
                original_bytes: 0,
                returned_bytes: 0,
                estimated_tokens: 0,
                output_handling: None,
                artifact: None,
            },
            RunEvent::ToolCall {
                call_id: "test_call".into(),
                name: "edit_file".into(),
                args: None,
                risk: None,
                duration_ms: 20,
            },
            RunEvent::ToolResult {
                call_id: "test_call".into(),
                name: "edit_file".into(),
                success: true,
                output_preview: None,
                output_truncated: false,
                duration_ms: 0,
                original_bytes: 0,
                returned_bytes: 0,
                estimated_tokens: 0,
                output_handling: None,
                artifact: None,
            },
            RunEvent::ToolCall {
                call_id: "test_call".into(),
                name: "read_file".into(),
                args: None,
                risk: None,
                duration_ms: 5,
            },
            RunEvent::ToolResult {
                call_id: "test_call".into(),
                name: "read_file".into(),
                success: true,
                output_preview: None,
                output_truncated: false,
                duration_ms: 0,
                original_bytes: 0,
                returned_bytes: 0,
                estimated_tokens: 0,
                output_handling: None,
                artifact: None,
            },
        ]);
        let replay = TrajectoryReplay::new(run);
        let counts = replay.tool_call_counts();
        assert_eq!(counts.len(), 2);
        assert_eq!(replay.total_tool_calls(), 3);
    }

    #[test]
    fn test_constraint_max_tool_calls() {
        let run = make_run(vec![
            RunEvent::ToolCall {
                call_id: "test_call".into(),
                name: "read_file".into(),
                args: None,
                risk: None,
                duration_ms: 10,
            },
            RunEvent::ToolResult {
                call_id: "test_call".into(),
                name: "read_file".into(),
                success: true,
                output_preview: None,
                output_truncated: false,
                duration_ms: 0,
                original_bytes: 0,
                returned_bytes: 0,
                estimated_tokens: 0,
                output_handling: None,
                artifact: None,
            },
            RunEvent::ToolCall {
                call_id: "test_call".into(),
                name: "edit_file".into(),
                args: None,
                risk: None,
                duration_ms: 20,
            },
            RunEvent::ToolResult {
                call_id: "test_call".into(),
                name: "edit_file".into(),
                success: true,
                output_preview: None,
                output_truncated: false,
                duration_ms: 0,
                original_bytes: 0,
                returned_bytes: 0,
                estimated_tokens: 0,
                output_handling: None,
                artifact: None,
            },
        ]);
        let replay = TrajectoryReplay::new(run);
        let constraints = EvalConstraints {
            max_tool_calls: Some(1),
            ..Default::default()
        };
        let violations = replay.evaluate_constraints(&constraints);
        assert!(!violations.is_empty());
    }

    #[test]
    fn test_error_count() {
        let run = make_run(vec![
            RunEvent::ToolCall {
                call_id: "test_call".into(),
                name: "read_file".into(),
                args: None,
                risk: None,
                duration_ms: 10,
            },
            RunEvent::ToolError {
                call_id: "test_call".into(),
                name: "read_file".into(),
                message: "not found".into(),
                failure: None,
            },
        ]);
        let replay = TrajectoryReplay::new(run);
        assert_eq!(replay.error_count(), 1);
    }

    #[test]
    fn contract_accepts_canonical_tool_and_subagent_trajectory() {
        let run = make_run(vec![
            RunEvent::PhaseTransition {
                phase: "think".into(),
                iteration: 0,
            },
            RunEvent::ToolCall {
                call_id: "write-你好-1".into(),
                name: "write_file".into(),
                args: Some(serde_json::json!({"path": "结果-🧪.md"})),
                risk: None,
                duration_ms: 1,
            },
            RunEvent::ToolResult {
                call_id: "write-你好-1".into(),
                name: "write_file".into(),
                success: true,
                output_preview: Some("完成".into()),
                output_truncated: false,
                duration_ms: 1,
                original_bytes: 0,
                returned_bytes: 0,
                estimated_tokens: 0,
                output_handling: None,
                artifact: None,
            },
            RunEvent::SubAgentRun {
                agent_name: "reviewer".into(),
                task: "检查 UTF-8 🧪".into(),
                outcome: "completed".into(),
            },
        ]);
        assert!(TrajectoryReplay::new(run).contract_violations().is_empty());
    }

    #[test]
    fn contract_reports_orphan_duplicate_unfinished_and_regressed_events() {
        let run = make_run(vec![
            RunEvent::PhaseTransition {
                phase: "act".into(),
                iteration: 2,
            },
            RunEvent::PhaseTransition {
                phase: "think".into(),
                iteration: 1,
            },
            RunEvent::ToolResult {
                call_id: "orphan".into(),
                name: "write_file".into(),
                success: true,
                output_preview: None,
                output_truncated: false,
                duration_ms: 0,
                original_bytes: 0,
                returned_bytes: 0,
                estimated_tokens: 0,
                output_handling: None,
                artifact: None,
            },
            RunEvent::ToolCall {
                call_id: "pending".into(),
                name: "shell".into(),
                args: None,
                risk: None,
                duration_ms: 0,
            },
            RunEvent::SubAgentRun {
                agent_name: "reviewer".into(),
                task: "review".into(),
                outcome: "unknown".into(),
            },
        ]);
        let violations = TrajectoryReplay::new(run).contract_violations();
        assert!(violations.iter().any(|value| value.contains("regressed")));
        assert!(violations.iter().any(|value| value.contains("orphan")));
        assert!(
            violations
                .iter()
                .any(|value| value.contains("without completion"))
        );
        assert!(
            violations
                .iter()
                .any(|value| value.contains("subagent outcome"))
        );
    }
}
