//! Trajectory replay — offline analysis of past agent runs.
//!
//! Given a [`Run`](crate::trace::Run) loaded from a [`RunStore`](crate::trace::RunStore),
//! the replay system extracts patterns, detects policy violations, and
//! generates eval metrics without re-running the agent.

use crate::eval::{EvalConstraints, EvalMetric, EvalResult};
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
            .filter(|e| {
                matches!(
                    e,
                    RunEvent::ToolError { .. } | RunEvent::Error { .. }
                )
            })
            .count()
    }

    /// Normalize a path for comparison (resolve ../, ./, //).
    fn normalize_path(path: &str) -> String {
        let p = path.replace('\\', "/").replace("//", "/");
        let parts: Vec<&str> = p.split('/').collect();
        let mut out: Vec<&str> = Vec::new();
        for part in parts {
            match part {
                "." | "" => {}
                ".." => { out.pop(); }
                _ => out.push(part),
            }
        }
        out.join("/")
    }

    /// Extract file paths that were written to, using ToolCall args.
    pub fn written_files(&self) -> Vec<String> {
        let mut files = Vec::new();
        for event in &self.run.events {
            if let RunEvent::ToolCall { name, args, .. } = event {
                if is_write_tool(name) || is_read_tool(name) {
                    if let Some(args) = args {
                        if let Some(path) = args.get("path").or_else(|| args.get("file_path")) {
                            if let Some(s) = path.as_str() {
                                files.push(Self::normalize_path(s));
                            }
                        }
                    }
                }
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
                    if let Some(args) = args {
                        if let Some(path) = args.get("path").or_else(|| args.get("file_path")) {
                            if let Some(s) = path.as_str() {
                                read_files.insert(Self::normalize_path(s));
                            }
                        }
                    }
                }
                RunEvent::ToolResult { name, success, .. } if is_read_tool(name) && *success => {
                    // Read confirmed successful — path already tracked from ToolCall
                }
                RunEvent::ToolCall { name, args, .. } if is_write_tool(name) => {
                    if let Some(args) = args {
                        if let Some(path) = args.get("path").or_else(|| args.get("file_path")) {
                            if let Some(s) = path.as_str() {
                                let normalized = Self::normalize_path(s);
                                if !read_files.contains(&normalized) {
                                    violations.push(format!(
                                        "Write without read: {name} on {normalized}"
                                    ));
                                }
                            }
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
        let mut violations = Vec::new();

        // Check max tool calls
        if let Some(max) = constraints.max_tool_calls {
            let total = self.total_tool_calls();
            if total > max {
                violations.push(format!(
                    "Too many tool calls: {total} (max {max})"
                ));
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
            file_changes: self.written_files().len(),
            compile_ok: None,
            tests_pass: None,
        }
    }
}

const WRITE_TOOLS: &[&str] = &[
    "edit_file", "write_file", "append_file", "create_file",
    "delete_file", "update_file", "move_file",
];

const READ_TOOLS: &[&str] = &["read_file", "read_text"];

fn is_write_tool(name: &str) -> bool {
    WRITE_TOOLS.contains(&name)
}

fn is_read_tool(name: &str) -> bool {
    READ_TOOLS.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace::{RunStatus, TokenUsage, RunTimings};
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
            RunEvent::ToolCall { call_id: "test_call".into(), name: "read_file".into(), args: None, risk: None, duration_ms: 10 },
            RunEvent::ToolResult { call_id: "test_call".into(), name: "read_file".into(), success: true, output_preview: None, output_truncated: false, duration_ms: 0 },
            RunEvent::ToolCall { call_id: "test_call".into(), name: "edit_file".into(), args: None, risk: None, duration_ms: 20 },
            RunEvent::ToolResult { call_id: "test_call".into(), name: "edit_file".into(), success: true, output_preview: None, output_truncated: false, duration_ms: 0 },
            RunEvent::ToolCall { call_id: "test_call".into(), name: "read_file".into(), args: None, risk: None, duration_ms: 5 },
            RunEvent::ToolResult { call_id: "test_call".into(), name: "read_file".into(), success: true, output_preview: None, output_truncated: false, duration_ms: 0 },
        ]);
        let replay = TrajectoryReplay::new(run);
        let counts = replay.tool_call_counts();
        assert_eq!(counts.len(), 2);
        assert_eq!(replay.total_tool_calls(), 3);
    }

    #[test]
    fn test_constraint_max_tool_calls() {
        let run = make_run(vec![
            RunEvent::ToolCall { call_id: "test_call".into(), name: "read_file".into(), args: None, risk: None, duration_ms: 10 },
            RunEvent::ToolResult { call_id: "test_call".into(), name: "read_file".into(), success: true, output_preview: None, output_truncated: false, duration_ms: 0 },
            RunEvent::ToolCall { call_id: "test_call".into(), name: "edit_file".into(), args: None, risk: None, duration_ms: 20 },
            RunEvent::ToolResult { call_id: "test_call".into(), name: "edit_file".into(), success: true, output_preview: None, output_truncated: false, duration_ms: 0 },
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
            RunEvent::ToolCall { call_id: "test_call".into(), name: "read_file".into(), args: None, risk: None, duration_ms: 10 },
            RunEvent::ToolError { call_id: "test_call".into(), name: "read_file".into(), message: "not found".into() },
        ]);
        let replay = TrajectoryReplay::new(run);
        assert_eq!(replay.error_count(), 1);
    }
}
