//! demo67_progress.rs — Progress tracking + task metadata (P0 + P1)
//!
//! Demonstrates:
//!   1. `PhasePlan` with weighted phases
//!   2. `ProgressReporter` broadcasting via `watch` channel
//!   3. Product-owned data in `TaskSpec::extension`
//!   4. `TaskEvent::Progress` flowing through a `TaskEventBus`
//!
//! Contract test: `contract_demo67_progress`.

use echo_agent::tasks::{Phase, PhasePlan, ProgressReporter, TaskEvent, TaskEventBus, TaskSpec};
use serde::{Deserialize, Serialize};
use tokio::time::{Duration, sleep};

// ── Typed metadata struct ──────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ResearchParams {
    topic: String,
    max_papers: usize,
    require_peer_review: bool,
}

#[tokio::test]
async fn contract_demo67_progress() -> Result<(), Box<dyn std::error::Error>> {
    println!("🚀 === demo67: Progress Tracking + Task Extension ===\n");

    // ── 1. Build a PhasePlan with 3 weighted phases ────────────────
    let plan = PhasePlan::new(vec![
        Phase::new("search", "🔍 Search", 2.0),
        Phase::new("analyze", "📊 Analyze", 3.0),
        Phase::new("report", "📝 Report", 1.0),
    ]);
    println!(
        "📋 PhasePlan created: {} phases, total weight = {:.1}",
        plan.len(),
        plan.phases.iter().map(|p| p.weight).sum::<f64>()
    );

    // ── 2. ProgressReporter + subscriber ───────────────────────────
    let mut reporter = ProgressReporter::new("task-42".into(), plan);
    let mut sub = reporter.subscribe();

    // ── 3. TaskEventBus + event subscriber ─────────────────────────
    let bus = TaskEventBus::new();
    let mut bus_rx = bus.subscribe();

    // Spawn a subagent that drives progress forward
    let bus_clone = bus.clone();
    let subagent = tokio::spawn(async move {
        // Phase 0: Search
        reporter.enter_phase(0, Some("Querying arxiv…".into()));
        emit_progress(&reporter, &bus_clone);
        for step in 1..=4 {
            sleep(Duration::from_millis(50)).await;
            reporter.update_phase_progress(
                step as f64 / 4.0,
                Some(format!("Found {} papers", step * 5)),
            );
            emit_progress(&reporter, &bus_clone);
        }

        // Phase 1: Analyze
        reporter.enter_phase(1, Some("Running citation analysis…".into()));
        emit_progress(&reporter, &bus_clone);
        for step in 1..=6 {
            sleep(Duration::from_millis(40)).await;
            reporter.update_phase_progress(
                step as f64 / 6.0,
                Some(format!("Analyzed {}/30 papers", step * 5)),
            );
            emit_progress(&reporter, &bus_clone);
        }

        // Phase 2: Report
        reporter.enter_phase(2, Some("Generating summary…".into()));
        emit_progress(&reporter, &bus_clone);
        sleep(Duration::from_millis(60)).await;
        reporter.update_phase_progress(1.0, Some("Report complete".into()));
        emit_progress(&reporter, &bus_clone);
    });

    // ── 4. Print progress from the watch subscriber ───────────────
    let printer = tokio::spawn(async move {
        while let Ok(()) = sub.changed().await {
            {
                let p = sub.borrow();
                let eta = p.eta_secs.map(|s| format!(" ETA {s}s")).unwrap_or_default();
                println!(
                    "  📡 [{}/{}] {} — {:.1}%{}",
                    p.phase_index + 1,
                    p.total_phases,
                    p.current_phase,
                    p.percentage,
                    eta,
                );
                if let Some(msg) = &p.message {
                    println!("      └─ {msg}");
                }
                if p.percentage >= 100.0 {
                    break;
                }
            }
        }
    });

    // Drain a few bus events in parallel
    let bus_printer = tokio::spawn(async move {
        let mut count = 0u32;
        while let Ok(event) = bus_rx.recv().await {
            let TaskEvent::Progress { task_id, progress } = event.as_ref();
            if count.is_multiple_of(3) {
                println!(
                    "  🔔 bus: {} @ {:.1}% ({})",
                    task_id, progress.percentage, progress.current_phase
                );
            }
            count += 1;
        }
    });

    let _ = tokio::join!(subagent, printer);
    // Give the bus printer a moment to drain, then drop the bus so it exits
    sleep(Duration::from_millis(50)).await;
    drop(bus);
    let _ = bus_printer.await;

    // ── 5. Product-owned typed extension ─────────────────────────────────
    println!("\n--- Task Extension ---");
    let extension = serde_json::to_value(ResearchParams {
        topic: "Retrieval-Augmented Generation".into(),
        max_papers: 20,
        require_peer_review: true,
    })?;
    let task = TaskSpec {
        id: "research-01".to_string(),
        title: "RAG literature review".to_string(),
        description: "Literature review on RAG systems".to_string(),
        depends_on: Vec::new(),
        max_retries: 2,
        extension,
    };

    println!("🏷️  Task id: {}", task.id);
    println!("📦 extension: {}", task.extension);

    // ── 6. Decode product data at the application boundary ───────────────
    let params: ResearchParams = serde_json::from_value(task.extension.clone())?;
    println!(
        "✅ extension → topic={:?}, max_papers={}, peer_review={}",
        params.topic, params.max_papers, params.require_peer_review
    );

    println!("\n🎉 === demo67 complete ===");
    Ok(())
}

/// Helper: snapshot the reporter's current progress and emit it as a `TaskEvent::Progress`.
fn emit_progress(reporter: &ProgressReporter, bus: &TaskEventBus) {
    let progress = reporter.current();
    bus.emit(TaskEvent::Progress {
        task_id: progress.task_id.clone(),
        progress,
    });
}
