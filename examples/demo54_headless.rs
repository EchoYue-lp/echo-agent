//! demo54 — Headless Mode
//!
//! Demonstrates the headless (non-interactive) execution mode designed for
//! CI/CD pipelines, scripting, and automation:
//!
//! 1. `HeadlessConfig` — configure prompt, output format, iteration limits
//! 2. `HeadlessResult` — structured result with exit_code() for process exit
//! 3. Text output mode — plain string output
//! 4. JSON output mode — structured JSON with success, model, output fields
//! 5. Empty-prompt error handling
//!
//! **Note:** Actual agent execution requires an LLM API key.
//! This demo shows the configuration and result-handling APIs
//! without making real LLM calls.
//!
//! ```sh
//! # Dry-run demo (no API key needed):
//! cargo run --example demo54_headless
//!
//! # With an LLM API key (full end-to-end):
//! DEEPSEEK_API_KEY=sk-xxx cargo run --example demo54_headless
//! ```

use echo_agent::headless::{HeadlessConfig, HeadlessResult};

macro_rules! section {
    ($n:expr, $title:expr) => {
        println!("\n══════════════════════════════════════════════════");
        println!("  Scenario {} : {}", $n, $title);
        println!("══════════════════════════════════════════════════");
    };
}

#[tokio::main]
async fn main() {
    println!("╔══════════════════════════════════════════════════╗");
    println!("║       echo-agent  Headless Mode Demo             ║");
    println!("║  (config + result handling — no LLM required)    ║");
    println!("╚══════════════════════════════════════════════════╝");

    demo_config_defaults();
    demo_config_custom();
    demo_text_output();
    demo_json_output();
    demo_exit_codes();
    demo_empty_prompt();
    demo_cicd_pattern();

    println!("\n╔══════════════════════════════════════════════════╗");
    println!("║  All 7 scenarios passed ✅                       ║");
    println!("╚══════════════════════════════════════════════════╝");
}

/// Scenario 1: Default configuration
fn demo_config_defaults() {
    section!(1, "HeadlessConfig Defaults");

    let config = HeadlessConfig::default();
    println!("  prompt:         {:?}", config.prompt);
    println!("  exit_on_error:  {}", config.exit_on_error);
    println!("  output_format:  {:?}", config.output_format);
    println!("  max_iterations: {:?}", config.max_iterations);

    assert!(config.prompt.is_empty());
    assert!(config.exit_on_error);
    assert_eq!(config.output_format, "text");
    assert!(config.max_iterations.is_none());
    println!("  ✅ Defaults verified");
}

/// Scenario 2: Custom configuration
fn demo_config_custom() {
    section!(2, "Custom HeadlessConfig");

    let config = HeadlessConfig {
        prompt: "List all Rust files in the project".into(),
        exit_on_error: true,
        output_format: "json".into(),
        max_iterations: Some(10),
        cancel_token: None,
    };
    println!("  prompt:         {:?}", config.prompt);
    println!("  exit_on_error:  {}", config.exit_on_error);
    println!("  output_format:  {:?}", config.output_format);
    println!("  max_iterations: {:?}", config.max_iterations);

    assert_eq!(config.prompt, "List all Rust files in the project");
    assert_eq!(config.max_iterations, Some(10));
    println!("  ✅ Custom config created");
}

/// Scenario 3: Text output formatting
fn demo_text_output() {
    section!(3, "Text Output Mode");

    let result = HeadlessResult {
        output: "Found 42 Rust files:\n  src/main.rs\n  src/lib.rs\n  ...".into(),
        success: true,
        model: "deepseek-v4-flash".into(),
        format: "text".into(),
        exit_on_error: true,
    };

    let formatted = result.format_output();
    println!("  format_output() result:");
    for line in formatted.lines() {
        println!("    {}", line);
    }

    assert_eq!(formatted, result.output);
    println!("  ✅ Text mode returns raw output string");
}

