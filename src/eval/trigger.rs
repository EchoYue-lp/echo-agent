//! Trigger accuracy evaluation — measures how well sub-agents activate.
//!
//! Evaluates precision (does it trigger when it should?) and recall
//! (does it NOT trigger when it shouldn't?) for sub-agent routing.

use serde::{Deserialize, Serialize};

/// A test case for sub-agent trigger accuracy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerTestCase {
    /// The user query.
    pub query: String,
    /// Which sub-agent should handle this (expected).
    pub expected_agent: String,
    /// Whether this query should trigger ANY sub-agent (false = should not trigger).
    pub should_trigger: bool,
}

/// Result of a trigger accuracy evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerAccuracy {
    /// Total test cases.
    pub total: usize,
    /// Correctly triggered cases (true positives).
    pub true_positives: usize,
    /// Incorrectly triggered cases (false positives — triggered when it shouldn't).
    pub false_positives: usize,
    /// Correctly skipped cases (true negatives — didn't trigger when it shouldn't).
    pub true_negatives: usize,
    /// Missed triggers (false negatives — should have triggered but didn't).
    pub false_negatives: usize,
    /// Precision: TP / (TP + FP).
    pub precision: f64,
    /// Recall: TP / (TP + FN).
    pub recall: f64,
    /// F1 score.
    pub f1: f64,
}

impl TriggerAccuracy {
    /// Evaluate trigger accuracy from test cases and actual triggers.
    pub fn evaluate(cases: &[TriggerTestCase], actual_triggers: &[(String, Option<String>)]) -> Self {
        let mut tp = 0usize; // triggered correctly
        let mut fp = 0usize; // triggered but shouldn't
        let mut tn = 0usize; // correctly skipped
        let mut fn_count = 0usize; // should trigger but didn't

        for (case, (_query, actual_agent)) in cases.iter().zip(actual_triggers.iter()) {
            let did_trigger = actual_agent.is_some();
            let correct_agent = actual_agent.as_deref() == Some(&case.expected_agent);

            match (case.should_trigger, did_trigger, correct_agent) {
                // Should trigger, did trigger, correct agent → TP
                (true, true, true) => tp += 1,
                // Should trigger, did trigger, wrong agent → partial TP, also FP
                (true, true, false) => { tp += 1; fp += 1; }
                // Should trigger, didn't trigger → FN
                (true, false, _) => fn_count += 1,
                // Shouldn't trigger, triggered anyway → FP
                (false, true, _) => fp += 1,
                // Shouldn't trigger, correctly skipped → TN
                (false, false, _) => tn += 1,
            }
        }

        let total = cases.len();
        let precision = if tp + fp > 0 { tp as f64 / (tp + fp) as f64 } else { 0.0 };
        let recall = if tp + fn_count > 0 { tp as f64 / (tp + fn_count) as f64 } else { 0.0 };
        let f1 = if precision + recall > 0.0 { 2.0 * precision * recall / (precision + recall) } else { 0.0 };

        Self { total, true_positives: tp, false_positives: fp, true_negatives: tn, false_negatives: fn_count, precision, recall, f1 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trigger_accuracy_perfect() {
        let cases = vec![
            TriggerTestCase { query: "read src/main.rs".into(), expected_agent: "code-explorer".into(), should_trigger: true },
            TriggerTestCase { query: "what is 2+2".into(), expected_agent: "".into(), should_trigger: false },
        ];
        let actuals = vec![
            ("read src/main.rs".into(), Some("code-explorer".into())),
            ("what is 2+2".into(), None),
        ];
        let acc = TriggerAccuracy::evaluate(&cases, &actuals);
        assert_eq!(acc.precision, 1.0);
        assert_eq!(acc.recall, 1.0);
        assert_eq!(acc.f1, 1.0);
    }

    #[test]
    fn test_trigger_accuracy_false_positive() {
        let cases = vec![
            TriggerTestCase { query: "what is 2+2".into(), expected_agent: "".into(), should_trigger: false },
        ];
        let actuals = vec![
            ("what is 2+2".into(), Some("code-explorer".into())),
        ];
        let acc = TriggerAccuracy::evaluate(&cases, &actuals);
        assert_eq!(acc.precision, 0.0); // triggered but shouldn't
    }
}
