//! demo70_scheduler.rs — Cron task scheduler demo
//!
//! Demonstrates the scheduler module: CronTask definitions, CronTaskStore
//! persistence, and SchedulerRunner management API.
//!
//! Run: cargo run -p echo-agent-learning --example demo70_scheduler

use echo_agent::scheduler::{CronTask, CronTaskStatus, CronTaskStore, FireFn, SchedulerRunner};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("⏰ === demo70: Scheduler Module ===\n");

    // ── 1. Create a file-based CronTaskStore (temp path) ──────────────
    let tmp = tempfile::tempdir()?;
    let store_path = tmp.path().join("tasks.json");
    let store = CronTaskStore::new().with_path(store_path.clone());
    println!("📁 Store path: {}", store_path.display());

    // ── 2. Add two CronTasks with different schedules ─────────────────
    let daily = CronTask::new("daily-backup", "0 2 * * *", "Run nightly database backup");
    let weekly = CronTask::new(
        "weekly-report",
        "0 9 * * 1",
        "Generate weekly analytics report",
    );
    store.add(daily.clone()).await?;
    store.add(weekly.clone()).await?;
    println!(
        "✅ Added 2 cron tasks: '{}' and '{}'\n",
        daily.name, weekly.name
    );

    // ── 3. Validate cron expressions ──────────────────────────────────
    println!("🔍 Cron validation:");
    for t in [&daily, &weekly] {
        println!(
            "   '{}' ({}) → valid: {}",
            t.name,
            t.cron_expr,
            t.validate_cron()
        );
    }
    let bad = CronTask::new("bad-task", "not a cron", "test");
    println!(
        "   '{}' ({}) → valid: {}\n",
        bad.name,
        bad.cron_expr,
        bad.validate_cron()
    );

    // ── 4. Show next_run() calculation ────────────────────────────────
    println!("📅 Next run times:");
    for t in [&daily, &weekly] {
        if let Some(next) = t.next_run() {
            println!("   '{}' → {}", t.name, next.format("%Y-%m-%d %H:%M %Z"));
        }
    }
    println!("   '{}' → {:?}\n", bad.name, bad.next_run());

    // ── 5. Create a SchedulerRunner with a simple fire function ───────
    let cancel = CancellationToken::new();
    let fire_fn: FireFn = Arc::new(|task: CronTask| {
        let name = task.name.clone();
        Box::pin(async move {
            println!("   🔥 Firing task: {}", name);
            Ok(format!("Executed: {}", name))
        })
    });
    let runner = Arc::new(SchedulerRunner::new(store, cancel, fire_fn).await?);
    println!("🤖 SchedulerRunner created (fire fn prints task name)\n");

    // ── 6. Manually trigger a task via run_once() ─────────────────────
    println!("▶️  Manual trigger (run_once):");
    let result = runner.run_once(&daily.id).await?;
    println!("   Result: {}\n", result);

    // ── 7. list_tasks(), set_status(), remove_task() ──────────────────
    println!("📋 All tasks:");
    for t in runner.list_tasks().await {
        println!("   [{:?}] {} — {}", t.status, t.name, t.cron_expr);
    }

    println!("\n🚫 Disabling '{}'...", daily.name);
    runner
        .set_status(&daily.id, CronTaskStatus::Disabled)
        .await?;

    println!("📋 Tasks after disable:");
    for t in runner.list_tasks().await {
        println!("   [{:?}] {} — {}", t.status, t.name, t.cron_expr);
    }

    println!("\n🗑️  Removing '{}'...", weekly.name);
    let removed = runner.remove_task(&weekly.id).await?;
    println!("   Removed: {}", removed);

    println!("📋 Remaining tasks:");
    for t in runner.list_tasks().await {
        println!("   [{:?}] {} — {}", t.status, t.name, t.cron_expr);
    }

    println!("\n✅ === demo70 complete ===");
    Ok(())
}
