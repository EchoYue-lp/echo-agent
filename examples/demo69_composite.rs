//! demo69_composite.rs - Composite task execution (P3)
//!
//! Demonstrates CompositePlan with Sequential and Parallel strategies,
//! showing upstream result chaining between steps.
//!
//! Run: cargo run --example demo69_composite --features tasks

use echo_agent::tasks::TaskContext;
use echo_agent::tasks::{CompositePlan, CompositeStep, CompositeStrategy, execute_composite};
use std::sync::Arc;

#[tokio::main]
async fn main() {
    println!("🔗 demo69: Composite Task Execution (P3)\n");

    // ── Part 1: Sequential with upstream chaining ──────────────────────
    println!("📋 Part 1: Sequential CompositePlan");
    println!("   fetch → parse (reads fetch) → report (reads fetch + parse)\n");

    let plan = CompositePlan {
        steps: vec![
            CompositeStep {
                id: "fetch".into(),
                name: "Fetch Data".into(),
                execute_fn: Arc::new(|ctx: TaskContext| {
                    Box::pin(async move {
                        println!("   ⚙️  [{}] running...", ctx.task_id);
                        Ok("raw_data: 42 records fetched".to_string())
                    })
                }),
                input_from: vec![],
            },
            CompositeStep {
                id: "parse".into(),
                name: "Parse Data".into(),
                execute_fn: Arc::new(|ctx: TaskContext| {
                    Box::pin(async move {
                        let upstream = ctx
                            .upstream_results
                            .iter()
                            .map(|(k, v)| format!("{k}={v}"))
                            .collect::<Vec<_>>()
                            .join(", ");
                        println!("   ⚙️  [{}] received upstream: {upstream}", ctx.task_id);
                        Ok(format!("parsed: 42 records → 38 valid (from: {upstream})"))
                    })
                }),
                input_from: vec!["fetch".into()],
            },
            CompositeStep {
                id: "report".into(),
                name: "Generate Report".into(),
                execute_fn: Arc::new(|ctx: TaskContext| {
                    Box::pin(async move {
                        let upstream = ctx
                            .upstream_results
                            .iter()
                            .map(|(k, v)| format!("{k}={v}"))
                            .collect::<Vec<_>>()
                            .join("; ");
                        println!("   ⚙️  [{}] received upstream: {upstream}", ctx.task_id);
                        Ok(format!("report: summary generated from [{upstream}]"))
                    })
                }),
                input_from: vec!["fetch".into(), "parse".into()],
            },
        ],
        strategy: CompositeStrategy::Sequential,
    };

    let results = execute_composite(plan)
        .await
        .expect("sequential plan failed");
    println!("\n   ✅ Sequential results:");
    for (id, output) in &results {
        println!("      [{id}] → {output}");
    }

    // ── Part 2: Parallel independent steps ────────────────────────────
    println!("\n⚡ Part 2: Parallel CompositePlan (3 independent steps)\n");

    let plan = CompositePlan {
        steps: vec![
            CompositeStep {
                id: "search_arxiv".into(),
                name: "Search arXiv".into(),
                execute_fn: Arc::new(|ctx: TaskContext| {
                    Box::pin(async move {
                        println!("   🔍 [{}] searching...", ctx.task_id);
                        Ok("arxiv: 12 papers found".to_string())
                    })
                }),
                input_from: vec![],
            },
            CompositeStep {
                id: "search_web".into(),
                name: "Search Web".into(),
                execute_fn: Arc::new(|ctx: TaskContext| {
                    Box::pin(async move {
                        println!("   🌐 [{}] searching...", ctx.task_id);
                        Ok("web: 8 relevant pages".to_string())
                    })
                }),
                input_from: vec![],
            },
            CompositeStep {
                id: "search_docs".into(),
                name: "Search Docs".into(),
                execute_fn: Arc::new(|ctx: TaskContext| {
                    Box::pin(async move {
                        println!("   📚 [{}] searching...", ctx.task_id);
                        Ok("docs: 5 API references".to_string())
                    })
                }),
                input_from: vec![],
            },
        ],
        strategy: CompositeStrategy::Parallel,
    };

    let results = execute_composite(plan).await.expect("parallel plan failed");
    println!("\n   ✅ Parallel results:");
    for (id, output) in &results {
        println!("      [{id}] → {output}");
    }

    println!("\n🎉 demo69 complete!");
}
