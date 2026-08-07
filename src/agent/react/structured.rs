//! Structured Output support
//!
//! Provides a `StructuredAgent<T>` wrapper that automatically parses text output into type `T` after Agent execution.
//!
//! # Usage
//!
//! ```rust,no_run
//! use echo_agent::prelude::*;
//! use echo_agent::agent::react::structured::StructuredAgent;
//! use serde::Deserialize;
//!
//! #[derive(Debug, Deserialize)]
//! struct Sentiment { label: String, score: f64 }
//!
//! # async fn run() -> echo_agent::error::Result<()> {
//! let agent = ReactAgentBuilder::simple("qwen3-max", "You are a sentiment analysis assistant, always output in JSON format")?;
//! let mut sa = StructuredAgent::<Sentiment>::new(agent);
//! let result = sa.execute("I love Rust!").await?;
//! println!("{}: {}", result.label, result.score);
//! # Ok(())
//! # }
//! ```

use crate::agent::Agent;
use crate::error::{ReactError, Result};

use super::ReactAgent;

/// Wrapper that automatically parses Agent text output into type `T`
///
/// Requires the Agent to output valid JSON text (can be constrained via system prompt,
/// or used with `ResponseFormat::JsonSchema`).
pub struct StructuredAgent<T> {
    inner: ReactAgent,
    _phantom: std::marker::PhantomData<T>,
}

impl<T> StructuredAgent<T>
where
    T: serde::de::DeserializeOwned + Send + 'static,
{
    /// Wrap an existing ReactAgent
    pub fn new(agent: ReactAgent) -> Self {
        Self {
            inner: agent,
            _phantom: std::marker::PhantomData,
        }
    }

    /// Get a reference to the inner Agent
    pub fn inner(&self) -> &ReactAgent {
        &self.inner
    }

    /// Get a mutable reference to the inner Agent
    pub fn inner_mut(&mut self) -> &mut ReactAgent {
        &mut self.inner
    }

    /// Execute a task and parse the result as type `T`
    pub async fn execute(&mut self, task: &str) -> Result<T> {
        let text = self.inner.execute(task).await?;
        parse_json_output(&text)
    }

    /// Chat and parse the result as type `T`
    pub async fn chat(&mut self, message: &str) -> Result<T> {
        let text = self.inner.chat(message).await?;
        parse_json_output(&text)
    }
}

/// Extract and parse JSON from LLM output text
///
/// Supports two formats:
/// 1. Pure JSON string
/// 2. JSON wrapped in Markdown code block (```json ... ```)
fn parse_json_output<T: serde::de::DeserializeOwned>(text: &str) -> Result<T> {
    let trimmed = text.trim();

    // Try direct JSON parsing
    if let Ok(v) = serde_json::from_str::<T>(trimmed) {
        return Ok(v);
    }

    // Try extracting JSON from markdown code block
    if let Some(json_str) = extract_json_from_markdown(trimmed)
        && let Ok(v) = serde_json::from_str::<T>(json_str)
    {
        return Ok(v);
    }

    Err(ReactError::Other(format!(
        "Failed to parse LLM output as target type. Raw output:\n{text}"
    )))
}

/// Extract JSON content from a markdown code block
fn extract_json_from_markdown(text: &str) -> Option<&str> {
    // Match ```json\n...\n``` or ```\n...\n```
    let start = if let Some(pos) = text.find("```json") {
        pos + 7
    } else {
        text.find("```")?.checked_add(3)?
    };

    let remaining = &text[start..];
    // Skip newline
    let content_start = remaining.find('\n').map(|p| p + 1).unwrap_or(0);
    let content = &remaining[content_start..];

    // Find closing ```
    content.find("```").map(|end| content[..end].trim())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Deserialize, PartialEq)]
    struct Person {
        name: String,
        age: u32,
    }

    #[test]
    fn test_parse_json_direct() {
        let result: Person = parse_json_output(r#"{"name": "Alice", "age": 30}"#).unwrap();
        assert_eq!(
            result,
            Person {
                name: "Alice".to_string(),
                age: 30
            }
        );
    }

    #[test]
    fn test_parse_json_with_whitespace() {
        let result: Person = parse_json_output("  \n{\"name\": \"Bob\", \"age\": 25}\n  ").unwrap();
        assert_eq!(result.name, "Bob");
    }

    #[test]
    fn test_parse_json_from_markdown() {
        let text = r#"Here is the result:
```json
{"name": "Charlie", "age": 35}
```
"#;
        let result: Person = parse_json_output(text).unwrap();
        assert_eq!(result.name, "Charlie");
        assert_eq!(result.age, 35);
    }

    #[test]
    fn test_parse_json_failure() {
        let result = parse_json_output::<Person>("not json at all");
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_json_from_markdown() {
        let text = "```json\n{\"a\": 1}\n```";
        assert_eq!(extract_json_from_markdown(text), Some("{\"a\": 1}"));
    }

    #[test]
    fn test_extract_json_no_markdown() {
        assert_eq!(extract_json_from_markdown("{\"a\": 1}"), None);
    }
}
