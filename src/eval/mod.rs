//! Agent evaluation framework — measurable, repeatable agent testing.
//!
//! # Overview
//!
//! The eval system lets you define test cases, run them against an agent,
//! and score the results. It builds on the trace system ([`crate::trace`])
//! to link eval results to execution traces for debugging and regression.
//!
//! # Quick Start
//!
//! ```rust,no_run
//! use echo_agent::eval::{EvalCase, EvalRunner, SuccessCriteria};
//!
//! let case = EvalCase {
//!     id: "test_001".into(),
//!     name: "Basic tool use".into(),
//!     description: "Agent should use read_file when asked to read".into(),
//!     task: "Read the file src/main.rs".into(),
//!     project_fixture: None,
//!     success_criteria: SuccessCriteria::ToolUsed {
//!         tool_name: "read_file".into(),
//!     },
//!     constraints: Default::default(),
//! };
//!
//! // let runner = EvalRunner::new(temp_dir);
//! // let result = runner.run(&case, &agent).await;
//! ```

pub mod comparator;
pub mod grader;
pub mod regression;
pub mod replay;
pub mod report;
pub mod runner;
pub mod server;
pub mod trigger;

pub use comparator::AbComparator;
pub use grader::LlmGrader;
pub use regression::RegressionSuite;
pub use replay::TrajectoryReplay;
pub use report::generate_html;
pub use runner::EvalRunner;
pub use server::generate_review_html;
pub use trigger::TriggerAccuracy;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ── EvalCase ─────────────────────────────────────────────────────────

/// A single eval test case — what to test and how to judge success.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalCase {
    /// Unique identifier for this case (e.g. "read_before_edit_001").
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Description of what this case tests.
    #[serde(default)]
    pub description: String,
    /// The task/question to give the agent.
    pub task: String,
    /// Optional project fixture directory to set up before the test.
    /// Files are copied to the eval workspace before the agent runs.
    #[serde(default)]
    pub project_fixture: Option<PathBuf>,
    /// Criteria that determine pass/fail.
    pub success_criteria: SuccessCriteria,
    /// Constraints the agent must respect.
    #[serde(default)]
    pub constraints: EvalConstraints,
}

// ── SuccessCriteria ──────────────────────────────────────────────────

/// How to determine if an eval case passed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SuccessCriteria {
    /// A test command exits successfully (e.g., `cargo test`).
    TestPass {
        command: String,
    },
    /// The agent's final output contains a specific substring.
    OutputContains {
        substring: String,
    },
    /// The agent used a specific tool at least once.
    ToolUsed {
        tool_name: String,
    },
    /// The agent did NOT use a specific tool.
    ToolNotUsed {
        tool_name: String,
    },
    /// All of the given criteria must pass.
    AllOf(Vec<SuccessCriteria>),
    /// At least one of the given criteria must pass.
    AnyOf(Vec<SuccessCriteria>),
    /// LLM-as-Judge: assertions checked by an LLM grader agent.
    LlmGraded {
        /// Assertions for the grader to check.
        assertions: Vec<crate::eval::grader::Assertion>,
    },
    /// SWE-bench style: clone repo, check out commit, run agent, apply test patch, run tests.
    SweBench {
        /// Git repository URL to clone.
        repo_url: String,
        /// Base commit to check out before the agent runs.
        base_commit: String,
        /// Patch file to apply after the agent completes (adds tests).
        test_patch: String,
        /// Command to run after applying the test patch.
        test_command: String,
    },
}

// ── EvalConstraints ──────────────────────────────────────────────────

/// Guardrails the agent must respect during the eval.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EvalConstraints {
    /// Maximum number of files the agent is allowed to change.
    #[serde(default)]
    pub max_files_changed: Option<usize>,
    /// Maximum number of tool calls allowed.
    #[serde(default)]
    pub max_tool_calls: Option<usize>,
    /// Paths the agent must not modify.
    #[serde(default)]
    pub forbidden_paths: Vec<String>,
    /// Whether the agent must read a file before editing it.
    #[serde(default)]
    pub required_read_before_edit: bool,
}

// ── EvalMetric ───────────────────────────────────────────────────────

/// A single scored dimension in an eval result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalMetric {
    /// Metric name (e.g. "tool_accuracy", "test_pass", "constraint_compliance").
    pub name: String,
    /// Score from 0.0 (worst) to 1.0 (best).
    pub score: f64,
    /// Human-readable detail.
    pub detail: String,
}

// ── EvalResult ───────────────────────────────────────────────────────

