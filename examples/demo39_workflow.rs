//! demo39_workflow.rs —— 图工作流引擎演示
//!
//! 展示 Graph + SharedState 的完整能力：
//! 1. 线性管道（A → B → C）
//! 2. 条件分支（Router 模式）
//! 3. 循环迭代（Loop 模式）
//! 4. 并行 fan-out / fan-in
//! 5. Agent 节点集成
//! 6. State snapshot / restore
//!
//! 运行方式：
//! ```bash
//! cargo run --example demo39_workflow
//! ```

use echo_agent::workflow::{GraphBuilder, SharedState};

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "echo_agent=info,demo39_workflow=info".into()),
        )
        .init();

    print_banner();

    // ── Part 1: 线性管道 ────────────────────────────────────────────────────
    separator("Part 1: Linear Pipeline (A -> B -> C)");
    demo_linear_pipeline().await?;

    // ── Part 2: 条件分支 ────────────────────────────────────────────────────
    separator("Part 2: Conditional Branching (Router)");
    demo_conditional().await?;

    // ── Part 3: 循环迭代 ────────────────────────────────────────────────────
    separator("Part 3: Loop / Iteration");
    demo_loop().await?;

    // ── Part 4: 并行 fan-out / fan-in ───────────────────────────────────────
    separator("Part 4: Parallel Fan-out / Fan-in");
    demo_parallel().await?;

    // ── Part 5: Agent 节点 ──────────────────────────────────────────────────
    separator("Part 5: Agent Node Integration");
    demo_agent_node().await?;

    // ── Part 6: State snapshot & restore ────────────────────────────────────
    separator("Part 6: State Snapshot & Restore");
    demo_snapshot().await?;

    println!("\n{}", "=".repeat(64));
    println!("  demo39 completed");
    println!("{}", "=".repeat(64));

    Ok(())
}

// ── Part 1: Linear Pipeline ─────────────────────────────────────────────────

async fn demo_linear_pipeline() -> echo_agent::error::Result<()> {
    let graph = GraphBuilder::new("etl_pipeline")
        .add_function_node("extract", |state: &SharedState| {
            Box::pin(async move {
                // Simulate data extraction
                let data = vec!["hello world", "RUST is great", "Echo Agent"];
                let _ = state.set("raw_data", data);
                let _ = state.set("stage", "extracted");
                Ok(())
            })
        })
        .add_function_node("transform", |state: &SharedState| {
            Box::pin(async move {
                let data: Vec<String> = state.get("raw_data").unwrap_or_default();
                let transformed: Vec<String> = data.iter().map(|s| s.to_uppercase()).collect();
                let _ = state.set("transformed_data", transformed);
                let _ = state.set("stage", "transformed");
                Ok(())
            })
        })
        .add_function_node("load", |state: &SharedState| {
            Box::pin(async move {
                let data: Vec<String> = state.get("transformed_data").unwrap_or_default();
                let _ = state.set("record_count", data.len() as i64);
                let _ = state.set("stage", "loaded");
                let _ = state.set("result", format!("Loaded {} records", data.len()));
                Ok(())
            })
        })
        .set_entry("extract")
        .add_edge("extract", "transform")
        .add_edge("transform", "load")
        .set_finish("load")
        .build()?;

    let state = SharedState::new();
    let result = graph.run(state).await?;

    println!("  Path: {:?}", result.path);
    println!("  Steps: {}", result.steps);
    println!("  Stage: {}", result.state.get::<String>("stage").unwrap());
    println!(
        "  Records: {}",
        result.state.get::<i64>("record_count").unwrap()
    );
    println!(
        "  Result: {}",
        result.state.get::<String>("result").unwrap()
    );
    println!();
    Ok(())
}

// ── Part 2: Conditional Branching ───────────────────────────────────────────

