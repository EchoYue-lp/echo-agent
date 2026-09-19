//! Pattern detection and improvement suggestion generation from run traces.

use crate::eval::{EvalCase, TrajectoryReplay};
use crate::improve::{CritiqueIssue, ImprovementSuggestion, RunCritique};
use crate::trace::{Run, RunEvent, RunStatus};
use std::collections::HashMap;

/// Analyzes completed runs to detect patterns and generate suggestions.
pub struct Analyzer;

impl Analyzer {
    /// Analyze a single run and produce a critique.
    pub fn analyze(run: &Run) -> RunCritique {
        let success = matches!(run.status, RunStatus::Completed);
        let mut critique = RunCritique::new(&run.run_id, success);

        // Detect issues
        critique.issues.extend(Self::detect_write_without_read(run));
        critique.issues.extend(Self::detect_excessive_retries(run));
        critique
            .issues
            .extend(Self::detect_excessive_tool_calls(run));

        // Generate suggestions from issues
        critique.suggestions = Self::generate_suggestions(&critique.issues, run);

        // Compute score
        let error_count = critique.issues.len();
        let total_events = run.events.len().max(1);
        critique.score = 1.0 - (error_count as f64 / total_events as f64).min(1.0);

        critique
    }

    /// Analyze multiple runs and detect cross-run patterns.
    pub fn analyze_batch(runs: &[Run]) -> Vec<RunCritique> {
        runs.iter().map(Self::analyze).collect()
    }

    // ── Issue detectors ────────────────────────────────────────────

    /// Detect confirmed file edits without a preceding successful read of that path.
    fn detect_write_without_read(run: &Run) -> Vec<CritiqueIssue> {
        let writes_without_read = TrajectoryReplay::new(run.clone())
            .detect_write_without_read()
            .len();

        if writes_without_read > 0 {
            vec![CritiqueIssue::WriteWithoutRead {
                tool: "write".into(),
                count: writes_without_read,
            }]
        } else {
            Vec::new()
        }
    }

    /// Detect tools that were called, failed, and retried multiple times.
    fn detect_excessive_retries(run: &Run) -> Vec<CritiqueIssue> {
        let mut tool_errors: HashMap<&str, usize> = HashMap::new();
        let skipped = crate::trace::skipped_tool_call_ids(&run.events);
        for event in &run.events {
            if let RunEvent::ToolError { call_id, name, .. } = event
                && !skipped.contains(call_id.as_str())
            {
                *tool_errors.entry(name.as_str()).or_default() += 1;
            }
        }
        tool_errors
            .into_iter()
            .filter(|(_, count)| *count > 2)
            .map(|(tool, count)| CritiqueIssue::ExcessiveRetries {
                tool: tool.to_string(),
                count,
            })
            .collect()
    }

    /// Detect runs with an unusually high number of tool calls.
    fn detect_excessive_tool_calls(run: &Run) -> Vec<CritiqueIssue> {
        let skipped = crate::trace::skipped_tool_call_ids(&run.events);
        let total = run
            .events
            .iter()
            .filter(|event| {
                matches!(event, RunEvent::ToolCall { call_id, .. } if !skipped.contains(call_id.as_str()))
            })
            .count();
        if total > 20 {
            vec![CritiqueIssue::ExcessiveToolCalls { total }]
        } else {
            Vec::new()
        }
    }

    // ── Suggestion generators ──────────────────────────────────────

