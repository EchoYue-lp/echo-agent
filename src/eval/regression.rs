//! Regression suite — automated regression testing from trace history.
//!
//! Builds eval cases from past runs and re-runs them to detect regressions.

use crate::eval::{EvalCase, EvalReport, EvalRunner, SuccessCriteria};
use crate::trace::{Run, RunEvent, RunStatus};
use std::collections::HashSet;

/// A suite of eval cases built from historical trace data.
pub struct RegressionSuite {
    pub name: String,
    pub cases: Vec<EvalCase>,
}

impl RegressionSuite {
    /// Create an empty suite.
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            cases: Vec::new(),
        }
    }

    /// Build a regression suite from past run traces.
    ///
    /// Each successful run becomes an eval case that checks for the
    /// same tool usage pattern and output characteristics.
    pub fn from_traces(runs: &[Run]) -> Self {
        let mut suite = Self::new("regression_from_traces");
        let mut seen_tasks: HashSet<String> = HashSet::new();

        for run in runs {
            // Only use completed, successful runs
            if !matches!(run.status, RunStatus::Completed) {
                continue;
            }
            // Skip duplicates
            let task_key = run.input.trim().to_lowercase();
            if !seen_tasks.insert(task_key) {
                continue;
            }

            let case = Self::build_case_from_run(run);
            suite.cases.push(case);
        }

        suite
    }

    /// Build a single eval case from one run.
    fn build_case_from_run(run: &Run) -> EvalCase {
        // Collect tools used in this run
        let tools_used: Vec<&str> = run
            .events
            .iter()
            .filter_map(|e| match e {
                RunEvent::ToolCall { name, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect();

        let tool_count = tools_used.len();
        let success_criteria = if !tools_used.is_empty() {
            SuccessCriteria::ToolUsed {
                tool_name: tools_used[0].to_string(),
            }
        } else {
            SuccessCriteria::OutputContains {
                substring: run.final_output.clone().unwrap_or_default().chars().take(50).collect(),
            }
        };

        EvalCase {
            id: format!("regression_{}", &run.run_id[..12.min(run.run_id.len())]),
            name: format!("Regression: {}", &run.input[..60.min(run.input.len())]),
            description: format!(
                "Auto-generated from run {}. Used {} tool(s).",
                run.run_id, tool_count
            ),
            task: run.input.clone(),
            project_fixture: None,
            success_criteria,
            constraints: Default::default(),
        }
    }

    /// Add a case manually.
    pub fn with_case(mut self, case: EvalCase) -> Self {
        self.cases.push(case);
        self
    }

    /// Run all cases in the suite.
    pub async fn run_all(
        &self,
        runner: &EvalRunner,
        agent: &dyn crate::agent::Agent,
    ) -> EvalReport {
        let mut results = Vec::new();
        for case in &self.cases {
            let result = runner.run(case, agent).await;
            results.push(result);
        }
        EvalReport::new(results)
    }

    /// Number of cases.
    pub fn len(&self) -> usize {
        self.cases.len()
    }

    /// Whether the suite is empty.
    pub fn is_empty(&self) -> bool {
        self.cases.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace::{TokenUsage, RunTimings};
    use chrono::Utc;

    fn make_run(id: &str, input: &str, events: Vec<RunEvent>, status: RunStatus) -> Run {
        Run {
            run_id: id.to_string(),
            parent_run_id: None,
            session_id: "s1".into(),
            status,
            input: input.to_string(),
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
    fn test_from_traces() {
        let runs = vec![
            make_run(
                "run1",
                "read src/main.rs",
                vec![
                    RunEvent::ToolCall { call_id: "test_call".into(), name: "read_file".into(), args: None, risk: None, duration_ms: 10 },
                    RunEvent::ToolResult { call_id: "test_call".into(), name: "read_file".into(), success: true, output_preview: None, output_truncated: false, duration_ms: 0 },
                ],
                RunStatus::Completed,
            ),
        ];
        let suite = RegressionSuite::from_traces(&runs);
        assert_eq!(suite.len(), 1);
        assert!(suite.cases[0].id.contains("run1"));
    }

    #[test]
    fn test_from_traces_skips_failed() {
        let runs = vec![
            make_run("run2", "fail task", vec![], RunStatus::Failed),
        ];
        let suite = RegressionSuite::from_traces(&runs);
        assert!(suite.is_empty());
    }

    #[test]
    fn test_from_traces_dedup() {
        let runs = vec![
            make_run("r1", "read file", vec![
                RunEvent::ToolCall { call_id: "test_call".into(), name: "read_file".into(), args: None, risk: None, duration_ms: 10 },
                RunEvent::ToolResult { call_id: "test_call".into(), name: "read_file".into(), success: true, output_preview: None, output_truncated: false, duration_ms: 0 },
            ], RunStatus::Completed),
            make_run("r2", "read file", vec![
                RunEvent::ToolCall { call_id: "test_call".into(), name: "read_file".into(), args: None, risk: None, duration_ms: 10 },
                RunEvent::ToolResult { call_id: "test_call".into(), name: "read_file".into(), success: true, output_preview: None, output_truncated: false, duration_ms: 0 },
            ], RunStatus::Completed),
        ];
        let suite = RegressionSuite::from_traces(&runs);
        assert_eq!(suite.len(), 1); // deduped
    }
}
