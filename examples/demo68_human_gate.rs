//! demo68_human_gate — HumanGate (P2) checkpoint demo
//!
//! Demonstrates the human-in-the-loop gate that pauses a running task
//! until a frontend provides approval, revision, or cancellation.
//!
//! ```bash
//! cargo run --example demo68_human_gate --features tasks,subagent
//! ```

use echo_agent::tasks::{HumanGate, HumanRequest, HumanResponse};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

fn sample_request(prompt: &str, phase: &str) -> HumanRequest {
    HumanRequest {
        prompt: prompt.into(),
        context: serde_json::json!({ "draft": "Hello, world!" }),
        options: vec!["Approve".into(), "Revise".into(), "Cancel".into()],
        phase: phase.into(),
    }
}

#[tokio::main]
async fn main() {
    println!("=== demo68: HumanGate (P2) ===\n");

    // ── Scenario 1: normal request -> respond flow ──────────────────
    println!("--- Scenario 1: Request / Respond ---");
    let gate = HumanGate::new();
    let cancel = CancellationToken::new();

    let gate_task = gate.clone();
    let cancel_task = cancel.clone();
    let task_handle = tokio::spawn(async move {
        println!("  🚀 [Task]      started");
        println!("  ⏸️  [Task]      pausing for human review...");
        let resp = gate_task
            .request(
                "task-1",
                sample_request("Review the draft", "review"),
                &cancel_task,
            )
            .await;
        match resp {
            Ok(r) => println!("  ✅ [Task]      resumed! selection = {:?}", r.selection),
            Err(e) => println!("  ❌ [Task]      error: {e}"),
        }
    });

    tokio::time::sleep(Duration::from_millis(50)).await;

    let gate_fe = gate.clone();
    let fe_handle = tokio::spawn(async move {
        let pending = gate_fe.pending().await;
        println!("  👀 [Frontend]  sees {} pending request(s)", pending.len());
        for (id, req) in &pending {
            println!(
                "  📋 [Frontend]    - {id}: \"{}\" (phase={})",
                req.prompt, req.phase
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        let ok = gate_fe
            .respond(
                "task-1",
                HumanResponse {
                    selection: "Approve".into(),
                    instructions: None,
                },
            )
            .await;
        println!("  📨 [Frontend]  responded (ok={ok})");
    });

    let _ = tokio::join!(task_handle, fe_handle);

    // ── Scenario 2: cancellation ────────────────────────────────────
    println!("\n--- Scenario 2: Cancellation ---");
    let gate2 = HumanGate::new();
    let cancel2 = CancellationToken::new();

    let gate2_task = gate2.clone();
    let cancel2_task = cancel2.clone();
    let task2_handle = tokio::spawn(async move {
        println!("  🚀 [Task-2]    started, requesting human input...");
        let resp = gate2_task
            .request(
                "task-2",
                sample_request("Approve deployment", "deploy"),
                &cancel2_task,
            )
            .await;
        match resp {
            Ok(r) => println!("  ⚠️  [Task-2]    unexpected resume: {:?}", r.selection),
            Err(_) => println!("  🛑 [Task-2]    cancelled (as expected)"),
        }
    });

    tokio::time::sleep(Duration::from_millis(50)).await;
    println!("  🎛️  [Ctrl]      cancelling task-2...");
    cancel2.cancel();

    let _ = task2_handle.await;
    println!(
        "  🧹 [Ctrl]      pending count = {}",
        gate2.pending_count().await
    );

    // ── Scenario 3: respond to non-existent task ────────────────────
    println!("\n--- Scenario 3: Respond to non-existent task ---");
    let gate3 = HumanGate::new();
    let ok = gate3
        .respond(
            "ghost",
            HumanResponse {
                selection: "Approve".into(),
                instructions: Some("no one here".into()),
            },
        )
        .await;
    println!("  👻 respond(\"ghost\", ...) -> ok={ok}  (no pending request)");

    println!("\n🎉 === demo68 complete ===");
}
