//! Self-improvement loop — evaluate → detect failures → improve → re-evaluate.
//!
//! Inspired by the skill-creator pattern: runs eval cases, detects failure
//! patterns using the Analyzer, generates improvement suggestions, applies
//! them, and re-tests to measure improvement.

use crate::eval::{EvalCase, EvalReport, EvalRunner, SuccessCriteria};
use crate::improve::{Analyzer, ImprovementSuggestion, RunCritique};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

/// Result of one improvement iteration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopIteration {
    pub iteration: usize,
    pub eval_report: EvalReport,
    pub train_score: f64,
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
#[derive(Clone)]
pub struct ImprovementLoop {
    pub max_iterations: usize,
    pub improvement_threshold: f64,
    pub holdout_ratio: f64,
}

impl Default for ImprovementLoop {
    fn default() -> Self {
        Self {
            max_iterations: 5,
            improvement_threshold: 0.95,
            holdout_ratio: 0.4,
        }
    }
}

impl ImprovementLoop {
    pub fn new() -> Self {
        Self::default()
    }

    /// Return the variant name of a SuccessCriteria for stratification.
    fn criteria_variant(criteria: &SuccessCriteria) -> &str {
        match criteria {
            SuccessCriteria::TestPass { .. } => "test_pass",
            SuccessCriteria::OutputContains { .. } => "output_contains",
            SuccessCriteria::ToolUsed { .. } => "tool_used",
            SuccessCriteria::ToolNotUsed { .. } => "tool_not_used",
            SuccessCriteria::AllOf(_) => "all_of",
            SuccessCriteria::AnyOf(_) => "any_of",
            SuccessCriteria::LlmGraded { .. } => "llm_graded",
            SuccessCriteria::SweBench { .. } => "swe_bench",
            SuccessCriteria::SafetyCheck { .. } => "safety_check",
            SuccessCriteria::CitationValid { .. } => "citation_valid",
            SuccessCriteria::ValueMatch { .. } => "value_match",
        }
    }

    /// Stratified split: group cases by criteria type, split each group proportionally.
    fn stratified_split(
        cases: &[EvalCase],
        holdout_ratio: f64,
    ) -> (Vec<&EvalCase>, Vec<&EvalCase>) {
        let mut groups: HashMap<String, Vec<&EvalCase>> = HashMap::new();
        for case in cases {
            let key = Self::criteria_variant(&case.success_criteria).to_string();
            groups.entry(key).or_default().push(case);
        }

        let mut train = Vec::new();
        let mut test = Vec::new();

        for (_, group) in groups {
            let split_idx = if group.len() == 1 {
                // A singleton cannot form an independent holdout without
                // leaking its training sample into evaluation.
                1
            } else {
                let proportional = ((1.0 - holdout_ratio) * group.len() as f64) as usize;
                proportional.clamp(1, group.len().saturating_sub(1))
            };
            for (index, case) in group.into_iter().enumerate() {
                if index < split_idx {
                    train.push(case);
                } else {
                    test.push(case);
                }
            }
        }

        (train, test)
    }

    /// Run the analysis loop: eval → critique → suggest → re-eval.
    /// Returns analysis results. Suggestions must be applied externally.
    pub async fn run(
        &self,
        cases: &[EvalCase],
        agent_factory: impl Fn() -> Box<dyn crate::agent::Agent>,
        run_store: &Option<Arc<dyn crate::trace::RunStore>>,
    ) -> LoopResult {
        self.run_async(cases, || std::future::ready(agent_factory()), run_store)
            .await
    }

