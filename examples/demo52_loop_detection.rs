//! demo52 — Loop Detection
//!
//! Demonstrates the `LoopDetector` which catches three common failure modes
//! in agent execution:
//!
//! 1. **Exact-duplicate loop** — same `(tool_name, args)` called N times
//! 2. **Same-tool failure streak** — same tool fails N consecutive times
//! 3. **No-progress loop** — N iterations without file writes or task updates
//!
//! All logic is local — no LLM calls needed.
//!
//! ```sh
//! cargo run --example demo52_loop_detection
//! ```

use echo_agent::agent::react::loop_detector::{LoopDetector, LoopDetectorConfig, LoopVerdict};

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
    println!("║       echo-agent  Loop Detection Demo            ║");
    println!("║  (no LLM calls — pure local logic)               ║");
    println!("╚══════════════════════════════════════════════════╝");

    demo_custom_config();
    demo_exact_duplicate();
    demo_failure_streak();
    demo_no_progress();
    demo_progress_resets_counter();
    demo_reset();

    println!("\n╔══════════════════════════════════════════════════╗");
    println!("║  All 6 scenarios passed ✅                       ║");
    println!("╚══════════════════════════════════════════════════╝");
}

/// Scenario 1: Create a LoopDetector with custom thresholds
fn demo_custom_config() {
    section!(1, "Custom LoopDetectorConfig");

    let config = LoopDetectorConfig {
        exact_threshold: 2,
        failure_threshold: 4,
        no_progress_threshold: 6,
    };
    println!("  Created config:");
    println!("    exact_threshold     = {}", config.exact_threshold);
    println!("    failure_threshold   = {}", config.failure_threshold);
    println!(
        "    no_progress_threshold = {}",
        config.no_progress_threshold
    );

    let detector = LoopDetector::new(config);
    assert_eq!(detector.check(), LoopVerdict::Continue);
    println!("  ✅ Fresh detector reports Continue");

    // Also show the defaults
    let default_config = LoopDetectorConfig::default();
    println!("\n  Default config:");
    println!(
        "    exact_threshold     = {}",
        default_config.exact_threshold
    );
    println!(
        "    failure_threshold   = {}",
        default_config.failure_threshold
    );
    println!(
        "    no_progress_threshold = {}",
        default_config.no_progress_threshold
    );
}

