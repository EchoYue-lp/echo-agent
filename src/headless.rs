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
//! };
//!
//! let result = run_headless(config, |builder| builder).await;
//! println!("{}", result.output);
//! std::process::exit(result.exit_code());
//! # }
//! ```

use crate::agent::Agent;
use crate::agent::react::builder::ReactAgentBuilder;

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
}

impl Default for HeadlessConfig {
    fn default() -> Self {
        Self {
            prompt: String::new(),
            exit_on_error: true,
            output_format: "text".into(),
            max_iterations: None,
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
}

impl HeadlessResult {
    /// Compute the process exit code: 0 on success, 1 on failure.
    pub fn exit_code(&self) -> i32 {
        if self.success { 0 } else { 1 }
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
    if config.prompt.is_empty() {
        return HeadlessResult {
            output: "Error: empty prompt".into(),
            success: false,
            model: String::new(),
            format: config.output_format,
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
            };
        }
    };

    let model = agent.model_name().to_string();

    // Execute the prompt
    match agent.execute(&config.prompt).await {
        Ok(output) => HeadlessResult {
            output,
            success: true,
            model,
            format: config.output_format,
        },
        Err(e) => HeadlessResult {
            output: format!("Error: {}", e),
            success: false,
            model,
            format: config.output_format,
        },
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
    }

    #[test]
    fn test_headless_result_exit_code() {
        let ok = HeadlessResult {
            output: "done".into(),
            success: true,
            model: "test".into(),
            format: "text".into(),
        };
        assert_eq!(ok.exit_code(), 0);

        let fail = HeadlessResult {
            output: "error".into(),
            success: false,
            model: "test".into(),
            format: "text".into(),
        };
        assert_eq!(fail.exit_code(), 1);
    }

    #[test]
    fn test_headless_result_format_json() {
        let result = HeadlessResult {
            output: "hello world".into(),
            success: true,
            model: "test-model".into(),
            format: "json".into(),
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
        };
        assert_eq!(result.format_output(), "hello world");
    }
}
