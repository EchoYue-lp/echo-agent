//! LLM-driven Critic implementation

use crate::error::{ReactError, Result};
use crate::llm;
use crate::llm::ResponseFormat;
use crate::llm::types::Message;
use echo_core::agent::Critic;
use echo_core::agent::{Critique, CritiqueOutput, critique_output_schema};
use futures::future::BoxFuture;
use reqwest::Client;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// LLM-driven evaluator
///
/// Uses a large language model to evaluate the quality of Agent output, returning structured `Critique`.
/// Reuses the `LlmPlanner` pattern: LLM call + structured JSON output + auto-fix.
pub struct LlmCritic {
    model: String,
    client: Arc<Client>,
    system_prompt: String,
    pass_threshold: f64,
}

impl LlmCritic {
    /// Create an LLM evaluator
    ///
    /// # Parameters
    /// * `model` - LLM model identifier for quality evaluation
    ///
    /// # Default configuration
    /// * System prompt: multi-dimensional quality evaluation expert (accuracy, completeness, clarity, usefulness)
    /// * Pass threshold: 7.0 (score >= 7.0 considered passing)
    /// * HTTP client: newly created `reqwest::Client`
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            client: Arc::new(
                Client::builder()
                    .timeout(std::time::Duration::from_secs(120))
                    .build()
                    .unwrap_or_default(),
            ),
            system_prompt: Self::default_system_prompt().to_string(),
            pass_threshold: 7.0,
        }
    }

    /// Custom system prompt
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = prompt.into();
        self
    }

    /// Set pass threshold (0.0 - 10.0)
    pub fn with_pass_threshold(mut self, threshold: f64) -> Self {
        self.pass_threshold = threshold;
        self
    }

    fn default_system_prompt() -> &'static str {
        "You are a strict quality evaluation expert. You need to evaluate the quality of the given response.\n\n\
        Evaluation dimensions:\n\
        1. Accuracy: Are the facts correct\n\
        2. Completeness: Does it cover all key points\n\
        3. Clarity: Is the expression clear and easy to understand\n\
        4. Usefulness: Does it provide valuable information\n\n\
        Scoring standards:\n\
        - 9.0-10.0: Excellent, almost flawless\n\
        - 7.0-8.9: Good, basically correct but with minor flaws\n\
        - 5.0-6.9: Mediocre, with noticeable deficiencies\n\
        - 0.0-4.9: Poor, contains serious errors\n\n\
        Please strictly return structured data according to the JSON Schema."
    }

    /// Parse LLM response as CritiqueOutput
    fn parse_critique_output(content: &str) -> Result<CritiqueOutput> {
        // 1. Direct parse
        if let Ok(output) = serde_json::from_str::<CritiqueOutput>(content) {
            return Ok(output);
        }

        // 2. Extract from markdown code block
        let json_str = crate::utils::json_parse::extract_json_from_markdown(content);
        if let Ok(output) = serde_json::from_str::<CritiqueOutput>(&json_str) {
            return Ok(output);
        }

        // 3. Auto-fix
        Self::try_auto_fix(&json_str)
    }

    fn try_auto_fix(json_str: &str) -> Result<CritiqueOutput> {
        let fixed = crate::utils::json_parse::clean_json(json_str);

        match serde_json::from_str::<CritiqueOutput>(&fixed) {
            Ok(output) => {
                info!("Auto-fix succeeded for LLM critique output");
                Ok(output)
            }
            Err(e) => {
                warn!(error = %e, "Failed to parse critique output");
                // Fallback: construct default non-passing evaluation
                Ok(CritiqueOutput {
                    score: 5.0,
                    passed: false,
                    feedback: json_str.trim().to_string(),
                    suggestions: vec![],
                })
            }
        }
    }
}

impl Critic for LlmCritic {
    fn critique<'a>(
        &'a self,
        task: &'a str,
        answer: &'a str,
        context: &'a str,
    ) -> BoxFuture<'a, Result<Critique>> {
        Box::pin(async move {
            info!(model = %self.model, "LlmCritic: evaluating answer");

            let user_content = if context.is_empty() {
                format!(
                    "Original task:\n{}\n\nResponse to evaluate:\n{}",
                    task, answer
                )
            } else {
                format!(
                    "Original task:\n{}\n\nResponse to evaluate:\n{}\n\nAdditional context:\n{}",
                    task, answer, context
                )
            };

            let messages = vec![
                Message::system(self.system_prompt.clone()),
                Message::user(user_content),
            ];

            let response_format = Some(ResponseFormat::json_schema(
                "critique_output",
                critique_output_schema(),
            ));

            let response = llm::chat(
                self.client.clone(),
                &self.model,
                &messages,
                Some(0.3),
                Some(2048u32),
                Some(false),
                None,
                None,
                response_format,
            )
            .await
            .map_err(|e| ReactError::Other(format!("LLM critique call failed: {}", e)))?;

            let content = response
                .choices
                .first()
                .and_then(|c| c.message.content.as_text())
                .unwrap_or_default();

            debug!(response = %content, "LlmCritic raw response");

            let output = Self::parse_critique_output(&content)?;
            let mut critique: Critique = output.into();

            // Override LLM's passed judgment with threshold
            critique.passed = critique.score >= self.pass_threshold;

            info!(
                score = critique.score,
                passed = critique.passed,
                "LlmCritic: evaluation complete"
            );

            Ok(critique)
        })
    }

    fn name(&self) -> &str {
        "llm_critic"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_critique_output_json() {
        let json = r#"{"score": 8.5, "passed": true, "feedback": "Accurate response", "suggestions": ["Could be more detailed"]}"#;
        let output = LlmCritic::parse_critique_output(json).unwrap();
        assert_eq!(output.score, 8.5);
        assert!(output.passed);
    }

    #[test]
    fn test_parse_critique_output_markdown() {
        let response = r#"```json
{"score": 6.0, "passed": false, "feedback": "Not complete enough", "suggestions": ["Add examples"]}
```"#;
        let output = LlmCritic::parse_critique_output(response).unwrap();
        assert_eq!(output.score, 6.0);
        assert!(!output.passed);
    }

    #[test]
    fn test_parse_critique_auto_fix() {
        let json = r#"{"score": 7.0, "passed": true, "feedback": "Good",}"#;
        let output = LlmCritic::parse_critique_output(json).unwrap();
        assert_eq!(output.score, 7.0);
    }

    #[test]
    fn test_parse_critique_fallback() {
        let text = "Unparseable text";
        let output = LlmCritic::parse_critique_output(text).unwrap();
        assert!(!output.passed); // Fallback: not passed
        assert_eq!(output.score, 5.0);
    }
}
