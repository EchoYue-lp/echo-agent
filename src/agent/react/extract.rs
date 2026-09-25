//! ReactAgent structured extraction
//!
//! Provides two types of structured output capabilities:
//!
//! - **One-shot extraction** (`extract_json` / `extract`): no ReAct loop, directly extracts from LLM
//! - **Full execution extraction** (`execute_typed`): runs the full ReAct loop, requires `output_type`

use super::ReactAgent;
use crate::agent::Agent;
use crate::error::{ReactError, Result, StructuredOutputError};
use crate::llm::types::Message;
use crate::llm::{ChatRequest, ResponseFormat};

struct RejectExternalSchemaReferences;

impl jsonschema::Retrieve for RejectExternalSchemaReferences {
    fn retrieve(
        &self,
        _uri: &jsonschema::Uri<String>,
    ) -> std::result::Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
        Err("external JSON Schema references are not supported".into())
    }
}

/// Prepared once per invocation so retries validate against the same schema.
/// External references cannot trigger blocking network or file I/O in an async turn.
pub(crate) struct PreparedResponseFormat {
    strict_schema: Option<(String, jsonschema::Validator)>,
}

impl PreparedResponseFormat {
    pub(crate) fn new(format: &ResponseFormat) -> Result<Self> {
        let strict_schema = match format {
            ResponseFormat::JsonSchema { json_schema } if json_schema.strict => {
                let validator = jsonschema::options()
                    .with_retriever(RejectExternalSchemaReferences)
                    .build(&json_schema.schema)
                    .map_err(|error| StructuredOutputError::InvalidSchema {
                        name: json_schema.name.clone(),
                        schema_path: bounded_schema_path(error.instance_path()),
                    })?;
                Some((json_schema.name.clone(), validator))
            }
            _ => None,
        };
        Ok(Self { strict_schema })
    }

    pub(crate) fn requires_schema_retry(&self) -> bool {
        self.strict_schema.is_some()
    }

    pub(crate) fn is_retryable_validation_error(&self, error: &ReactError) -> bool {
        match error {
            ReactError::StructuredOutput(inner) => match inner.as_ref() {
                StructuredOutputError::InvalidJson { .. } => true,
                StructuredOutputError::SchemaMismatch { .. } => self.requires_schema_retry(),
                _ => false,
            },
            _ => false,
        }
    }

    pub(crate) fn validate(&self, value: &serde_json::Value) -> Result<()> {
        if let Some((name, validator)) = &self.strict_schema {
            validator
                .validate(value)
                .map_err(|error| StructuredOutputError::SchemaMismatch {
                    name: name.clone(),
                    schema_path: bounded_schema_path(error.schema_path()),
                })?;
        }
        Ok(())
    }

    pub(crate) fn parse_text(&self, text: &str) -> Result<serde_json::Value> {
        let value = parse_json_text(text)?;
        self.validate(&value)?;
        Ok(value)
    }
}

fn bounded_schema_path(path: &jsonschema::paths::Location) -> String {
    let path = path.to_string();
    if path.is_empty() {
        "<root>".to_string()
    } else {
        path.chars().take(160).collect()
    }
}

pub(crate) fn parse_json_text(text: &str) -> Result<serde_json::Value> {
    serde_json::from_str(text).map_err(|error| {
        StructuredOutputError::InvalidJson {
            line: error.line(),
            column: error.column(),
        }
        .into()
    })
}

impl ReactAgent {
    /// One-shot structured JSON extraction, no ReAct loop.
    ///
    /// Sends `schema` as a provider hint and returns the parsed [`serde_json::Value`].
    /// Malformed JSON and strict schema mismatches can trigger bounded correction retries.
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
        let prepared = PreparedResponseFormat::new(&schema)?;
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

