//! Framework-level integration tests.
//!
//! These live in the `tests/` crate (no `#[cfg(test)]`) and exercise real
//! production code paths. They focus on pure (no live LLM), high-value areas:
//! token-calibration convergence (P5) and the shell-safety classifier shared
//! by spawn_task / prompt_exec (S1/S6).
//!
//! Tests that need a mock LLM driving the full ReAct loop require the
//! `testing` feature plus a snapshot-level `LlmClient` field (tracked
//! separately as a larger refactor).

/// The calibrated tokenizer must converge its factor toward the true ratio
/// when fed real usage. Guards the P5 wiring (factor must move off 1.0).
#[tokio::test]
async fn calibrated_tokenizer_converges_on_feedback() {
    use echo_agent::tokenizer::{CalibratedTokenizer, HeuristicTokenizer, Tokenizer};
    use std::sync::Arc;

    let base = Arc::new(HeuristicTokenizer);
    let calibrated = CalibratedTokenizer::with_alpha(base.clone(), 1.0); // instant update

    let text = "hello world this is a calibration test";
    let estimated = base.count_tokens(text);

    // Simulate the real model reporting 2x the heuristic estimate.
    calibrated.calibrate(estimated, (estimated * 2) as u32);

    assert!(
        (calibrated.calibration_factor() - 2.0).abs() < 0.01,
        "factor should converge to 2.0, got {}",
        calibrated.calibration_factor()
    );
    assert_eq!(calibrated.count_tokens(text), estimated * 2);
}

/// The shell-safety classifier (shared by spawn_task and prompt_exec) must
/// reject shell metacharacters and dangerous commands. Guards S1/S6.
#[test]
fn shell_safety_rejects_metacharacters() {
    #[cfg(feature = "shell")]
    {
        use echo_tools::shell::{CommandSafety, validate_command_safety};

        // A pipe / semicolon is an injection vector → rejected.
        let dangerous = validate_command_safety("ls; rm -rf /");
        assert!(
            matches!(dangerous, CommandSafety::Dangerous(_)),
            "metacharacter command must be rejected"
        );

        // A whitelisted read-only command is safe.
        let safe = validate_command_safety("ls -la");
        assert!(
            matches!(safe, CommandSafety::Safe),
            "plain ls should be safe, got {:?}",
            safe
        );
    }
}
