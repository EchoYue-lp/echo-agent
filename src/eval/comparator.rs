//! A/B comparison evaluator — compares two agent configurations.
//!
//! Inspired by `skill-creator/agents/comparator.md`. Runs the same eval cases
//! against two different agent configurations and computes per-case deltas.

use crate::eval::{EvalCase, EvalReport, EvalRunner};

/// Result of comparing two eval runs.
pub struct AbComparison {
    /// Eval results from configuration A (baseline).
    pub baseline: EvalReport,
    /// Eval results from configuration B (experiment).
    pub experiment: EvalReport,
    /// Per-case score deltas (experiment - baseline).
    pub per_case_deltas: Vec<CaseDelta>,
    /// Cases where experiment improved over baseline.
    pub improved: usize,
    /// Cases where experiment regressed vs baseline.
    pub regressed: usize,
    /// Cases with no change.
    pub unchanged: usize,
}

/// Per-case comparison data.
pub struct CaseDelta {
    pub case_id: String,
    pub baseline_score: f64,
    pub experiment_score: f64,
    pub delta: f64,
    pub improved: bool,
}

/// Runs A/B comparison between two agent configurations.
pub struct AbComparator;

impl AbComparator {
    /// Compare two agent factories on the same eval cases.
    pub async fn compare(
        cases: &[EvalCase],
        mut baseline_factory: impl FnMut() -> Box<dyn crate::agent::Agent>,
        mut experiment_factory: impl FnMut() -> Box<dyn crate::agent::Agent>,
    ) -> AbComparison {
        let workspace = std::env::temp_dir().join(format!("ab_compare_{}", uuid::Uuid::new_v4()));
        let runner = EvalRunner::new(workspace);

        let baseline = runner.run_all(cases, &mut baseline_factory).await;
        let experiment = runner.run_all(cases, &mut experiment_factory).await;

        let mut per_case_deltas = Vec::new();
        let mut improved = 0usize;
        let mut regressed = 0usize;
        let mut unchanged = 0usize;

        for (base, exp) in baseline.results.iter().zip(experiment.results.iter()) {
            let delta = exp.score - base.score;
            let is_improved = delta > 0.001;
            let is_regressed = delta < -0.001;

            if is_improved {
                improved += 1;
            } else if is_regressed {
                regressed += 1;
            } else {
                unchanged += 1;
            }

            per_case_deltas.push(CaseDelta {
                case_id: base.case_id.clone(),
                baseline_score: base.score,
                experiment_score: exp.score,
                delta,
                improved: is_improved,
            });
        }

        // Attach delta to experiment report
        let mut experiment = experiment;
        experiment.delta_vs_baseline = Some(experiment.avg_score - baseline.avg_score);

        AbComparison {
            baseline,
            experiment,
            per_case_deltas,
            improved,
            regressed,
            unchanged,
        }
    }

    /// Format comparison as a readable summary.
    pub fn format_summary(comparison: &AbComparison) -> String {
        let delta = comparison.experiment.avg_score - comparison.baseline.avg_score;
        let direction = if delta > 0.0 {
            "improved"
        } else if delta < 0.0 {
            "regressed"
        } else {
            "unchanged"
        };
        let mut lines = vec![
            "A/B Comparison Results:".to_string(),
            format!(
                "  Baseline:  {:.4} avg (n={})",
                comparison.baseline.avg_score, comparison.baseline.total
            ),
            format!(
                "  Experiment: {:.4} avg (n={})",
                comparison.experiment.avg_score, comparison.experiment.total
            ),
            format!("  Delta: {delta:+.4} → {direction}"),
            format!(
                "  Improved: {}  Regressed: {}  Unchanged: {}",
                comparison.improved, comparison.regressed, comparison.unchanged
            ),
            String::new(),
            "Per-case deltas:".into(),
        ];
        for cd in &comparison.per_case_deltas {
            let icon = if cd.improved {
                "↑"
            } else if cd.delta < -0.001 {
                "↓"
            } else {
                "="
            };
            lines.push(format!(
                "  {icon} {}: {:+.4} ({:.4} → {:.4})",
                cd.case_id, cd.delta, cd.baseline_score, cd.experiment_score
            ));
        }
        lines.join("\n")
    }
}
