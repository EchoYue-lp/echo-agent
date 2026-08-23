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
use crate::agent::react::builder::ReactAgentBuilder;
use echo_orchestration::runtime::{
    AgentTurnDriver, EventSink, SinkControl, TurnMode, TurnOutcome, TurnRequest,
};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

struct HeadlessEventSink;

impl EventSink for HeadlessEventSink {
    fn on_event(
        &self,
        _envelope: &crate::agent::EventEnvelope,
    ) -> crate::error::Result<SinkControl> {
        Ok(SinkControl::Continue)
    }
}

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
    let identity_value = format!("headless-{}", uuid::Uuid::new_v4());
    let identity = match crate::agent::EventIdentity::new(&identity_value, &identity_value) {
        Ok(identity) => identity,
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
    let request = TurnRequest::new(identity, config.prompt)
        .mode(TurnMode::Execute)
        .cancel(cancel);
    let receipt = AgentTurnDriver
        .drive(
            Arc::new(agent) as Arc<dyn Agent>,
            request,
            &HeadlessEventSink,
        )
        .await;
    let (output, success) = match (receipt.outcome, receipt.final_answer) {
        (TurnOutcome::Completed, Some(output)) => (output, true),
        (TurnOutcome::Completed, None) => (
            "Error: completed turn did not include a final answer".to_string(),
            false,
        ),
        (TurnOutcome::Cancelled, _) => ("Cancelled".to_string(), false),
        (TurnOutcome::Failed(failure), _) => (
            format!("Error ({}): {}", failure.code, failure.message),
            false,
        ),
    };
    HeadlessResult {
        output,
        success,
        model,
        format: config.output_format,
        exit_on_error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::MockLlmClient;

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

    #[tokio::test]
    async fn run_headless_uses_shared_turn_driver_terminal_contract() {
        let llm = Arc::new(MockLlmClient::new().with_response("driver result"));
        let result = run_headless(
            HeadlessConfig {
                prompt: "run through the shared driver".to_string(),
                ..HeadlessConfig::default()
            },
            |builder| builder.llm_client(llm).system_prompt("test"),
        )
        .await;

        assert!(result.success);
        assert_eq!(result.output, "driver result");
        assert_eq!(result.exit_code(), 0);
    }
}
