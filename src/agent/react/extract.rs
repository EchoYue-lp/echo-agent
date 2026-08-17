//! ReactAgent structured extraction
//!
//! Provides two types of structured output capabilities:
//!
//! - **One-shot extraction** (`extract_json` / `extract`): no ReAct loop, directly extracts from LLM
//! - **Full execution extraction** (`execute_typed`): runs the full ReAct loop, requires `output_type`

use super::ReactAgent;
use crate::agent::Agent;
use crate::error::{ReactError, Result};
use crate::llm::types::Message;
use crate::llm::{ChatRequest, ResponseFormat};

impl ReactAgent {
    /// One-shot structured JSON extraction, no ReAct loop.
    ///
    /// Sends a single request to the LLM, asking it to return JSON per `schema`,
    /// returns the parsed [`serde_json::Value`].
    ///
    /// Suitable for "extraction / classification / format conversion" scenarios that don't need tool calls.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # async fn run() -> echo_agent::error::Result<()> {
    /// use echo_agent::prelude::*;
    /// use serde_json::json;
    ///
    /// # let config = AgentConfig::new("qwen3-max", "extractor", "You are an information extraction assistant");
    /// # let agent = ReactAgent::new(config);
    /// let result = agent.extract_json(
    ///     "Zhang San, 28 years old",
    ///     ResponseFormat::json_schema(
    ///         "person",
    ///         json!({ "type": "object",
    ///                 "properties": { "name": { "type": "string" }, "age": { "type": "integer" } },
    ///                 "required": ["name", "age"],
    ///                 "additionalProperties": false }),
    ///     ),
    /// ).await?;
    /// println!("{}", result["name"]);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn extract_json(
        &self,
        prompt: &str,
        schema: ResponseFormat,
    ) -> Result<serde_json::Value> {
        let mut messages = vec![
            Message::system(self.config.system_prompt.clone()),
            Message::user(prompt.to_string()),
        ];

        let max_retries = self.config.llm_max_retries;
        let retry_delay = std::time::Duration::from_millis(self.config.llm_retry_delay_ms);
        let llm_client = self.llm_client().ok_or_else(|| {
            ReactError::Other(
                "JSON extraction requires an explicit LLM config or injected LLM client"
                    .to_string(),
            )
        })?;

        for attempt in 0..=max_retries {
            let response = llm_client
                .chat(ChatRequest {
                    messages: messages.clone(),
                    temperature: Some(0.0),
                    max_tokens: Some(4096),
                    response_format: Some(schema.clone()),
                    ..Default::default()
                })
                .await?;

            let text = response
                .content()
                .filter(|content| !content.trim().is_empty())
                .ok_or_else(|| ReactError::Other("LLM returned empty content".to_string()))?;

            match serde_json::from_str(&text) {
                Ok(value) => return Ok(value),
                Err(e) if attempt < max_retries => {
                    tracing::warn!(
                        attempt = attempt + 1,
                        error = %e,
                        "JSON parse failed, feeding error back to LLM for retry"
                    );
                    // Feed the error back to LLM for self-correction
                    let correction = format!(
                        "Your previous response was not valid JSON.\n\
                         Parse error: {e}\n\
                         Raw response:\n{text}\n\n\
                         Please provide a valid JSON response that strictly matches the required schema."
                    );
                    messages.push(Message::assistant(text));
                    messages.push(Message::user(correction));
                    tokio::time::sleep(retry_delay).await;
                }
                Err(e) => {
                    return Err(ReactError::Other(format!(
                        "JSON parse failed (retried {max_retries} times): {e}\nRaw response: {text}"
                    )));
                }
            }
        }

        Err(ReactError::Other(format!(
            "JSON extraction exhausted {max_retries} retries without finding valid JSON"
        )))
    }

    /// One-shot structured extraction, automatically deserializes the JSON result into the specified type `T`.
    ///
    /// Same as [`extract_json`](Self::extract_json), but additionally performs `serde` deserialization.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use echo_agent::llm::ResponseFormat;
    /// use serde::{Deserialize, Serialize};
    /// use serde_json::json;
    ///
    /// #[derive(Debug, Deserialize)]
    /// struct Person { name: String, age: u32 }
    ///
    /// # async fn run() -> echo_agent::error::Result<()> {
    /// # use echo_agent::prelude::*;
    /// # let config = AgentConfig::new("qwen3-max", "extractor", "You are an extraction assistant");
    /// # let agent = ReactAgent::new(config);
    /// let person: Person = agent.extract(
    ///     "Zhang San, 28 years old",
    ///     ResponseFormat::json_schema(
    ///         "person",
    ///         json!({ "type": "object",
    ///                 "properties": { "name": { "type": "string" }, "age": { "type": "integer" } },
    ///                 "required": ["name", "age"],
    ///                 "additionalProperties": false }),
    ///     ),
    /// ).await?;
    /// println!("Name: {}, Age: {}", person.name, person.age);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn extract<T>(&self, prompt: &str, schema: ResponseFormat) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
    {
        let value = self.extract_json(prompt, schema).await?;
        serde_json::from_value(value)
            .map_err(|e| ReactError::Other(format!("Deserialization failed: {e}")))
    }

    /// After full ReAct execution, deserialize the result as type `T`
    ///
    /// Requires declaring the output type via `ReactAgentBuilder::output_type` when building the Agent.
    /// The framework automatically sets `response_format` to guide the LLM to return matching JSON.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use echo_agent::prelude::*;
    /// use schemars::JsonSchema;
    /// use serde::Deserialize;
    ///
    /// #[derive(Debug, Deserialize, JsonSchema)]
    /// struct Analysis { summary: String, score: f64 }
    ///
    /// # async fn run() -> echo_agent::error::Result<()> {
    /// let mut agent = ReactAgentBuilder::new()
    ///     .model("qwen3-max")
    ///     .system_prompt("You are an analysis assistant, please return analysis results in JSON format")
    ///     .output_type::<Analysis>()
    ///     .build()?;
    ///
    /// let result: Analysis = agent.execute_typed("Analyze the pros and cons of the Rust language").await?;
    /// println!("Score: {}", result.score);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn execute_typed<T>(&mut self, task: &str) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
    {
        let raw = self.execute(task).await?;
        serde_json::from_str(&raw).map_err(|e| {
            ReactError::Other(format!(
                "Structured output deserialization failed: {e}\nRaw response: {raw}"
            ))
        })
    }
}