    /// Run with a lazily invoked asynchronous Agent factory. Remote adapters
    /// use this entry so early-stop does not trigger speculative construction.
    pub async fn run_async<F, Fut>(
        &self,
        cases: &[EvalCase],
        agent_factory: F,
        run_store: &Option<Arc<dyn crate::trace::RunStore>>,
    ) -> LoopResult
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Box<dyn crate::agent::Agent>>,
    {
        let started = Instant::now();
        if cases.is_empty() {
            return LoopResult {
                iterations: vec![],
                best_score: 0.0,
                best_iteration: 0,
                total_duration_ms: 0,
            };
        }
        let mut iterations = Vec::new();
        let mut best_score = 0.0;
        let mut best_iteration = 0;

        // Stratified split by criteria type to prevent overfitting
        let (train_cases, test_cases) = Self::stratified_split(cases, self.holdout_ratio);
        let runner = EvalRunner::new(std::env::temp_dir());

        for i in 0..self.max_iterations {
            let iter_start = Instant::now();

            // a. Evaluate on train set
            let train_cases_vec: Vec<EvalCase> = train_cases.iter().map(|c| (*c).clone()).collect();
            let train_report = runner.run_all_async(&train_cases_vec, &agent_factory).await;

            // b. Analyze failures — load runs and critique
            let mut critiques = Vec::new();
            if let Some(store) = run_store {
                for result in &train_report.results {
                    if !result.success
                        && let Some(ref run_id) = result.run_id
                        && let Ok(Some(run)) = store.load(run_id).await
                    {
                        critiques.push(Analyzer::analyze(&run));
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

            // d. Re-evaluate on test set (blinded — test scores not visible to generator)
            let test_cases_vec: Vec<EvalCase> = test_cases.iter().map(|c| (*c).clone()).collect();
            let test_report = runner.run_all_async(&test_cases_vec, &agent_factory).await;

            // e. Track best by test score
            if test_report.avg_score > best_score {
                best_score = test_report.avg_score;
                best_iteration = i;
            }

            let iter = LoopIteration {
                iteration: i,
                eval_report: test_report,
                train_score: train_report.avg_score,
                critiques,
                suggestions: suggestions.clone(),
                duration_ms: iter_start.elapsed().as_millis() as u64,
            };
            iterations.push(iter);

            // Stop early if threshold reached
            if best_score >= self.improvement_threshold {
                break;
            }
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
    use crate::testing::MockAgent;

    fn eval_case(id: &str, success_criteria: SuccessCriteria) -> EvalCase {
        EvalCase {
            id: id.to_string(),
            name: id.to_string(),
            description: String::new(),
            domain: None,
            task: format!("Evaluate {id}"),
            project_fixture: None,
            success_criteria,
            constraints: Default::default(),
        }
    }

    #[test]
    fn test_loop_defaults() {
        let lp = ImprovementLoop::new();
        assert_eq!(lp.max_iterations, 5);
        assert_eq!(lp.improvement_threshold, 0.95);
    }

    #[test]
    fn singleton_criteria_group_is_train_only() {
        let cases = vec![eval_case(
            "single",
            SuccessCriteria::OutputContains {
                substring: "done".to_string(),
            },
        )];

        let (train, holdout) = ImprovementLoop::stratified_split(&cases, 0.4);

        assert_eq!(train.len(), 1);
        assert_eq!(train.first().map(|case| case.id.as_str()), Some("single"));
        assert!(holdout.is_empty());
    }

    #[test]
    fn mixed_groups_keep_singletons_out_of_holdout() {
        let cases = vec![
            eval_case(
                "single",
                SuccessCriteria::OutputContains {
                    substring: "done".to_string(),
                },
            ),
            eval_case(
                "tool-a",
                SuccessCriteria::ToolUsed {
                    tool_name: "read".to_string(),
                },
            ),
            eval_case(
                "tool-b",
                SuccessCriteria::ToolUsed {
                    tool_name: "write".to_string(),
                },
            ),
        ];

        let (train, holdout) = ImprovementLoop::stratified_split(&cases, 0.5);

        assert_eq!(train.len(), 2);
        assert_eq!(holdout.len(), 1);
        assert!(train.iter().any(|case| case.id == "single"));
        assert!(!holdout.iter().any(|case| case.id == "single"));
        assert_eq!(
            train
                .iter()
                .chain(&holdout)
                .filter(|case| case.id.starts_with("tool-"))
                .count(),
            2
        );
    }

    #[test]
    fn multi_case_groups_remain_split_at_ratio_boundaries() {
        let cases = vec![
            eval_case(
                "case-a",
                SuccessCriteria::OutputContains {
                    substring: "a".to_string(),
                },
            ),
            eval_case(
                "case-b",
                SuccessCriteria::OutputContains {
                    substring: "b".to_string(),
                },
            ),
            eval_case(
                "case-c",
                SuccessCriteria::OutputContains {
                    substring: "c".to_string(),
                },
            ),
        ];

        for ratio in [-1.0, 0.0, 1.0, 2.0] {
            let (train, holdout) = ImprovementLoop::stratified_split(&cases, ratio);
            assert!(!train.is_empty());
            assert!(!holdout.is_empty());
            assert_eq!(train.len().saturating_add(holdout.len()), cases.len());
            for case in &cases {
                let occurrences = train
                    .iter()
                    .chain(&holdout)
                    .filter(|candidate| candidate.id == case.id)
                    .count();
                assert_eq!(occurrences, 1);
            }
        }
    }

    #[tokio::test]
    async fn concurrent_early_stop_loops_use_distinct_cleaned_generations()
    -> std::result::Result<(), String> {
        let cases = vec![eval_case(
            "early-stop",
            SuccessCriteria::OutputContains {
                substring: "done".to_string(),
            },
        )];
        let loop_config = ImprovementLoop {
            max_iterations: 3,
            improvement_threshold: 0.0,
            holdout_ratio: 0.4,
        };
        let first_observer = MockAgent::new("first-loop").with_default_success("done".to_string());
        let first_factory_agent = first_observer.clone();
        let second_observer =
            MockAgent::new("second-loop").with_default_success("done".to_string());
        let second_factory_agent = second_observer.clone();
        let run_store = None;

        let first_loop = loop_config.run_async(
            &cases,
            move || {
                let agent = first_factory_agent.clone();
                std::future::ready(Box::new(agent) as Box<dyn crate::agent::Agent>)
            },
            &run_store,
        );
        let second_loop = loop_config.run_async(
            &cases,
            move || {
                let agent = second_factory_agent.clone();
                std::future::ready(Box::new(agent) as Box<dyn crate::agent::Agent>)
            },
            &run_store,
        );
        let (first_result, second_result) = tokio::join!(first_loop, second_loop);
        let first_cwd = first_observer
            .invocation_contexts()
            .first()
            .and_then(|context| context.working_dir.clone())
            .ok_or_else(|| "first early-stop loop did not record a workspace".to_string())?;
        let second_cwd = second_observer
            .invocation_contexts()
            .first()
            .and_then(|context| context.working_dir.clone())
            .ok_or_else(|| "second early-stop loop did not record a workspace".to_string())?;

        if first_result.iterations.len() != 1 || second_result.iterations.len() != 1 {
            return Err("improvement loops did not stop after the first iteration".to_string());
        }
        if first_cwd == second_cwd {
            return Err("concurrent improvement loops shared a workspace".to_string());
        }
        if first_cwd.exists() || second_cwd.exists() {
            return Err("early-stop improvement workspace was not cleaned up".to_string());
        }
        Ok(())
    }
}