async fn demo_conditional() -> echo_agent::error::Result<()> {
    let graph = GraphBuilder::new("ticket_router")
        .add_function_node("classify", |state: &SharedState| {
            Box::pin(async move {
                let ticket: String = state.get("ticket").unwrap_or_default();
                let priority = if ticket.contains("urgent") || ticket.contains("crash") {
                    "high"
                } else if ticket.contains("bug") {
                    "medium"
                } else {
                    "low"
                };
                let _ = state.set("priority", priority);
                Ok(())
            })
        })
        .add_function_node("escalate", |state: &SharedState| {
            Box::pin(async move {
                let _ = state.set("action", "Escalated to on-call engineer");
                Ok(())
            })
        })
        .add_function_node("assign", |state: &SharedState| {
            Box::pin(async move {
                let _ = state.set("action", "Assigned to bug triage queue");
                Ok(())
            })
        })
        .add_function_node("backlog", |state: &SharedState| {
            Box::pin(async move {
                let _ = state.set("action", "Added to backlog");
                Ok(())
            })
        })
        .set_entry("classify")
        .add_conditional_edge("classify", |state: &SharedState| {
            Box::pin(async move {
                let priority: String = state.get("priority").unwrap_or_default();
                match priority.as_str() {
                    "high" => "escalate".to_string(),
                    "medium" => "assign".to_string(),
                    _ => "backlog".to_string(),
                }
            })
        })
        .set_finish("escalate")
        .set_finish("assign")
        .set_finish("backlog")
        .build()?;

    let tickets = [
        ("Server crash in production - urgent", "high"),
        ("Login button bug on mobile", "medium"),
        ("Add dark mode feature request", "low"),
    ];

    for (ticket, _expected) in &tickets {
        let state = SharedState::new();
        let _ = state.set("ticket", *ticket);
        let result = graph.run(state).await?;
        println!(
            "  Ticket: {:40} -> priority={:6} action={:?}  path={:?}",
            ticket,
            result.state.get::<String>("priority").unwrap(),
            result.state.get::<String>("action").unwrap(),
            result.path,
        );
    }
    println!();
    Ok(())
}

// ── Part 3: Loop ────────────────────────────────────────────────────────────

async fn demo_loop() -> echo_agent::error::Result<()> {
    let graph = GraphBuilder::new("refinement_loop")
        .add_function_node("init", |state: &SharedState| {
            Box::pin(async move {
                let _ = state.set("draft", "initial rough draft");
                let _ = state.set("quality_score", 30i64);
                let _ = state.set("iteration", 0i64);
                Ok(())
            })
        })
        .add_function_node("refine", |state: &SharedState| {
            Box::pin(async move {
                let iter: i64 = state.get("iteration").unwrap_or(0);
                let score: i64 = state.get("quality_score").unwrap_or(0);

                // Simulate quality improvement
                let new_score = score + 20;
                let new_iter = iter + 1;

                let _ = state.set("quality_score", new_score);
                let _ = state.set("iteration", new_iter);
                let _ = state.set(
                    "draft",
                    format!("refined draft v{} (quality={})", new_iter, new_score),
                );
                Ok(())
            })
        })
        .add_function_node("finalize", |state: &SharedState| {
            Box::pin(async move {
                let draft: String = state.get("draft").unwrap_or_default();
                let _ = state.set("final_output", format!("FINAL: {}", draft));
                Ok(())
            })
        })
        .set_entry("init")
        .add_edge("init", "refine")
        .add_conditional_edge("refine", |state: &SharedState| {
            Box::pin(async move {
                let score: i64 = state.get("quality_score").unwrap_or(0);
                if score >= 90 {
                    "finalize".to_string()
                } else {
                    "refine".to_string() // loop back
                }
            })
        })
        .set_finish("finalize")
        .build()?;

    let state = SharedState::new();
    let result = graph.run(state).await?;

    println!("  Path: {:?}", result.path);
    println!("  Steps: {}", result.steps);
    println!(
        "  Iterations: {}",
        result.state.get::<i64>("iteration").unwrap()
    );
    println!(
        "  Final score: {}",
        result.state.get::<i64>("quality_score").unwrap()
    );
    println!(
        "  Output: {}",
        result.state.get::<String>("final_output").unwrap()
    );
    println!();
    Ok(())
}

// ── Part 4: Parallel Fan-out / Fan-in ───────────────────────────────────────

async fn demo_parallel() -> echo_agent::error::Result<()> {
    let graph = GraphBuilder::new("multi_analysis")
        .add_function_node("prepare", |state: &SharedState| {
            Box::pin(async move {
                let _ = state.set("text", "Rust is a systems programming language focused on safety, speed, and concurrency.");
                Ok(())
            })
        })
        .add_function_node("word_count", |state: &SharedState| {
            Box::pin(async move {
                let text: String = state.get("text").unwrap_or_default();
                let count = text.split_whitespace().count();
                let _ = state.set("word_count", count as i64);
                Ok(())
            })
        })
        .add_function_node("char_count", |state: &SharedState| {
            Box::pin(async move {
                let text: String = state.get("text").unwrap_or_default();
                let _ = state.set("char_count", text.len() as i64);
                Ok(())
            })
        })
        .add_function_node("keyword_extract", |state: &SharedState| {
            Box::pin(async move {
                let text: String = state.get("text").unwrap_or_default();
                let keywords: Vec<&str> = text.split_whitespace()
                    .filter(|w| w.len() > 5)
                    .collect();
                let _ = state.set("keywords", keywords);
                Ok(())
            })
        })
        .add_function_node("summarize", |state: &SharedState| {
            Box::pin(async move {
                let words: i64 = state.get("word_count").unwrap_or(0);
                let chars: i64 = state.get("char_count").unwrap_or(0);
                let keywords: Vec<String> = state.get("keywords").unwrap_or_default();
                let _ = state.set(
                    "summary",
                    format!(
                        "Analysis: {} words, {} chars, keywords: {:?}",
                        words, chars, keywords
                    ),
                );
                Ok(())
            })
        })
        .set_entry("prepare")
        .add_parallel_edge(
            "prepare",
            vec![
                "word_count".to_string(),
                "char_count".to_string(),
                "keyword_extract".to_string(),
            ],
            "summarize",
        )
        .set_finish("summarize")
        .build()?;

    let state = SharedState::new();
    let result = graph.run(state).await?;

    println!("  Path: {:?}", result.path);
    println!("  Steps: {}", result.steps);
    println!(
        "  Words: {}",
        result.state.get::<i64>("word_count").unwrap()
    );
    println!(
        "  Chars: {}",
        result.state.get::<i64>("char_count").unwrap()
    );
    println!(
        "  Keywords: {:?}",
        result.state.get::<Vec<String>>("keywords").unwrap()
    );
    println!(
        "  Summary: {}",
        result.state.get::<String>("summary").unwrap()
    );
    println!();
    Ok(())
}

