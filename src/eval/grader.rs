//! LLM-as-Judge grader — uses an agent to score evaluation outputs.
//!
//! Inspired by `skill-creator/agents/grader.md`. The grader receives an assertion,
//! a task description, and the agent's output, then determines whether the
//! assertion passes and extracts supporting evidence.

use crate::agent::Agent;
use crate::agent::config::DEFAULT_TOKEN_LIMIT;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// A single assertion to check against an agent's output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Assertion {
    /// Unique identifier for this assertion.
    pub id: String,
    /// What to check (natural language, e.g. "The output contains a valid JSON object").
    pub check: String,
    /// Expected outcome hint for the grader.
    pub expected: String,
}

/// Result of grading a single assertion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GradeResult {
    pub assertion_id: String,
    pub passed: bool,
    pub confidence: f64,
    pub evidence: String,
    pub reasoning: String,
}

/// Full grading output for one eval case.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GradingReport {
    pub case_id: String,
    pub results: Vec<GradeResult>,
    pub pass_rate: f64,
    pub overall_assessment: String,
}

/// LLM-based grader for evaluating agent outputs.
pub struct LlmGrader {
    grader_prompt: String,
}

impl Default for LlmGrader {
    fn default() -> Self {
        Self::new()
    }
}

impl LlmGrader {
    pub fn new() -> Self {
        Self {
            grader_prompt: concat!(
                "You are an expert evaluator. You will receive:\n",
                "1. A TASK that an agent was asked to complete\n",
                "2. The agent's OUTPUT\n",
                "3. A list of ASSERTIONS to check\n\n",
                "For each assertion, determine:\n",
                "- PASSED: true/false\n",
                "- CONFIDENCE: 0.0-1.0\n",
                "- EVIDENCE: quote the specific part of the output that proves or disproves\n",
                "- REASONING: explain your judgment\n\n",
                "Output format (JSON):\n",
                "{\n",
                "  \"results\": [\n",
                "    {\"assertion_id\": \"...\", \"passed\": bool, \"confidence\": 0.0-1.0,\n",
                "     \"evidence\": \"...\", \"reasoning\": \"...\"}\n",
                "  ],\n",
                "  \"overall_assessment\": \"brief summary\"\n",
                "}\n\n",
                "Be strict but fair. If evidence is ambiguous, mark as not passed."
            )
            .to_string(),
        }
    }

    /// Grade an agent's output against a set of assertions.
    pub async fn grade(
        &self,
        agent: &dyn Agent,
        task: &str,
        output: &str,
        assertions: &[Assertion],
    ) -> GradingReport {
        self.grade_with_trajectory(agent, task, output, assertions, None)
            .await
    }

    /// Grade an agent's output with optional trajectory summary.
    ///
    /// The `trajectory_summary` can include tool calls made, files edited, etc.
    /// to give the grader more context about the agent's execution path.
    pub async fn grade_with_trajectory(
        &self,
        agent: &dyn Agent,
        task: &str,
        output: &str,
        assertions: &[Assertion],
        trajectory_summary: Option<&str>,
    ) -> GradingReport {
        let assertions_text: String = assertions
            .iter()
            .map(|a| {
                format!(
                    "- [{}] Check: {}\n  Expected: {}",
                    a.id, a.check, a.expected
                )
            })
            .collect::<Vec<_>>()
            .join("\n");

        let mut sections = vec![self.grader_prompt.clone(), format!("--- TASK ---\n{task}")];

        if let Some(traj) = trajectory_summary {
            sections.push(format!("--- TRAJECTORY ---\n{traj}"));
        }

        sections.push(format!(
            "--- OUTPUT ---\n{}",
            truncate(output, DEFAULT_TOKEN_LIMIT)
        ));
        sections.push(format!("--- ASSERTIONS ---\n{assertions_text}"));
        sections.push("Provide your grading in the JSON format specified.".to_string());

        let prompt = sections.join("\n\n");

        match agent.execute(&prompt).await {
            Ok(raw) => Self::parse_grading_response(&raw, assertions),
            Err(error) => Self::failed_report(assertions, format!("grader agent failed: {error}")),
        }
    }

