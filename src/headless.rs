//! Headless mode — run a single prompt, print output, exit.
//!
//! Designed for CI/CD pipelines, scripting, and non-interactive automation.
//! The agent runs a single prompt, collects the output, and returns a
//! structured result that the caller can print and exit on.
//!
//! # Example
//!
//! ```rust,no_run
//! use echo_agent::headless::{HeadlessConfig, run_headless};
//!
//! # #[tokio::main]
//! # async fn main() {
//! let config = HeadlessConfig {
//!     prompt: "List all Rust files in the project".into(),
//!     exit_on_error: true,
//!     output_format: "text".into(),
//!     max_iterations: Some(10),
//!     cancel_token: None,
//! };
//!
//! let result = run_headless(config, |builder| builder).await;
//! println!("{}", result.output);
//! std::process::exit(result.exit_code());
//! # }
//! ```

use crate::agent::Agent;
use crate::agent::AgentEvent;
use crate::agent::react::builder::ReactAgentBuilder;
use futures::StreamExt;
use tokio_util::sync::CancellationToken;

/// Configuration for headless (non-interactive) agent execution.
pub struct HeadlessConfig {
    /// The prompt to execute.
    pub prompt: String,

    /// Exit with error if the agent reports failure.
    pub exit_on_error: bool,

    /// Output format: `"text"` (default) or `"json"`.
    pub output_format: String,

    /// Max iterations before forcing stop (safety limit).
    pub max_iterations: Option<usize>,

    /// Optional caller-owned cancellation token.
    pub cancel_token: Option<CancellationToken>,
}

impl Default for HeadlessConfig {
    fn default() -> Self {
        Self {
            prompt: String::new(),
            exit_on_error: true,
            output_format: "text".into(),
            max_iterations: None,
            cancel_token: None,
        }
    }
}

/// Result of a headless execution.
pub struct HeadlessResult {
    /// The agent's final output text.
    pub output: String,

    /// Whether the execution succeeded.
    pub success: bool,

    /// Model name used.
    pub model: String,

    /// Output format requested.
    pub format: String,

    /// Whether a failed run should produce a non-zero process exit code.
    pub exit_on_error: bool,
}

impl HeadlessResult {
    /// Compute the process exit code: 0 on success, 1 on failure.
    pub fn exit_code(&self) -> i32 {
        if self.success || !self.exit_on_error {
            0
        } else {
            1
        }
    }

    /// Format the result for stdout according to the requested output format.
    pub fn format_output(&self) -> String {
        match self.format.as_str() {
            "json" => {
                let json = serde_json::json!({
                    "success": self.success,
                    "model": self.model,
                    "output": self.output,
                });
                serde_json::to_string_pretty(&json).unwrap_or_else(|_| self.output.clone())
            }
            _ => self.output.clone(),
        }
    }
}

/// Run the agent in headless mode.
///
/// The `configure` closure receives a [`ReactAgentBuilder`] so the caller can
/// set model, system prompt, tools, etc. before the agent is built and run.
///
/// # Arguments
///
/// * `config` — headless execution parameters (prompt, format, limits)
/// * `configure` — closure to customize the agent builder
///
/// # Returns
///
/// A [`HeadlessResult`] with the agent output, success flag, and metadata.
pub async fn run_headless<F>(config: HeadlessConfig, configure: F) -> HeadlessResult
where
    F: FnOnce(ReactAgentBuilder) -> ReactAgentBuilder,
{
    let exit_on_error = config.exit_on_error;
    if config.prompt.is_empty() {
        return HeadlessResult {
            output: "Error: empty prompt".into(),
            success: false,
            model: String::new(),
            format: config.output_format,
            exit_on_error,
        };
    }

    // Build the agent
    let builder = ReactAgentBuilder::new();
    let builder = configure(builder);

    // Apply max_iterations if set
    let builder = if let Some(max) = config.max_iterations {
        builder.max_iterations(max)
    } else {
        builder
    };

    let agent = match builder.build() {
        Ok(a) => a,
        Err(e) => {
            return HeadlessResult {
                output: format!("Error building agent: {}", e),
                success: false,
                model: String::new(),
                format: config.output_format,
                exit_on_error,
            };
        }
    };

    let model = agent.model_name().to_string();

    let cancel = config.cancel_token.unwrap_or_default();
    let stream = match agent
        .execute_stream_with_cancel(&config.prompt, cancel)
        .await
    {
        Ok(stream) => stream,
        Err(error) => {
            return HeadlessResult {
                output: format!("Error: {error}"),
                success: false,
                model,
                format: config.output_format,
                exit_on_error,
            };
        }
    };
    futures::pin_mut!(stream);
    while let Some(event) = stream.next().await {
        match event {
            Ok(AgentEvent::FinalAnswer(output)) => {
                return HeadlessResult {
                    output,
                    success: true,
                    model,
                    format: config.output_format,
                    exit_on_error,
                };
            }
            Ok(AgentEvent::Cancelled) => {
                return HeadlessResult {
                    output: "Cancelled".to_string(),
                    success: false,
                    model,
                    format: config.output_format,
                    exit_on_error,
                };
            }
            Ok(AgentEvent::Error {
                source, message, ..
            }) => {
                return HeadlessResult {
                    output: format!("Error ({source}): {message}"),
                    success: false,
                    model,
                    format: config.output_format,
                    exit_on_error,
                };
            }
            Err(error) => {
                return HeadlessResult {
                    output: format!("Error: {error}"),
                    success: false,
                    model,
                    format: config.output_format,
                    exit_on_error,
                };
            }
            Ok(_) => {}
        }
    }

    HeadlessResult {
        output: "Error: agent event stream ended without a terminal event".to_string(),
        success: false,
        model,
        format: config.output_format,
        exit_on_error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_headless_config_default() {
        let config = HeadlessConfig::default();
        assert!(config.prompt.is_empty());
        assert!(config.exit_on_error);
        assert_eq!(config.output_format, "text");
        assert!(config.max_iterations.is_none());
        assert!(config.cancel_token.is_none());
    }

    #[test]
    fn test_headless_result_exit_code() {
        let ok = HeadlessResult {
            output: "done".into(),
            success: true,
            model: "test".into(),
            format: "text".into(),
            exit_on_error: true,
        };
        assert_eq!(ok.exit_code(), 0);

        let fail = HeadlessResult {
            output: "error".into(),
            success: false,
            model: "test".into(),
            format: "text".into(),
            exit_on_error: true,
        };
        assert_eq!(fail.exit_code(), 1);

        let tolerated_failure = HeadlessResult {
            output: "error".into(),
            success: false,
            model: "test".into(),
            format: "text".into(),
            exit_on_error: false,
        };
        assert_eq!(tolerated_failure.exit_code(), 0);
    }

    #[test]
    fn test_headless_result_format_json() {
        let result = HeadlessResult {
            output: "hello world".into(),
            success: true,
            model: "test-model".into(),
            format: "json".into(),
            exit_on_error: true,
        };
        let formatted = result.format_output();
        assert!(formatted.contains("\"success\": true"));
        assert!(formatted.contains("\"model\": \"test-model\""));
        assert!(formatted.contains("hello world"));
    }

    #[test]
    fn test_headless_result_format_text() {
        let result = HeadlessResult {
            output: "hello world".into(),
            success: true,
            model: "test-model".into(),
            format: "text".into(),
            exit_on_error: true,
        };
        assert_eq!(result.format_output(), "hello world");
    }
}
