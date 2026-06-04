//! demo68_human_selection — HumanLoopProvider Selection (task checkpoint) demo
//!
//! Demonstrates the unified human-in-the-loop Selection kind that replaces
//! the legacy HumanGate. A running task can pause and wait for human input
//! via the same HumanLoopProvider used for tool approvals.
//!
//! ```bash
//! cargo run --example demo68_human_gate --features tasks,subagent,human-loop
//! ```

use echo_agent::human_loop::{
    ApprovalDecision, HumanLoopEvent, HumanLoopManager, HumanLoopRequest, HumanLoopResponse,
};
use std::sync::Arc;
use std::time::Duration;

#[tokio::main]
async fn main() {
    println!("=== demo68: HumanLoopProvider Selection ===\n");

    // ── Scenario 1: Selection via HumanLoopManager event loop ─────────
    println!("--- Scenario 1: Selection via Manager events ---");
    let manager = Arc::new(HumanLoopManager::new());

    // Spawn event handler (simulates frontend)
    let mgr = manager.clone();
    let handler = tokio::spawn(async move {
        while let Some(event) = mgr.recv_event().await {
            match event {
                HumanLoopEvent::SelectionRequest {
                    task_id,
                    prompt,
                    options,
                    phase,
                    responder,
                    ..
                } => {
                    println!("  👀 [Frontend]  task={task_id}, phase={phase}");
                    println!("  📋 [Frontend]  \"{prompt}\"");
                    for (i, opt) in options.iter().enumerate() {
                        println!("  [{i}] {opt}");
                    }
                    // Auto-select "Approve"
                    responder.respond("Approve".to_string(), None);
                    println!("  📨 [Frontend]  responded: Approve");
                }
                _ => {}
            }
        }
    });

    // Task requests selection
    let req = HumanLoopRequest::selection(
        "task-1",
        "Review the draft and choose an action",
        vec!["Approve".into(), "Revise".into(), "Cancel".into()],
    )
    .with_context(serde_json::json!({ "draft": "Hello, world!" }))
    .with_phase("review");

    println!("  🚀 [Task]      requesting selection...");
    match manager.request(req).await {
        Ok(HumanLoopResponse::Selection {
            selection,
            instructions,
        }) => {
            println!("  ✅ [Task]      selection={selection}, instructions={instructions:?}");
        }
        Ok(other) => println!("  ⚠️  [Task]      unexpected response: {other:?}"),
        Err(e) => println!("  ❌ [Task]      error: {e}"),
    }

    drop(manager);
    let _ = handler.await;

    // ── Scenario 2: Timeout ──────────────────────────────────────────
    println!("\n--- Scenario 2: Selection timeout ---");
    let manager2 = Arc::new(HumanLoopManager::new());

    // Spawn handler that never responds (simulates no frontend)
    let mgr2 = manager2.clone();
    let handler2 = tokio::spawn(async move {
        while let Some(_event) = mgr2.recv_event().await {
            // Intentionally not responding to trigger timeout
        }
    });

    let req2 = HumanLoopRequest::selection(
        "task-2",
        "Approve deployment",
        vec!["Deploy".into(), "Abort".into()],
    );
    // Use the approval_with_timeout pattern adapted for selection
    let mut req2 = req2;
    req2.timeout = Some(Duration::from_millis(200));

    println!("  🚀 [Task-2]    requesting with 200ms timeout...");
    match manager2.request(req2).await {
        Ok(HumanLoopResponse::Timeout) => println!("  ⏰ [Task-2]    timed out (as expected)"),
        Ok(other) => println!("  ⚠️  [Task-2]    unexpected: {other:?}"),
        Err(e) => println!("  ❌ [Task-2]    error: {e}"),
    }

    drop(manager2);
    handler2.abort();

    // ── Scenario 3: Approval kind still works ────────────────────────
    println!("\n--- Scenario 3: Approval kind (backward compat) ---");
    let manager3 = Arc::new(HumanLoopManager::new());

    let mgr3 = manager3.clone();
    let handler3 = tokio::spawn(async move {
        while let Some(event) = mgr3.recv_event().await {
            match event {
                HumanLoopEvent::ApprovalRequest {
                    tool_name,
                    responder,
                    ..
                } => {
                    println!("  👀 [Frontend]  approval for tool: {tool_name}");
                    responder.respond(ApprovalDecision::Approved);
                }
                _ => {}
            }
        }
    });

    let req3 = HumanLoopRequest::approval("write_file", serde_json::json!({"path": "test.rs"}));
    println!("  🚀 [Task-3]    requesting approval...");
    match manager3.request(req3).await {
        Ok(HumanLoopResponse::Approved) => println!("  ✅ [Task-3]    approved"),
        Ok(other) => println!("  ⚠️  [Task-3]    unexpected: {other:?}"),
        Err(e) => println!("  ❌ [Task-3]    error: {e}"),
    }

    drop(manager3);
    let _ = handler3.await;

    println!("\n🎉 === demo68 complete ===");
}