/// Scenario 2: Exact-duplicate detection
fn demo_exact_duplicate() {
    section!(2, "Exact-Duplicate Loop Detection");

    // Use threshold = 3: the same (tool, args) called 3 times triggers Break
    let config = LoopDetectorConfig {
        exact_threshold: 3,
        ..Default::default()
    };
    let mut detector = LoopDetector::new(config);

    let args = r#"{"path": "src/main.rs"}"#;

    // First two calls — fine
    detector.record_tool_call("read_file", args, true);
    println!("  Call 1: read_file({}) → success", args);
    println!("    verdict: {:?}", detector.check());

    detector.record_tool_call("read_file", args, true);
    println!("  Call 2: read_file({}) → success", args);
    println!("    verdict: {:?}", detector.check());

    // Third call — triggers Break
    detector.record_tool_call("read_file", args, true);
    println!("  Call 3: read_file({}) → success", args);
    let verdict = detector.check();
    println!("    verdict: {:?}", verdict);

    match &verdict {
        LoopVerdict::Break(msg) => {
            println!("  ✅ Break triggered: {}", msg);
            assert!(msg.contains("Loop detected"));
        }
        other => panic!("Expected Break, got {:?}", other),
    }

    // Show that different args don't count as duplicates
    detector.reset();
    detector.record_tool_call("read_file", r#"{"path": "a.rs"}"#, true);
    detector.record_tool_call("read_file", r#"{"path": "b.rs"}"#, true);
    detector.record_tool_call("read_file", r#"{"path": "c.rs"}"#, true);
    assert_eq!(detector.check(), LoopVerdict::Continue);
    println!("\n  Different args each time → no duplicate detection ✅");
}

/// Scenario 3: Same-tool failure streak
fn demo_failure_streak() {
    section!(3, "Same-Tool Failure Streak Detection");

    let config = LoopDetectorConfig {
        failure_threshold: 3,
        ..Default::default()
    };
    let mut detector = LoopDetector::new(config);

    // Use different args to avoid exact-duplicate detection
    for i in 0..2 {
        let args = format!(r#"{{"cmd": "bad_command_{}"}}"#, i);
        detector.record_tool_call("shell", &args, false);
        println!("  Call {}: shell({}) → FAIL", i + 1, args);
        println!("    verdict: {:?}", detector.check());
    }

    // Third failure triggers Warn
    let args = r#"{"cmd": "bad_command_2"}"#;
    detector.record_tool_call("shell", args, false);
    println!("  Call 3: shell({}) → FAIL", args);
    let verdict = detector.check();
    println!("    verdict: {:?}", verdict);

    match &verdict {
        LoopVerdict::Warn(msg) => {
            println!("  ✅ Warn triggered: {}", msg);
            assert!(msg.contains("failed 3 times"));
        }
        other => panic!("Expected Warn, got {:?}", other),
    }

    // Show that a success resets the failure streak
    detector.reset();
    detector.record_tool_call("shell", "cmd_0", false);
    detector.record_tool_call("shell", "cmd_1", false);
    detector.record_tool_call("shell", "cmd_2", true); // success resets
    detector.record_tool_call("shell", "cmd_3", false);
    detector.record_tool_call("shell", "cmd_4", false);
    assert_eq!(detector.check(), LoopVerdict::Continue);
    println!("\n  Success in between resets streak → Continue ✅");
}

/// Scenario 4: No-progress loop detection
fn demo_no_progress() {
    section!(4, "No-Progress Loop Detection");

    let config = LoopDetectorConfig {
        no_progress_threshold: 5,
        ..Default::default()
    };
    let mut detector = LoopDetector::new(config);

    // Read-only tool calls don't count as "progress"
    for i in 0..5 {
        let args = format!(r#"{{"path": "file_{}.rs"}}"#, i);
        detector.record_tool_call("read_file", &args, true);
        detector.record_iteration();
        println!("  Iteration {}: read_file → no progress", i + 1);
    }

    let verdict = detector.check();
    println!("    verdict: {:?}", verdict);

    match &verdict {
        LoopVerdict::Warn(msg) => {
            println!("  ✅ Warn triggered: {}", msg);
            assert!(msg.contains("No progress"));
        }
        other => panic!("Expected Warn, got {:?}", other),
    }

    // List of tools that DO count as progress:
    println!("\n  Tools that reset the progress counter:");
    println!("    edit_file, write_file, create_file, delete_file,");
    println!("    create_task, update_task, git_commit, shell");
}

/// Scenario 5: Progress resets the no-progress counter
fn demo_progress_resets_counter() {
    section!(5, "Progress Resets the Counter");

    let config = LoopDetectorConfig {
        no_progress_threshold: 5,
        ..Default::default()
    };
    let mut detector = LoopDetector::new(config);

    // 4 iterations without progress (one short of threshold)
    for i in 0..4 {
        let args = format!(r#"{{"path": "file_{}.rs"}}"#, i);
        detector.record_tool_call("read_file", &args, true);
        detector.record_iteration();
    }
    println!("  4 read-only iterations (threshold = 5)");

    // Now make progress with a write
    detector.record_tool_call("write_file", r#"{"path": "output.rs"}"#, true);
    detector.record_iteration();
    println!("  write_file → progress! Counter resets.");

    // 4 more read-only iterations (counter was at 1 after write's iteration)
    for i in 0..3 {
        let args = format!(r#"{{"path": "another_{}.rs"}}"#, i);
        detector.record_tool_call("read_file", &args, true);
        detector.record_iteration();
    }
    println!("  3 more read-only iterations (total 4 since write)");

    // Should still be Continue — the write reset the counter
    let verdict = detector.check();
    assert_eq!(verdict, LoopVerdict::Continue);
    println!("    verdict: {:?} ✅", verdict);
}

/// Scenario 6: Reset clears all state
fn demo_reset() {
    section!(6, "Reset Clears All State");

    let mut detector = LoopDetector::new(LoopDetectorConfig::default());

    // Build up some state
    let args = r#"{"path": "x"}"#;
    for _ in 0..2 {
        detector.record_tool_call("read_file", args, true);
    }
    for i in 0..5 {
        detector.record_tool_call("read_file", &format!("f{}", i), true);
        detector.record_iteration();
    }
    println!("  Built up state: 2 duplicate calls + 5 iterations");

    // Not yet at threshold
    assert_eq!(detector.check(), LoopVerdict::Continue);

    // Reset everything
    detector.reset();
    println!("  Called detector.reset()");

    // Verify clean slate
    assert_eq!(detector.check(), LoopVerdict::Continue);
    println!("  ✅ All counters cleared, verdict is Continue");
}