    /// Generate improvement suggestions from detected issues.
    fn generate_suggestions(issues: &[CritiqueIssue], _run: &Run) -> Vec<ImprovementSuggestion> {
        let mut suggestions = Vec::new();

        for issue in issues {
            match issue {
                CritiqueIssue::WriteWithoutRead { count, .. } => {
                    suggestions.push(ImprovementSuggestion::PromptChange {
                        section: "tools".into(),
                        suggestion: format!(
                            "Add instruction: 'Always read a file with read_file before editing it. \
                             This was violated {count} time(s).'"
                        ),
                    });
                    suggestions.push(ImprovementSuggestion::PolicyChange {
                        rule: "force_read_before_edit: true".into(),
                        reason: format!("Agent wrote files without reading first {count} time(s)"),
                    });
                }
                CritiqueIssue::ExcessiveRetries { tool, count } => {
                    suggestions.push(ImprovementSuggestion::PromptChange {
                        section: "error_handling".into(),
                        suggestion: format!(
                            "Add instruction: 'If {tool} fails, try a different approach \
                             instead of retrying. Failed {count} time(s).'"
                        ),
                    });
                }
                CritiqueIssue::ExcessiveToolCalls { total } => {
                    suggestions.push(ImprovementSuggestion::PromptChange {
                        section: "efficiency".into(),
                        suggestion: format!(
                            "Add instruction: 'Aim to complete tasks in fewer tool calls. \
                             Previous run used {total} calls.'"
                        ),
                    });
                }
                CritiqueIssue::ToolErrorPattern { tool, message } => {
                    suggestions.push(ImprovementSuggestion::EvalGeneration {
                        case_id: format!("eval_{}", tool),
                        json: serde_json::to_string(&EvalCase {
                            id: format!("eval_{tool}"),
                            name: format!("Tool error: {tool}"),
                            description: message.clone(),
                            domain: None,
                            task: format!("Use {tool} correctly"),
                            project_fixture: None,
                            success_criteria: crate::eval::SuccessCriteria::ToolUsed {
                                tool_name: tool.clone(),
                            },
                            constraints: Default::default(),
                        })
                        .unwrap_or_default(),
                    });
                }
                _ => {}
            }
        }

        suggestions
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace::{RunStatus, RunTimings, TokenUsage};
    use chrono::Utc;

    fn make_run(events: Vec<RunEvent>, status: RunStatus) -> Run {
        Run {
            run_id: "test".into(),
            parent_run_id: None,
            agent_name: String::new(),
            model: String::new(),
            provider: None,
            turn_id: None,
            execution_id: None,
            session_id: "s1".into(),
            status,
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
    fn test_detect_write_without_read() {
        let run = make_run(
            vec![
                RunEvent::ToolCall {
                    call_id: "test_call".into(),
                    name: "write_file".into(),
                    args: None,
                    risk: None,
                    duration_ms: 10,
                },
                RunEvent::FileEdit {
                    tool: "write_file".into(),
                    path: "src/lib.rs".into(),
                },
                RunEvent::ToolResult {
                    call_id: "test_call".into(),
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
            ],
            RunStatus::Completed,
        );
        let critique = Analyzer::analyze(&run);
        assert!(!critique.issues.is_empty());
        assert!(
            critique
                .suggestions
                .iter()
                .any(|s| matches!(s, ImprovementSuggestion::PromptChange { .. }))
        );
        assert!(
            critique
                .suggestions
                .iter()
                .any(|s| matches!(s, ImprovementSuggestion::PolicyChange { .. }))
        );
    }

    #[test]
    fn failed_or_different_path_reads_do_not_suppress_confirmed_edit_critique() {
        let run = make_run(
            vec![
                RunEvent::ToolCall {
                    call_id: "failed-read".into(),
                    name: "read_file".into(),
                    args: Some(serde_json::json!({"path": "src/lib.rs"})),
                    risk: None,
                    duration_ms: 1,
                },
                RunEvent::ToolError {
                    call_id: "failed-read".into(),
                    name: "read_file".into(),
                    message: "not found".into(),
                    failure: None,
                },
                RunEvent::FileRead {
                    tool: "read_file".into(),
                    path: "src/other.rs".into(),
                },
                RunEvent::FileEdit {
                    tool: "write_file".into(),
                    path: "src/lib.rs".into(),
                },
            ],
            RunStatus::Completed,
        );

        assert!(
            Analyzer::analyze(&run)
                .issues
                .iter()
                .any(|issue| { matches!(issue, CritiqueIssue::WriteWithoutRead { count: 1, .. }) })
        );
    }

    #[test]
    fn skipped_invocations_do_not_drive_execution_critiques() {
        let mut events = Vec::new();
        for index in 0..21 {
            let call_id = format!("skipped-{index}");
            events.push(RunEvent::ToolCall {
                call_id: call_id.clone(),
                name: "write_file".into(),
                args: None,
                risk: None,
                duration_ms: 0,
            });
            events.push(RunEvent::ToolExecutionSkipped {
                call_id: call_id.clone(),
                name: "write_file".into(),
                reason: "cancelled before execution".into(),
            });
            events.push(RunEvent::ToolError {
                call_id,
                name: "write_file".into(),
                message: "cancelled before execution".into(),
                failure: None,
            });
        }

        let critique = Analyzer::analyze(&make_run(events, RunStatus::Failed));
        assert!(!critique.issues.iter().any(|issue| matches!(
            issue,
            CritiqueIssue::WriteWithoutRead { .. }
                | CritiqueIssue::ExcessiveRetries { .. }
                | CritiqueIssue::ExcessiveToolCalls { .. }
        )));
    }

    #[test]
    fn test_detect_excessive_retries() {
        let run = make_run(
            vec![
                RunEvent::ToolCall {
                    call_id: "test_call".into(),
                    name: "shell".into(),
                    args: None,
                    risk: None,
                    duration_ms: 10,
                },
                RunEvent::ToolError {
                    call_id: "test_call".into(),
                    name: "shell".into(),
                    message: "fail1".into(),
                    failure: None,
                },
                RunEvent::ToolCall {
                    call_id: "test_call".into(),
                    name: "shell".into(),
                    args: None,
                    risk: None,
                    duration_ms: 10,
                },
                RunEvent::ToolError {
                    call_id: "test_call".into(),
                    name: "shell".into(),
                    message: "fail2".into(),
                    failure: None,
                },
                RunEvent::ToolCall {
                    call_id: "test_call".into(),
                    name: "shell".into(),
                    args: None,
                    risk: None,
                    duration_ms: 10,
                },
                RunEvent::ToolError {
                    call_id: "test_call".into(),
                    name: "shell".into(),
                    message: "fail3".into(),
                    failure: None,
                },
            ],
            RunStatus::Failed,
        );
        let critique = Analyzer::analyze(&run);
        assert!(
            critique
                .issues
                .iter()
                .any(|i| matches!(i, CritiqueIssue::ExcessiveRetries { .. }))
        );
    }

    #[test]
    fn test_clean_run() {
        let run = make_run(
            vec![
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
            ],
            RunStatus::Completed,
        );
        let critique = Analyzer::analyze(&run);
        assert!(critique.issues.is_empty());
    }
}
