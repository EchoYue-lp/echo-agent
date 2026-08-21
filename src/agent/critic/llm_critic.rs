//! LLM-driven Critic implementation

use crate::error::{ReactError, Result};
use crate::llm::types::Message;
use crate::llm::{ChatRequest, LlmClient, ResponseFormat};
use echo_core::agent::Critic;
use echo_core::agent::{Critique, CritiqueOutput, critique_output_schema};
use echo_core::retry::{RetryPolicy, with_retry_if};
use futures::future::BoxFuture;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// LLM-driven evaluator
///
/// Uses a large language model to evaluate the quality of Agent output, returning structured `Critique`.
/// Reuses the `LlmPlanner` pattern: LLM call + structured JSON output + auto-fix.
pub struct LlmCritic {
    model: String,
    client: Arc<dyn LlmClient>,
    system_prompt: String,
    pass_threshold: f64,
    cache_user_id: Option<String>,
    retry_policy: RetryPolicy,
}

impl LlmCritic {
    /// Create an LLM evaluator
    ///
    /// # Parameters
    /// * `client` - The already-prepared client used by the owning Agent
    ///
    /// # Default configuration
    /// * System prompt: multi-dimensional quality evaluation expert (accuracy, completeness, clarity, usefulness)
    /// * Pass threshold: 7.0 (score >= 7.0 considered passing)
    pub fn new(client: Arc<dyn LlmClient>) -> Self {
        Self {
            model: client.model_name().to_string(),
            client,
            system_prompt: Self::default_system_prompt().to_string(),
            pass_threshold: 7.0,
            cache_user_id: None,
            retry_policy: RetryPolicy::default(),
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

    /// Set provider-side user id for KVCache/content-safety/scheduling isolation
    /// on providers that support it (for example DeepSeek).
    pub fn with_cache_user_id(mut self, user_id: impl Into<String>) -> Self {
        self.cache_user_id = Some(user_id.into());
        self
    }

    /// Set the retry policy for transient provider failures during evaluation.
    pub fn with_retry_policy(mut self, retry_policy: RetryPolicy) -> Self {
        self.retry_policy = retry_policy;
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

impl LlmCritic {
    /// Issue the critique LLM call with a given `response_format`.
    ///
    /// Extracted so the main `critique` flow can retry once without
    /// `response_format` when the provider rejects structured output
    /// (some OpenAI-compatible endpoints return HTTP 400 for
    /// `response_format: json_schema`, e.g. "This response_format type is
    /// unavailable now"). Returning the raw text here keeps the JSON parsing
    /// concern in `parse_critique_output`.
    async fn call_llm_once(
        &self,
        messages: &[Message],
        response_format: Option<ResponseFormat>,
    ) -> Result<String> {
        let response = self
            .client
            .chat(ChatRequest {
                messages: messages.to_vec(),
                temperature: Some(0.3),
                max_tokens: Some(2048u32),
                response_format,
                user_id: self.cache_user_id.clone(),
                ..Default::default()
            })
            .await?;

        Ok(response.content().unwrap_or_default())
    }

    /// Call the provider using the framework retry policy for transient LLM
    /// failures. Permanent errors stay single-attempt so authentication and
    /// request-shape problems surface immediately.
    async fn call_llm(
        &self,
        messages: &[Message],
        response_format: Option<ResponseFormat>,
    ) -> Result<String> {
        with_retry_if(
            &self.retry_policy,
            || self.call_llm_once(messages, response_format.clone()),
            crate::agent::react::is_retryable_llm_error,
        )
        .await
    }

    /// Whether an error message indicates the provider rejected the
    /// `response_format` parameter (and thus a text fallback is worth trying).
    ///
    /// Matches on common phrasings from OpenAI-compatible endpoints that don't
    /// support structured output (DeepSeek/Qwen/GLM/etc. return variants of
    /// "response_format type is unavailable"). Conservative: only triggers
    /// fallback on a clear signal, so unrelated API errors still surface.
    fn is_response_format_unsupported(err: &str) -> bool {
        let lower = err.to_ascii_lowercase();
        lower.contains("response_format")
            && (lower.contains("unavailable")
                || lower.contains("unsupported")
                || lower.contains("not supported")
                || lower.contains("invalid"))
    }

    fn should_skip_structured_response_format(&self) -> bool {
        let model = self.model.to_ascii_lowercase();
        model.starts_with("deepseek-")
    }

    fn fallback_messages(&self, user_content: &str) -> Vec<Message> {
        vec![
            Message::system(self.system_prompt.clone()),
            Message::user(format!(
                "{user_content}\n\n\
                 IMPORTANT: respond with ONLY a JSON object matching this schema, \
                 no markdown fences, no prose. Fields: score (0-10 number), \
                 passed (boolean), feedback (string), suggestions (array of strings)."
            )),
        ]
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
                Message::user(user_content.clone()),
            ];
            let response_format = Some(ResponseFormat::json_schema(
                "critique_output",
                critique_output_schema(),
            ));

            let content = if self.should_skip_structured_response_format() {
                debug!(
                    model = %self.model,
                    "critique: skipping response_format for provider without json_schema support"
                );
                self.call_llm(&self.fallback_messages(&user_content), None)
                    .await
                    .map_err(|e| ReactError::Other(format!("LLM critique call failed: {e}")))?
            } else {
                // First attempt: structured output via json_schema (preferred —
                // guarantees schema-conformant JSON when the provider supports it).
                match self.call_llm(&messages, response_format).await {
                    Ok(text) => text,
                    Err(e) if Self::is_response_format_unsupported(&e.to_string()) => {
                        // Provider rejects structured output — retry once without
                        // response_format. The system prompt already asks for JSON;
                        // append an explicit instruction so the model emits parseable
                        // JSON that `parse_critique_output` (markdown/autofix aware)
                        // can still handle. Keeps critic functional on providers
                        // that lack json_schema support.
                        warn!(
                            error = %e,
                            "critique: structured output unsupported, retrying as plain text"
                        );
                        self.call_llm(&self.fallback_messages(&user_content), None)
                            .await
                            .map_err(|e| {
                                ReactError::Other(format!("LLM critique call failed: {e}"))
                            })?
                    }
                    Err(e) => {
                        return Err(ReactError::Other(format!("LLM critique call failed: {e}")));
                    }
                }
            };

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
    use crate::error::LlmError;
    use crate::testing::MockLlmClient;
    use std::time::Duration;

    fn critic_for_model(model: &str) -> LlmCritic {
        LlmCritic::new(Arc::new(
            MockLlmClient::new().with_model_name(model.to_string()),
        ))
    }

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

    #[test]
    fn test_is_response_format_unsupported_matches_provider_phrasings() {
        // Real phrasing from the user's bug report (国产 OpenAI 兼容端点).
        let real = "API error (status 400): This response_format type is unavailable now";
        assert!(LlmCritic::is_response_format_unsupported(real));
        // Common variants across providers.
        assert!(LlmCritic::is_response_format_unsupported(
            "response_format unsupported"
        ));
        assert!(LlmCritic::is_response_format_unsupported(
            "response_format is not supported by this model"
        ));
        assert!(LlmCritic::is_response_format_unsupported(
            "invalid response_format parameter"
        ));
    }

    #[test]
    fn test_is_response_format_unsupported_rejects_unrelated_errors() {
        // Unrelated API errors must NOT trigger the fallback (would mask real
        // failures like auth/quota/network issues).
        assert!(!LlmCritic::is_response_format_unsupported(
            "401 unauthorized"
        ));
        assert!(!LlmCritic::is_response_format_unsupported("rate limited"));
        assert!(!LlmCritic::is_response_format_unsupported(
            "network timeout"
        ));
        assert!(!LlmCritic::is_response_format_unsupported(
            "insufficient quota"
        ));
    }

    #[test]
    fn deepseek_skips_structured_response_format() {
        let critic = critic_for_model("deepseek-v4-flash");
        assert!(critic.should_skip_structured_response_format());

        let other = critic_for_model("gpt-5.1");
        assert!(!other.should_skip_structured_response_format());
    }

    #[test]
    fn fallback_messages_request_json_only() -> std::result::Result<(), &'static str> {
        let critic = critic_for_model("deepseek-v4-flash");
        let messages = critic.fallback_messages("Evaluate this");
        assert_eq!(messages.len(), 2);
        let user_message = messages
            .get(1)
            .and_then(|m| m.content.as_text())
            .ok_or("expected fallback user message text")?;
        assert!(user_message.contains("ONLY a JSON object"));
        assert!(user_message.contains("score (0-10 number)"));
        Ok(())
    }

    #[tokio::test]
    async fn critique_uses_the_prepared_client_transport() -> Result<()> {
        let client = Arc::new(
            MockLlmClient::new()
                .with_model_name("prepared-model")
                .with_response(r#"{"score":8.0,"passed":true,"feedback":"ok","suggestions":[]}"#),
        );
        let critic = LlmCritic::new(client.clone());

        let critique = critic.critique("task", "answer", "").await?;

        assert!(critique.passed);
        assert_eq!(client.call_count(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn critique_retries_transient_server_error() -> Result<()> {
        let client = Arc::new(
            MockLlmClient::new()
                .with_error(ReactError::Llm(Box::new(LlmError::ApiError {
                    status: 502,
                    message: "Upstream request failed".to_string(),
                })))
                .with_response(r#"{"score":8.0,"passed":true,"feedback":"ok","suggestions":[]}"#),
        );
        let critic =
            LlmCritic::new(client.clone()).with_retry_policy(RetryPolicy::new(1, Duration::ZERO));

        let critique = critic.critique("task", "answer", "").await?;

        assert!(critique.passed);
        assert_eq!(client.call_count(), 2);
        Ok(())
    }

    #[tokio::test]
    async fn critique_does_not_retry_permanent_api_error() {
        let client = Arc::new(
            MockLlmClient::new()
                .with_error(ReactError::Llm(Box::new(LlmError::ApiError {
                    status: 401,
                    message: "Unauthorized".to_string(),
                })))
                .with_response(r#"{"score":8.0,"passed":true,"feedback":"ok","suggestions":[]}"#),
        );
        let critic =
            LlmCritic::new(client.clone()).with_retry_policy(RetryPolicy::new(3, Duration::ZERO));

        let result = critic.critique("task", "answer", "").await;

        assert!(result.is_err());
        assert_eq!(client.call_count(), 1);
    }
}