// ── Part 5: Agent Node ──────────────────────────────────────────────────────

async fn demo_agent_node() -> echo_agent::error::Result<()> {
    // Use MockAgent to demonstrate without real LLM
    let mock = echo_agent::testing::MockAgent::new("analyst")
        .with_response("The input data shows a positive trend with 3 key insights: growth in Q3, stable Q4, strong user engagement.");

    let graph = GraphBuilder::new("agent_workflow")
        .add_function_node("prepare_prompt", |state: &SharedState| {
            Box::pin(async move {
                let _ = state.set(
                    "task",
                    "Analyze the following metrics: revenue=100M, users=5M, growth=15%",
                );
                Ok(())
            })
        })
        .add_agent_node("analyze", mock, "task", "analysis")
        .add_function_node("format_report", |state: &SharedState| {
            Box::pin(async move {
                let analysis: String = state.get("analysis").unwrap_or_default();
                let _ = state.set("report", format!("=== Report ===\n{analysis}\n=== End ==="));
                Ok(())
            })
        })
        .set_entry("prepare_prompt")
        .add_edge("prepare_prompt", "analyze")
        .add_edge("analyze", "format_report")
        .set_finish("format_report")
        .build()?;

    let state = SharedState::new();
    let result = graph.run(state).await?;

    println!("  Path: {:?}", result.path);
    println!(
        "  Analysis: {}",
        result.state.get::<String>("analysis").unwrap()
    );
    println!(
        "  Report:\n  {}",
        result
            .state
            .get::<String>("report")
            .unwrap()
            .replace('\n', "\n  ")
    );
    println!();
    Ok(())
}

// ── Part 6: State Snapshot & Restore ────────────────────────────────────────

async fn demo_snapshot() -> echo_agent::error::Result<()> {
    // Run a graph partway
    let state = SharedState::new();
    let _ = state.set("counter", 0i64);
    let _ = state.set("label", "checkpoint demo");

    // Simulate some work
    let _ = state.set("counter", 42i64);
    let _ = state.push_message(echo_agent::llm::types::Message::user(
        "What is the meaning of life?".to_string(),
    ));

    // Take snapshot
    let snapshot = state.snapshot().expect("snapshot failed");
    println!("  Snapshot taken ({} bytes):", snapshot.len());
    println!(
        "  {}",
        if snapshot.chars().count() > 200 {
            let truncated: String = snapshot.chars().take(200).collect();
            format!("{truncated}...")
        } else {
            snapshot.clone()
        }
    );

    // Restore from snapshot
    let restored = SharedState::from_snapshot(&snapshot).unwrap();
    println!("\n  Restored state:");
    println!("    counter: {}", restored.get::<i64>("counter").unwrap());
    println!("    label: {}", restored.get::<String>("label").unwrap());
    println!("    messages: {}", restored.message_count());

    // Demonstrate merge
    let other = SharedState::new();
    let _ = other.set("extra", "merged value");
    let _ = other.set("counter", 999i64); // should NOT overwrite

    let _ = restored.merge(&other);
    println!("\n  After merge (no overwrite):");
    println!(
        "    counter: {} (preserved)",
        restored.get::<i64>("counter").unwrap()
    );
    println!(
        "    extra: {} (merged)",
        restored.get::<String>("extra").unwrap()
    );
    println!("    keys: {:?}", restored.keys());
    println!();
    Ok(())
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn print_banner() {
    println!("{}", "=".repeat(64));
    println!("      Echo Agent x Graph Workflow Engine (demo39)");
    println!("      GraphBuilder + SharedState + Conditional + Parallel");
    println!("{}", "=".repeat(64));
    println!();
}

fn separator(title: &str) {
    println!("{}", "-".repeat(64));
    println!("{title}\n");
}
