//! Self-improvement loop — evaluate → detect failures → improve → re-evaluate.
//!
//! Inspired by the skill-creator pattern: runs eval cases, detects failure
//! patterns using the Analyzer, generates improvement suggestions, applies
//! them, and re-tests to measure improvement.

use crate::eval::{EvalCase, EvalReport, EvalRunner};
use crate::improve::{Analyzer, ImprovementSuggestion, RunCritique};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Instant;

/// Result of one improvement iteration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopIteration {
    pub iteration: usize,
    pub eval_report: EvalReport,
    pub critiques: Vec<RunCritique>,
    pub suggestions: Vec<ImprovementSuggestion>,
    pub duration_ms: u64,
}

/// Full improvement loop history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopResult {
    pub iterations: Vec<LoopIteration>,
    pub best_score: f64,
    pub best_iteration: usize,
    pub total_duration_ms: u64,
}

/// Analysis loop — evaluates, detects failures, and suggests improvements.
///
/// NOTE: This loop analyzes failures and generates suggestions for human review.
/// It does NOT automatically apply suggestions to the agent. To apply suggestions,
/// use [`PromptGenerator`](crate::improve::PromptGenerator) to generate an updated
/// system prompt and pass it to a new agent via the factory.
pub struct ImprovementLoop {
    pub max_iterations: usize,
    pub improvement_threshold: f64,
    pub holdout_ratio: f64,
}

impl Default for ImprovementLoop {
    fn default() -> Self {
        Self { max_iterations: 5, improvement_threshold: 0.95, holdout_ratio: 0.4 }
    }
}

impl ImprovementLoop {
    pub fn new() -> Self { Self::default() }

    /// Run the analysis loop: eval → critique → suggest → re-eval.
    /// Returns analysis results. Suggestions must be applied externally.
    pub async fn run(
        &self,
        cases: &[EvalCase],
        agent_factory: impl Fn() -> Box<dyn crate::agent::Agent>,
        run_store: &Option<Arc<dyn crate::trace::RunStore>>,
    ) -> LoopResult {
        let started = Instant::now();
        if cases.is_empty() { return LoopResult { iterations: vec![], best_score: 0.0, best_iteration: 0, total_duration_ms: 0 }; }
        let mut iterations = Vec::new();
        let mut best_score = 0.0;
        let mut best_iteration = 0;

        // Split cases
        let split_idx = ((1.0 - self.holdout_ratio) * cases.len() as f64) as usize;
        let train_cases = &cases[..split_idx.max(1)];
        let test_cases = &cases[split_idx.min(cases.len())..];

        for i in 0..self.max_iterations {
            let iter_start = Instant::now();

            // a. Evaluate on train set
            let runner = EvalRunner::new(std::env::temp_dir().join(format!("improve_{i}")));
            let train_report = runner
                .run_all(train_cases, || agent_factory())
                .await;

            // b. Analyze failures — load runs and critique
            let mut critiques = Vec::new();
            if let Some(store) = run_store {
                for result in &train_report.results {
                    if !result.success {
                        if let Some(ref run_id) = result.run_id {
                            if let Ok(Some(run)) = store.load(run_id).await {
                                critiques.push(Analyzer::analyze(&run));
                            }
                        }
                    }
                }
            }

            // c. Generate suggestions from critiques
            let mut suggestions = Vec::new();
            for c in &critiques {
                suggestions.extend(c.suggestions.clone());
            }
            // Sort and deduplicate (dedup only removes consecutive, so sort first)
            suggestions.sort_by_key(|s| format!("{:?}", s));
            suggestions.dedup_by_key(|s| format!("{:?}", s));

            // d. Re-evaluate on test set
            let test_report = runner
                .run_all(test_cases, || agent_factory())
                .await;

            // e. Track best
            if test_report.avg_score > best_score {
                best_score = test_report.avg_score;
                best_iteration = i;
            }

            let iter = LoopIteration {
                iteration: i,
                eval_report: test_report,
                critiques,
                suggestions: suggestions.clone(),
                duration_ms: iter_start.elapsed().as_millis() as u64,
            };
            iterations.push(iter);

            // Stop early if threshold reached
            if best_score >= self.improvement_threshold {
                break;
            }

            // Clean up
            let _ = std::fs::remove_dir_all(runner.workspace_root);
        }

        LoopResult {
            iterations,
            best_score,
            best_iteration,
            total_duration_ms: started.elapsed().as_millis() as u64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_loop_defaults() {
        let lp = ImprovementLoop::new();
        assert_eq!(lp.max_iterations, 5);
        assert_eq!(lp.improvement_threshold, 0.95);
    }
}