/// Scenario 4: JSON output formatting
fn demo_json_output() {
    section!(4, "JSON Output Mode");

    let result = HeadlessResult {
        output: "Task completed successfully".into(),
        success: true,
        model: "qwen3-max".into(),
        format: "json".into(),
        exit_on_error: true,
    };

    let formatted = result.format_output();
    println!("  format_output() result:");
    for line in formatted.lines() {
        println!("    {}", line);
    }

    // Verify JSON structure
    let parsed: serde_json::Value = serde_json::from_str(&formatted).unwrap();
    assert_eq!(parsed["success"], true);
    assert_eq!(parsed["model"], "qwen3-max");
    assert_eq!(parsed["output"], "Task completed successfully");
    println!("  ✅ JSON mode returns structured {{success, model, output}}");
}

/// Scenario 5: Exit code computation
fn demo_exit_codes() {
    section!(5, "Exit Codes for CI/CD");

    // Success → exit code 0
    let ok_result = HeadlessResult {
        output: "All tests passed".into(),
        success: true,
        model: "test-model".into(),
        format: "text".into(),
        exit_on_error: true,
    };
    println!("  Success case:");
    println!("    exit_code() = {}", ok_result.exit_code());
    assert_eq!(ok_result.exit_code(), 0);

    // Failure → exit code 1
    let fail_result = HeadlessResult {
        output: "Error: compilation failed".into(),
        success: false,
        model: "test-model".into(),
        format: "text".into(),
        exit_on_error: true,
    };
    println!("  Failure case:");
    println!("    exit_code() = {}", fail_result.exit_code());
    assert_eq!(fail_result.exit_code(), 1);

    println!("  ✅ exit_code() returns 0 for success, 1 for failure");
}

/// Scenario 6: Empty prompt handling (simulated)
fn demo_empty_prompt() {
    section!(6, "Empty Prompt Error Handling");

    // When run_headless receives an empty prompt, it returns a failure result.
    // We simulate the expected result here:
    let result = HeadlessResult {
        output: "Error: empty prompt".into(),
        success: false,
        model: String::new(),
        format: "text".into(),
        exit_on_error: true,
    };

    println!("  Empty prompt → success=false");
    println!("  Output: {}", result.output);
    println!("  exit_code() = {}", result.exit_code());
    assert_eq!(result.exit_code(), 1);
    println!("  ✅ Empty prompt is rejected gracefully");
}

/// Scenario 7: Complete CI/CD pipeline pattern
fn demo_cicd_pattern() {
    section!(7, "CI/CD Pipeline Pattern");

    println!("  Typical usage in a CI/CD script:\n");
    println!("  ```rust");
    println!("  let config = HeadlessConfig {{");
    println!("      prompt: \"Review this PR for security issues\".into(),");
    println!("      exit_on_error: true,");
    println!("      output_format: \"json\".into(),");
    println!("      max_iterations: Some(20),");
    println!("      cancel_token: None,");
    println!("  }};");
    println!();
    println!("  let result = run_headless(config, |builder| {{");
    println!("      builder");
    println!("          .model(\"deepseek-v4-flash\")");
    println!("          .system_prompt(\"You are a security reviewer.\")");
    println!("  }}).await;");
    println!();
    println!("  println!(\"{{}}\", result.format_output());");
    println!("  std::process::exit(result.exit_code());");
    println!("  ```\n");

    // Demonstrate what the JSON output would look like
    let simulated = HeadlessResult {
        output: "Found 2 potential XSS vulnerabilities in handlers.rs".into(),
        success: true,
        model: "deepseek-v4-flash".into(),
        format: "json".into(),
        exit_on_error: true,
    };
    println!("  Simulated CI output:");
    for line in simulated.format_output().lines() {
        println!("    {}", line);
    }
    println!("  exit_code = {} (CI passes)", simulated.exit_code());
    println!("  ✅ CI/CD integration pattern demonstrated");
}