            match prepared.parse_text(&text) {
                Ok(value) => return Ok(value),
                Err(error)
                    if attempt < max_retries && prepared.is_retryable_validation_error(&error) =>
                {
                    tracing::warn!(
                        attempt = attempt + 1,
                        error = %error,
                        "Structured JSON validation failed, requesting a corrected response"
                    );
                    let correction = format!(
                        "Your previous response was not valid for the requested JSON format: {error}. \
                         Return only valid JSON that satisfies the schema, if one was provided."
                    );
                    messages.push(Message::assistant(text));
                    messages.push(Message::user(correction));
                    tokio::time::sleep(retry_delay).await;
                }
                Err(error) => return Err(error),
            }
        }

        Err(ReactError::Other(
            "JSON extraction exhausted its configured retry budget".to_string(),
        ))
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
        serde_json::from_value(value).map_err(|_| StructuredOutputError::InvalidTargetType.into())
    }

    /// After full ReAct execution, deserialize the result as type `T`
    ///
    /// Requires `ReactAgentBuilder::output_type` or another configured response format.
    /// A strict schema is checked against the final JSON value before deserialization.
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
        let format = self
            .config
            .response_format
            .as_ref()
            .ok_or(StructuredOutputError::MissingResponseFormat)?;
        let prepared = PreparedResponseFormat::new(format)?;
        let raw = self.execute(task).await?;
        let value = prepared.parse_text(&raw)?;
        serde_json::from_value(value).map_err(|_| StructuredOutputError::InvalidTargetType.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentConfig;
    use crate::llm::types::JsonSchemaSpec;
    use crate::testing::MockLlmClient;
    use serde_json::json;
    use std::sync::Arc;

    #[tokio::test]
    async fn strict_extract_rejects_valid_json_that_violates_schema() {
        let llm = Arc::new(MockLlmClient::new().with_response(r#"{"age":"not-an-integer"}"#));
        let agent = ReactAgent::new(
            AgentConfig::new("mock-model", "extractor", "Extract data").llm_max_retries(0),
        )
        .with_llm_client(llm);
        let schema = ResponseFormat::json_schema(
            "person",
            json!({
                "type": "object",
                "properties": {"age": {"type": "integer"}},
                "required": ["age"]
            }),
        );

        let result = agent.extract_json("Extract age", schema).await;
        assert!(matches!(
            result,
            Err(ReactError::StructuredOutput(inner))
                if matches!(inner.as_ref(), StructuredOutputError::SchemaMismatch { .. })
        ));
    }

    #[tokio::test]
    async fn strict_extract_retries_schema_mismatch() -> Result<()> {
        let llm = Arc::new(
            MockLlmClient::new()
                .with_response(r#"{"age":"wrong"}"#)
                .with_response(r#"{"age":28}"#),
        );
        let agent = ReactAgent::new(
            AgentConfig::new("mock-model", "extractor", "Extract data")
                .llm_max_retries(1)
                .llm_retry_delay_ms(0),
        )
        .with_llm_client(llm.clone());
        let schema = ResponseFormat::json_schema(
            "person",
            json!({
                "type": "object",
                "properties": {"age": {"type": "integer"}},
                "required": ["age"]
            }),
        );

        let value = agent.extract_json("Extract age", schema).await?;
        assert_eq!(value, json!({"age": 28}));
        assert_eq!(llm.call_count(), 2);
        Ok(())
    }

    #[tokio::test]
    async fn strict_extract_exhaustion_is_typed_and_never_echoes_values() {
        let llm = Arc::new(
            MockLlmClient::new()
                .with_response(r#"{"age":"private-first"}"#)
                .with_response(r#"{"age":"private-second"}"#),
        );
        let agent = ReactAgent::new(
            AgentConfig::new("mock-model", "extractor", "Extract data")
                .llm_max_retries(1)
                .llm_retry_delay_ms(0),
        )
        .with_llm_client(llm.clone());
        let schema = ResponseFormat::json_schema(
            "person",
            json!({"type": "object", "properties": {"age": {"type": "integer"}}}),
        );

        let result = agent.extract_json("Extract age", schema).await;
        assert_eq!(llm.call_count(), 2);
        assert!(matches!(
            &result,
            Err(ReactError::StructuredOutput(inner))
                if matches!(inner.as_ref(), StructuredOutputError::SchemaMismatch { .. })
        ));
        let message = result
            .err()
            .map(|error| error.to_string())
            .unwrap_or_default();
        assert!(!message.contains("private-first"));
        assert!(!message.contains("private-second"));
    }

    #[tokio::test]
    async fn invalid_strict_schema_rejects_before_llm_call() {
        let llm = Arc::new(MockLlmClient::new().with_response(r#"{"age":28}"#));
        let agent = ReactAgent::new(AgentConfig::new("mock-model", "extractor", "Extract data"))
            .with_llm_client(llm.clone());
        let result = agent
            .extract_json(
                "Extract age",
                ResponseFormat::json_schema("person", json!({"type": 7})),
            )
            .await;

        assert!(matches!(
            result,
            Err(ReactError::StructuredOutput(inner))
                if matches!(inner.as_ref(), StructuredOutputError::InvalidSchema { .. })
        ));
        assert_eq!(llm.call_count(), 0);
    }

    #[tokio::test]
    async fn strict_false_is_only_a_provider_hint() -> Result<()> {
        let llm = Arc::new(MockLlmClient::new().with_response(r#"{"age":"not-an-integer"}"#));
        let agent = ReactAgent::new(AgentConfig::new("mock-model", "extractor", "Extract data"))
            .with_llm_client(llm.clone());
        let format = ResponseFormat::JsonSchema {
            json_schema: JsonSchemaSpec {
                name: "person".to_string(),
                schema: json!({"type": "object", "properties": {"age": {"type": "integer"}}}),
                strict: false,
            },
        };

        assert_eq!(
            agent.extract_json("Extract age", format).await?,
            json!({"age": "not-an-integer"})
        );
        assert_eq!(llm.call_count(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn json_object_only_requires_valid_json() -> Result<()> {
        let llm = Arc::new(MockLlmClient::new().with_response(r#"[1,2]"#));
        let agent = ReactAgent::new(AgentConfig::new("mock-model", "extractor", "Extract data"))
            .with_llm_client(llm.clone());

        assert_eq!(
            agent
                .extract_json("Extract numbers", ResponseFormat::JsonObject)
                .await?,
            json!([1, 2])
        );
        assert_eq!(llm.call_count(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn malformed_json_retries_with_the_same_bounded_budget() -> Result<()> {
        let llm = Arc::new(
            MockLlmClient::new()
                .with_response("private malformed output")
                .with_response(r#"{"age":28}"#),
        );
        let agent = ReactAgent::new(
            AgentConfig::new("mock-model", "extractor", "Extract data")
                .llm_max_retries(1)
                .llm_retry_delay_ms(0),
        )
        .with_llm_client(llm.clone());
        let result = agent
            .extract_json(
                "Extract age",
                ResponseFormat::json_schema(
                    "person",
                    json!({"type": "object", "properties": {"age": {"type": "integer"}}}),
                ),
            )
            .await?;

        assert_eq!(result, json!({"age": 28}));
        assert_eq!(llm.call_count(), 2);
        Ok(())
    }

    #[tokio::test]
    async fn malformed_json_exhaustion_is_typed_and_redacted() {
        let llm = Arc::new(
            MockLlmClient::new()
                .with_response("private malformed first")
                .with_response("private malformed second"),
        );
        let agent = ReactAgent::new(
            AgentConfig::new("mock-model", "extractor", "Extract data")
                .llm_max_retries(1)
                .llm_retry_delay_ms(0),
        )
        .with_llm_client(llm.clone());
        let result = agent
            .extract_json("Extract age", ResponseFormat::JsonObject)
            .await;

        assert!(matches!(
            &result,
            Err(ReactError::StructuredOutput(inner))
                if matches!(inner.as_ref(), StructuredOutputError::InvalidJson { .. })
        ));
        assert_eq!(llm.call_count(), 2);
        let message = result
            .err()
            .map(|error| error.to_string())
            .unwrap_or_default();
        assert!(!message.contains("private malformed first"));
        assert!(!message.contains("private malformed second"));
    }

    #[tokio::test]
    async fn external_schema_reference_fails_closed_in_async_context() {
        let llm = Arc::new(MockLlmClient::new().with_response(r#"{"age":28}"#));
        let agent = ReactAgent::new(AgentConfig::new("mock-model", "extractor", "Extract data"))
            .with_llm_client(llm.clone());
        let result = agent
            .extract_json(
                "Extract age",
                ResponseFormat::json_schema(
                    "person",
                    json!({"$ref": "https://example.invalid/schema.json"}),
                ),
            )
            .await;

        assert!(matches!(
            result,
            Err(ReactError::StructuredOutput(inner))
                if matches!(inner.as_ref(), StructuredOutputError::InvalidSchema { .. })
        ));
        assert_eq!(llm.call_count(), 0);
    }

    #[tokio::test]
    async fn execute_typed_requires_declared_format_before_execution() {
        let llm = Arc::new(MockLlmClient::new().with_response(r#"{"age":28}"#));
        let mut agent =
            ReactAgent::new(AgentConfig::new("mock-model", "extractor", "Extract data"))
                .with_llm_client(llm.clone());

        let result: Result<serde_json::Value> = agent.execute_typed("Extract age").await;
        assert!(matches!(
            result,
            Err(ReactError::StructuredOutput(inner))
                if matches!(inner.as_ref(), StructuredOutputError::MissingResponseFormat)
        ));
        assert_eq!(llm.call_count(), 0);
    }
}