    /// Parse the grading response JSON, with graceful fallback.
    fn parse_grading_response(raw: &str, assertions: &[Assertion]) -> GradingReport {
        // Try to parse structured response
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(raw) {
            let Some(rows) = parsed.get("results").and_then(serde_json::Value::as_array) else {
                return Self::failed_report(assertions, "grader response omitted results".into());
            };
            let expected: HashSet<&str> = assertions.iter().map(|item| item.id.as_str()).collect();
            let mut by_id = HashMap::with_capacity(rows.len());
            for row in rows {
                let Some(assertion_id) =
                    row.get("assertion_id").and_then(serde_json::Value::as_str)
                else {
                    return Self::failed_report(
                        assertions,
                        "grader row omitted assertion_id".into(),
                    );
                };
                if !expected.contains(assertion_id) || by_id.contains_key(assertion_id) {
                    return Self::failed_report(
                        assertions,
                        format!(
                            "grader returned unknown or duplicate assertion id {assertion_id:?}"
                        ),
                    );
                }
                let Some(confidence) = row.get("confidence").and_then(serde_json::Value::as_f64)
                else {
                    return Self::failed_report(
                        assertions,
                        format!("grader row {assertion_id:?} omitted confidence"),
                    );
                };
                if !confidence.is_finite() || !(0.0..=1.0).contains(&confidence) {
                    return Self::failed_report(
                        assertions,
                        format!("grader row {assertion_id:?} has invalid confidence"),
                    );
                }
                by_id.insert(
                    assertion_id.to_string(),
                    GradeResult {
                        assertion_id: assertion_id.to_string(),
                        passed: row
                            .get("passed")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false),
                        confidence,
                        evidence: row
                            .get("evidence")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        reasoning: row
                            .get("reasoning")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    },
                );
            }
            if by_id.len() != assertions.len() {
                return Self::failed_report(
                    assertions,
                    "grader response did not cover every assertion".into(),
                );
            }
            let results: Vec<GradeResult> = assertions
                .iter()
                .filter_map(|assertion| by_id.remove(&assertion.id))
                .collect();
            let pass_count = results.iter().filter(|result| result.passed).count();
            let pass_rate = if assertions.is_empty() {
                1.0
            } else {
                pass_count as f64 / assertions.len() as f64
            };

            GradingReport {
                case_id: String::new(),
                results,
                pass_rate,
                overall_assessment: parsed
                    .get("overall_assessment")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .into(),
            }
        } else {
            Self::failed_report(assertions, "grader response was not valid JSON".into())
        }
    }

    fn failed_report(assertions: &[Assertion], reason: String) -> GradingReport {
        GradingReport {
            case_id: String::new(),
            results: assertions
                .iter()
                .map(|assertion| GradeResult {
                    assertion_id: assertion.id.clone(),
                    passed: false,
                    confidence: 0.0,
                    evidence: String::new(),
                    reasoning: reason.clone(),
                })
                .collect(),
            pass_rate: if assertions.is_empty() { 1.0 } else { 0.0 },
            overall_assessment: reason,
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_valid_response() {
        let raw = r#"{"results":[{"assertion_id":"a1","passed":true,"confidence":0.9,"evidence":"found","reasoning":"clear"}],"overall_assessment":"good"}"#;
        let assertions = vec![Assertion {
            id: "a1".into(),
            check: "check".into(),
            expected: "yes".into(),
        }];
        let report = LlmGrader::parse_grading_response(raw, &assertions);
        assert_eq!(report.pass_rate, 1.0);
        assert_eq!(report.results.len(), 1);
        assert!(report.results[0].passed);
    }
}
