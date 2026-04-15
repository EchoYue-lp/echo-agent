//! demo34_workflow_stream —— Workflow 流式输出
//!
//! 演示 `Graph::run_stream()` 方法，逐节点发出 `WorkflowEvent`，
//! 支持 NodeStart / NodeEnd / Token / Completed 等实时事件。
//!
//! ```bash
//! cargo run --example demo34_workflow_stream
//! ```

use echo_agent::prelude::*;
use futures::StreamExt;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("echo_agent=info")
        .init();

    println!("═══ Workflow Streaming Demo ═══\n");

    // ── 1. 线性管道流式输出 ───────────────────────────────────────────────
    println!("[1] 线性管道流式执行：fetch → process → report\n");
    demo_linear_stream().await?;

    // ── 2. 并行 fan-out 流式输出 ─────────────────────────────────────────
    println!("\n[2] 并行 fan-out 流式执行：start → [analyze, count] → merge\n");
    demo_parallel_stream().await?;

    // ── 3. 条件分支流式输出 ───────────────────────────────────────────────
    println!("\n[3] 条件分支流式执行：check → yes|no\n");
    demo_conditional_stream().await?;

    println!("\n═══ Demo Complete ═══");
    Ok(())
}

async fn demo_linear_stream() -> echo_agent::error::Result<()> {
    let graph = GraphBuilder::new("etl_stream")
        .add_function_node("fetch", |state: &SharedState| {
            Box::pin(async move {
                let _ = state.set("records", vec!["Alice", "Bob", "Charlie"]);
                Ok(())
            })
        })
        .add_function_node("process", |state: &SharedState| {
            Box::pin(async move {
                let records: Vec<String> = state.get("records").unwrap_or_default();
                let upper: Vec<String> = records.iter().map(|s| s.to_uppercase()).collect();
                let _ = state.set("processed", upper.len() as i64);
                let _ = state.set("data", upper);
                Ok(())
            })
        })
        .add_function_node("report", |state: &SharedState| {
            Box::pin(async move {
                let count: i64 = state.get("processed").unwrap_or(0);
                let _ = state.set("result", format!("ETL complete: {count} records"));
                Ok(())
            })
        })
        .set_entry("fetch")
        .add_edge("fetch", "process")
        .add_edge("process", "report")
        .set_finish("report")
        .build()?;

    let state = SharedState::new();
    let mut stream = graph.run_stream(state).await?;

    while let Some(event) = stream.next().await {
        print_event(&event?);
    }
    Ok(())
}

async fn demo_parallel_stream() -> echo_agent::error::Result<()> {
    let graph = GraphBuilder::new("parallel_stream")
        .add_function_node("start", |state: &SharedState| {
            Box::pin(async move {
                let _ = state.set("text", "Rust is blazingly fast and memory-efficient");
                Ok(())
            })
        })
        .add_function_node("analyze", |state: &SharedState| {
            Box::pin(async move {
                let text: String = state.get("text").unwrap_or_default();
                let keywords: Vec<&str> = text.split_whitespace().filter(|w| w.len() > 4).collect();
                let _ = state.set("keywords", keywords);
                Ok(())
            })
        })
        .add_function_node("count", |state: &SharedState| {
            Box::pin(async move {
                let text: String = state.get("text").unwrap_or_default();
                let _ = state.set("word_count", text.split_whitespace().count() as i64);
                Ok(())
            })
        })
        .add_function_node("merge", |state: &SharedState| {
            Box::pin(async move {
                let words: i64 = state.get("word_count").unwrap_or(0);
                let kw: Vec<String> = state.get("keywords").unwrap_or_default();
                let _ = state.set("result", format!("{words} words, keywords: {kw:?}"));
                Ok(())
            })
        })
        .set_entry("start")
        .add_parallel_edge("start", vec!["analyze".into(), "count".into()], "merge")
        .set_finish("merge")
        .build()?;

    let state = SharedState::new();
    let mut stream = graph.run_stream(state).await?;

    while let Some(event) = stream.next().await {
        print_event(&event?);
    }
    Ok(())
}

async fn demo_conditional_stream() -> echo_agent::error::Result<()> {
    let graph = GraphBuilder::new("cond_stream")
        .add_function_node("check", |_state: &SharedState| {
            Box::pin(async move { Ok(()) })
        })
        .add_function_node("approve", |state: &SharedState| {
            Box::pin(async move {
                let _ = state.set("result", "Request approved ✓");
                Ok(())
            })
        })
        .add_function_node("reject", |state: &SharedState| {
            Box::pin(async move {
                let _ = state.set("result", "Request rejected ✗");
                Ok(())
            })
        })
        .set_entry("check")
        .add_conditional_edge("check", |state: &SharedState| {
            Box::pin(async move {
                let score: i64 = state.get("score").unwrap_or(0);
                if score >= 60 {
                    "approve".to_string()
                } else {
                    "reject".to_string()
                }
            })
        })
        .set_finish("approve")
        .set_finish("reject")
        .build()?;

    for (label, score) in [("高分", 85i64), ("低分", 30i64)] {
        println!("  --- {label}（score={score}）---");
        let state = SharedState::new();
        let _ = state.set("score", score);
        let mut stream = graph.run_stream(state).await?;
        while let Some(event) = stream.next().await {
            print_event(&event?);
        }
        println!();
    }
    Ok(())
}

fn print_event(event: &WorkflowEvent) {
    match event {
        WorkflowEvent::NodeStart {
            node_name,
            step_index,
        } => {
            println!("    ▶ [{step_index}] NodeStart: {node_name}");
        }
        WorkflowEvent::NodeEnd {
            node_name,
            step_index,
            elapsed,
        } => {
            println!("    ✓ [{step_index}] NodeEnd:   {node_name} ({elapsed:?})");
        }
        WorkflowEvent::Token { node_name, token } => {
            println!("    📝 [{node_name}] Token: {token}");
        }
        WorkflowEvent::NodeError { node_name, error } => {
            println!("    ✗ [{node_name}] Error: {error}");
        }
        WorkflowEvent::Completed {
            result,
            total_steps,
            elapsed,
        } => {
            println!("    🏁 Completed: \"{result}\" ({total_steps} steps, {elapsed:?})");
        }
    }
}
