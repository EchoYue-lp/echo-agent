//! LLM-as-Judge grader — uses an agent to score evaluation outputs.
//!
//! Inspired by `skill-creator/agents/grader.md`. The grader receives an assertion,
//! a task description, and the agent's output, then determines whether the
//! assertion passes and extracts supporting evidence.

use crate::agent::config::DEFAULT_TOKEN_LIMIT;
use crate::agent::Agent;
use serde::{Deserialize, Serialize};

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

        sections.push(format!("--- OUTPUT ---\n{}", truncate(output, DEFAULT_TOKEN_LIMIT)));
        sections.push(format!("--- ASSERTIONS ---\n{assertions_text}"));
        sections.push("Provide your grading in the JSON format specified.".to_string());

        let prompt = sections.join("\n\n");

        let response = agent.execute(&prompt).await;
        let raw = response.unwrap_or_else(|e| format!("{{\"error\": \"{e}\"}}"));

        Self::parse_grading_response(&raw, assertions)
    }

    /// Parse the grading response JSON, with graceful fallback.
    fn parse_grading_response(raw: &str, assertions: &[Assertion]) -> GradingReport {
        // Try to parse structured response
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(raw) {
            let results: Vec<GradeResult> = parsed["results"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .map(|v| GradeResult {
                            assertion_id: v["assertion_id"].as_str().unwrap_or("?").into(),
                            passed: v["passed"].as_bool().unwrap_or(false),
                            confidence: v["confidence"].as_f64().unwrap_or(0.0),
                            evidence: v["evidence"].as_str().unwrap_or("").into(),
                            reasoning: v["reasoning"].as_str().unwrap_or("").into(),
                        })
                        .collect()
                })
                .unwrap_or_default();

            let pass_count = results.iter().filter(|r| r.passed).count();
            let pass_rate = if assertions.is_empty() {
                1.0
            } else {
                pass_count as f64 / assertions.len() as f64
            };

            GradingReport {
                case_id: String::new(),
                results,
                pass_rate,
                overall_assessment: parsed["overall_assessment"].as_str().unwrap_or("").into(),
            }
        } else {
            // Fallback: check raw output for pass/fail indicators
            let results: Vec<GradeResult> = assertions
                .iter()
                .map(|a| {
                    let passed = raw.to_lowercase().contains("passed") && raw.contains(&a.id);
                    GradeResult {
                        assertion_id: a.id.clone(),
                        passed,
                        confidence: 0.5,
                        evidence: raw.chars().take(200).collect(),
                        reasoning: String::new(),
                    }
                })
                .collect();

            let pass_count = results.iter().filter(|r| r.passed).count();
            let pass_rate = if assertions.is_empty() {
                1.0
            } else {
                pass_count as f64 / assertions.len() as f64
            };

            GradingReport {
                case_id: String::new(),
                results,
                pass_rate,
                overall_assessment: "Fallback parsing applied".into(),
            }
        }
    }
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max { s } else { &s[..max] }
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