/// Outcome of running a single eval case.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalResult {
    /// The case ID this result corresponds to.
    pub case_id: String,
    /// Whether the success criteria were met.
    pub success: bool,
    /// Aggregate score (average of all metrics).
    pub score: f64,
    /// Per-dimension scores.
    pub metrics: Vec<EvalMetric>,
    /// Constraint violations found.
    pub violations: Vec<String>,
    /// Link to the execution trace (run ID).
    pub run_id: Option<String>,
    /// Wall-clock duration in milliseconds.
    pub duration_ms: u64,
    /// Total tool calls made by the agent.
    #[serde(default)]
    pub tool_calls: usize,
    /// Estimated input tokens.
    #[serde(default)]
    pub tokens_in: u32,
    /// Estimated output tokens.
    #[serde(default)]
    pub tokens_out: u32,
    /// Number of files changed.
    #[serde(default)]
    pub file_changes: usize,
    /// Whether compilation succeeded (if applicable).
    #[serde(default)]
    pub compile_ok: Option<bool>,
    /// Whether tests passed (if applicable).
    #[serde(default)]
    pub tests_pass: Option<bool>,
}

impl EvalResult {
    pub fn new(case_id: &str, success: bool) -> Self {
        Self {
            case_id: case_id.to_string(),
            success,
            score: if success { 1.0 } else { 0.0 },
            metrics: Vec::new(),
            violations: Vec::new(),
            run_id: None,
            duration_ms: 0,
            tool_calls: 0,
            tokens_in: 0,
            tokens_out: 0,
            file_changes: 0,
            compile_ok: None,
            tests_pass: None,
        }
    }

    pub fn with_metric(mut self, name: &str, score: f64, detail: &str) -> Self {
        self.metrics.push(EvalMetric {
            name: name.to_string(),
            score,
            detail: detail.to_string(),
        });
        self.recompute_score();
        self
    }

    pub fn with_violation(mut self, violation: &str) -> Self {
        self.violations.push(violation.to_string());
        self.success = false;
        self
    }

    pub fn with_run_id(mut self, run_id: &str) -> Self {
        self.run_id = Some(run_id.to_string());
        self
    }

    fn recompute_score(&mut self) {
        if self.metrics.is_empty() {
            self.score = if self.success { 1.0 } else { 0.0 };
        } else {
            self.score = self.metrics.iter().map(|m| m.score).sum::<f64>()
                / self.metrics.len() as f64;
        }
    }
}

// ── EvalReport ───────────────────────────────────────────────────────

/// Aggregate report across multiple eval cases.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalReport {
    /// Total number of cases.
    pub total: usize,
    /// Number that passed.
    pub passed: usize,
    /// Number that failed.
    pub failed: usize,
    /// Average score across all cases.
    pub avg_score: f64,
    /// Standard deviation of scores.
    #[serde(default)]
    pub std_dev: f64,
    /// Minimum score.
    #[serde(default)]
    pub min_score: f64,
    /// Maximum score.
    #[serde(default)]
    pub max_score: f64,
    /// Score delta from baseline (if comparing two runs).
    #[serde(default)]
    pub delta_vs_baseline: Option<f64>,
    /// Total tool calls across all cases.
    #[serde(default)]
    pub total_tool_calls: usize,
    /// Total tokens in across all cases.
    #[serde(default)]
    pub total_tokens_in: u32,
    /// Total tokens out across all cases.
    #[serde(default)]
    pub total_tokens_out: u32,
    /// Total files changed across all cases.
    #[serde(default)]
    pub total_file_changes: usize,
    /// Per-case results.
    pub results: Vec<EvalResult>,
}

impl EvalReport {
    pub fn new(results: Vec<EvalResult>) -> Self {
        let total = results.len();
        let passed = results.iter().filter(|r| r.success).count();
        let failed = total - passed;
        let avg_score = if total > 0 {
            results.iter().map(|r| r.score).sum::<f64>() / total as f64
        } else {
            0.0
        };
        let total_tool_calls: usize = results.iter().map(|r| r.tool_calls).sum();
        let total_tokens_in: u32 = results.iter().map(|r| r.tokens_in).sum();
        let total_tokens_out: u32 = results.iter().map(|r| r.tokens_out).sum();
        let total_file_changes: usize = results.iter().map(|r| r.file_changes).sum();
        // Statistical aggregation
        let scores: Vec<f64> = results.iter().map(|r| r.score).collect();
        let variance = if total > 1 {
            scores.iter().map(|s| (s - avg_score).powi(2)).sum::<f64>() / (total - 1) as f64
        } else { 0.0 };
        let std_dev = variance.sqrt();
        let min_score = scores.iter().cloned().fold(f64::MAX, f64::min);
        let max_score = scores.iter().cloned().fold(f64::MIN, f64::max);
        Self {
            total,
            passed,
            failed,
            avg_score,
            std_dev,
            min_score: if total > 0 { min_score } else { 0.0 },
            max_score: if total > 0 { max_score } else { 0.0 },
            delta_vs_baseline: None,
            total_tool_calls,
            total_tokens_in,
            total_tokens_out,
            total_file_changes,
            results,
        }
    }
}
