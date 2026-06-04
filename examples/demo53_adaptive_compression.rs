//! demo53 — Adaptive Compression
//!
//! Demonstrates the `AdaptiveCompressor` which applies compression levels
//! from cheapest to most expensive until the token budget is met:
//!
//! | Level | Name | Strategy |
//! |-------|------|----------|
//! | L1 Snip | Snip | Remove tool outputs exceeding N tokens |
//! | L1 Fold | Fold | Collapse consecutive tool results, keep latest N |
//! | L2 | Micro | Truncate tool outputs to first/last N lines |
//! | L3 | Collapse | Remove older messages, keep recent N |
//! | L4 | Compact | Full LLM summarization (via `.with_llm()`) |
//! | L5 | Reactive | Emergency — keep only system prompt + last 3 messages |
//!
//! Scenarios 1-7 are local (no LLM calls). Scenario 8 demonstrates
//! `ContextManager` integration via the `ContextCompressor` trait.
//!
//! ```sh
//! cargo run --example demo53_adaptive_compression
//! ```

use echo_agent::compression::ContextManager;
use echo_agent::compression::levels::{AdaptiveCompressionConfig, AdaptiveCompressor};
use echo_agent::compression::{CompressionInput, CompressionOutput, ContextCompressor};
use echo_agent::prelude::{Message, MessageContent, Role};

macro_rules! section {
    ($n:expr, $title:expr) => {
        println!("\n══════════════════════════════════════════════════");
        println!("  Scenario {} : {}", $n, $title);
        println!("══════════════════════════════════════════════════");
    };
}

fn make_msg(role: Role, text: &str) -> Message {
    Message {
        role,
        content: MessageContent::Text(text.to_string()),
        ..Default::default()
    }
}

/// Create a tool output message of approximately `token_count` tokens.
/// The heuristic tokenizer uses ~4 chars per token.
fn make_tool_output(id: usize, approx_tokens: usize) -> Message {
    let char_count = approx_tokens * 4;
    let content = format!(
        "Tool output #{}:\n{}",
        id,
        "lorem ipsum dolor sit amet. ".repeat(char_count / 28 + 1)
    );
    make_msg(Role::Tool, &content)
}

#[tokio::main]
async fn main() {
    println!("╔══════════════════════════════════════════════════╗");
    println!("║    echo-agent  Adaptive Compression Demo         ║");
    println!("║  (no LLM calls — local heuristic compression)    ║");
    println!("╚══════════════════════════════════════════════════╝");

    demo_l1_snip();
    demo_l1_fold();
    demo_l2_micro();
    demo_l3_collapse();
    demo_l5_reactive();
    demo_escalation();
    demo_no_compression_needed();
    demo_context_manager_integration().await;

    println!("\n╔══════════════════════════════════════════════════╗");
    println!("║  All 8 scenarios passed ✅                       ║");
    println!("╚══════════════════════════════════════════════════╝");
}

/// L1 Snip: truncate oversized tool outputs
fn demo_l1_snip() {
    section!("1", "L1 Snip — truncate oversized tool outputs");

    let config = AdaptiveCompressionConfig {
        l1_snip_threshold_tokens: 0,      // always try L1
        l1_max_output_tokens: 50,         // max 50 tokens per tool output
        l1_fold_consecutive_tools: false, // disable fold for isolation
        // Disable higher levels for this isolated test
        l2_micro_threshold_tokens: usize::MAX,
        l3_collapse_threshold_tokens: usize::MAX,
        l4_compact_threshold_tokens: usize::MAX,
        ..Default::default()
    };
    let compressor = AdaptiveCompressor::new(config);

    let mut messages = vec![
        make_msg(Role::System, "You are a helpful assistant."),
        make_msg(Role::User, "Read the file"),
        make_tool_output(1, 500), // ~500 tokens — way above the 50 limit
        make_msg(Role::User, "Thanks!"),
    ];

    let current_tokens = 600;
    let target_tokens = 100;
    let result = compressor.compress_in_place(&mut messages, current_tokens, target_tokens);

    println!("  Tokens before: {}", result.tokens_before);
    println!("  Tokens after:  {}", result.tokens_after);
    println!("  Levels applied: {:?}", result.levels_applied);

    assert!(result.levels_applied.contains(&"L1:Snip".to_string()));
    println!("  ✅ L1:Snip applied — oversized tool output truncated");
}

/// L1 Fold: collapse consecutive tool results
fn demo_l1_fold() {
    section!("2", "L1 Fold — collapse consecutive tool results");

    let config = AdaptiveCompressionConfig {
        l1_snip_threshold_tokens: 0, // always try L1
        l1_max_output_tokens: 99999, // don't snip — only fold
        l1_fold_consecutive_tools: true,
        l1_fold_keep_latest: 2, // keep latest 2 per run
        // Disable higher levels
        l2_micro_threshold_tokens: usize::MAX,
        l3_collapse_threshold_tokens: usize::MAX,
        l4_compact_threshold_tokens: usize::MAX,
        ..Default::default()
    };
    let compressor = AdaptiveCompressor::new(config);

    let mut messages = vec![
        make_msg(Role::System, "System prompt"),
        make_msg(Role::User, "read all files"),
        make_tool_output(1, 100),
        make_tool_output(2, 100),
        make_tool_output(3, 100),
        make_tool_output(4, 100),
        make_tool_output(5, 100),
        make_msg(Role::Assistant, "Done reading all files."),
    ];

    let original_count = messages.len();
    let current_tokens = 1000;
    let target_tokens = 100;
    let result = compressor.compress_in_place(&mut messages, current_tokens, target_tokens);

    println!("  Messages before: {}", original_count);
    println!("  Messages after:  {}", messages.len());
    println!("  Levels applied: {:?}", result.levels_applied);

    // 5 tool msgs with keep=2 → fold 3, insert 1 summary → 8 - 3 + 1 = 6
    assert!(result.levels_applied.contains(&"L1:Fold".to_string()));
    assert!(
        messages
            .iter()
            .any(|m| m.content.as_text_ref().unwrap_or("").contains("L1 fold"))
    );
    println!("  ✅ L1:Fold applied — 3 consecutive tool results collapsed into 1 summary");
}

/// L2: Micro — truncate tool outputs to first/last N lines
fn demo_l2_micro() {
    section!("3", "L2 Micro — truncate to first/last N lines");

    let config = AdaptiveCompressionConfig {
        // Disable L1 so we can isolate L2
        l1_snip_threshold_tokens: usize::MAX,
        l2_micro_threshold_tokens: 0, // always try L2
        l2_keep_lines: 5,             // keep first 5 + last 5 lines
        // Disable higher levels
        l3_collapse_threshold_tokens: usize::MAX,
        l4_compact_threshold_tokens: usize::MAX,
        ..Default::default()
    };
    let compressor = AdaptiveCompressor::new(config);

    // Create a tool output with 30 lines (more than 5*2 = 10)
    let long_output: String = (1..=30)
        .map(|i| format!("Line {}: some content here", i))
        .collect::<Vec<_>>()
        .join("\n");
    let mut messages = vec![
        make_msg(Role::System, "system prompt"),
        make_msg(Role::User, "run the command"),
        make_msg(Role::Tool, &long_output),
    ];

    let current_tokens = 500;
    let target_tokens = 100;
    let result = compressor.compress_in_place(&mut messages, current_tokens, target_tokens);

    println!("  Original lines: 30");
    println!("  Keep lines: 5 head + 5 tail = 10");
    println!("  Tokens before: {}", result.tokens_before);
    println!("  Tokens after:  {}", result.tokens_after);
    println!("  Levels applied: {:?}", result.levels_applied);

    assert!(result.levels_applied.contains(&"L2:Micro".to_string()));
    println!("  ✅ L2:Micro applied — truncated to first/last 5 lines");
}

/// L3: Collapse — remove older messages, keep recent N
fn demo_l3_collapse() {
    section!("4", "L3 Collapse — remove older messages");

    let config = AdaptiveCompressionConfig {
        // Disable L1 and L2
        l1_snip_threshold_tokens: usize::MAX,
        l2_micro_threshold_tokens: usize::MAX,
        l3_collapse_threshold_tokens: 0, // always try L3
        l3_keep_recent: 3,               // keep last 3 messages
        // Disable L4/L5
        l4_compact_threshold_tokens: usize::MAX,
        ..Default::default()
    };
    let compressor = AdaptiveCompressor::new(config);

    let mut messages = vec![
        make_msg(Role::System, "You are a coding assistant."),
        make_msg(Role::User, "message 1 — old"),
        make_msg(Role::Assistant, "response 1 — old"),
        make_msg(Role::User, "message 2 — old"),
        make_msg(Role::Assistant, "response 2 — old"),
        make_msg(Role::User, "message 3 — old"),
        make_msg(Role::Assistant, "response 3 — old"),
        make_msg(Role::User, "latest question"),
        make_msg(Role::Assistant, "latest response"),
        make_msg(Role::User, "follow-up"),
    ];

    let original_count = messages.len();
    let current_tokens = 1000;
    let target_tokens = 100;
    let result = compressor.compress_in_place(&mut messages, current_tokens, target_tokens);

    println!("  Messages before: {}", original_count);
    println!("  Messages after:  {}", messages.len());
    println!("  Tokens before:   {}", result.tokens_before);
    println!("  Tokens after:    {}", result.tokens_after);
    println!("  Levels applied:  {:?}", result.levels_applied);

    // System message should be preserved
    assert!(messages.iter().any(|m| m.role == Role::System));
    assert!(result.levels_applied.contains(&"L3:Collapse".to_string()));
    println!("  ✅ L3:Collapse applied — older messages removed, system + recent kept");
}

/// L5: Reactive — emergency compression
fn demo_l5_reactive() {
    section!("5", "L5 Reactive — emergency (system prompt + last 3 only)");

    let config = AdaptiveCompressionConfig {
        // Disable L1, L2, L3
        l1_snip_threshold_tokens: usize::MAX,
        l2_micro_threshold_tokens: usize::MAX,
        l3_collapse_threshold_tokens: usize::MAX,
        l4_compact_threshold_tokens: 0, // allow L5 to trigger
        ..Default::default()
    };
    let compressor = AdaptiveCompressor::new(config);

    // Use substantial content so token counts are realistic
    let long_text = "lorem ipsum dolor sit amet consectetur adipiscing elit. ".repeat(20);
    let mut messages = vec![
        make_msg(Role::System, "You are an AI assistant."),
        make_msg(Role::User, &long_text),
        make_msg(Role::Assistant, &long_text),
        make_msg(Role::User, &long_text),
        make_msg(Role::Assistant, &long_text),
        make_msg(Role::User, &long_text),
        make_msg(Role::Assistant, &long_text),
        make_msg(Role::User, &long_text),
        make_msg(Role::Assistant, &long_text),
        make_msg(Role::User, "final question"),
    ];

    let original_count = messages.len();
    // Estimate ~1 token per 4 chars
    let current_tokens: usize = messages
        .iter()
        .map(|m| m.content.as_text_ref().unwrap_or("").len() / 4)
        .sum();
    // Target much smaller than current
    let target_tokens = current_tokens / 20;
    let result = compressor.compress_in_place(&mut messages, current_tokens, target_tokens);

    println!("  Messages before: {}", original_count);
    println!("  Messages after:  {}", messages.len());
    println!("  Tokens before:   {}", result.tokens_before);
    println!("  Tokens after:    {}", result.tokens_after);
    println!("  Levels applied:  {:?}", result.levels_applied);

    assert!(messages.iter().any(|m| m.role == Role::System));
    assert!(result.levels_applied.contains(&"L5:Reactive".to_string()));
    println!("  ✅ L5:Reactive applied — only system prompt + last 3 messages remain");
}

/// Automatic escalation: multiple levels applied in sequence
fn demo_escalation() {
    section!("6", "Automatic Level Escalation");

    // Set all thresholds low so multiple levels fire
    let config = AdaptiveCompressionConfig {
        l1_snip_threshold_tokens: 0,
        l1_max_output_tokens: 20,
        l2_micro_threshold_tokens: 0,
        l2_keep_lines: 3,
        l3_collapse_threshold_tokens: 0,
        l3_keep_recent: 3,
        l4_compact_threshold_tokens: usize::MAX,
        ..Default::default()
    };
    let compressor = AdaptiveCompressor::new(config);

    // Build a conversation with a large tool output
    let long_output: String = (1..=40)
        .map(|i| format!("Line {}: data={}", i, "x".repeat(50)))
        .collect::<Vec<_>>()
        .join("\n");
    let mut messages = vec![
        make_msg(Role::System, "System prompt"),
        make_msg(Role::User, "old question 1"),
        make_msg(Role::Assistant, "old answer 1"),
        make_msg(Role::User, "old question 2"),
        make_msg(Role::Assistant, "old answer 2"),
        make_msg(Role::User, "read file"),
        make_msg(Role::Tool, &long_output),
        make_msg(Role::User, "latest question"),
    ];

    let current_tokens = 2000;
    let target_tokens = 100;
    let result = compressor.compress_in_place(&mut messages, current_tokens, target_tokens);

    println!("  Tokens before: {}", result.tokens_before);
    println!("  Tokens after:  {}", result.tokens_after);
    println!("  Levels applied: {:?}", result.levels_applied);
    println!("  Messages remaining: {}", messages.len());

    assert!(!result.levels_applied.is_empty());
    println!("  ✅ Multiple levels escalated automatically");

    println!("\n  Escalation order: L1:Snip → L1:Fold → L2:Micro → L3:Collapse → L5:Reactive");
    println!("  (L4:Compact skipped — no LLM client configured via .with_llm())");
}

/// No compression when below target
fn demo_no_compression_needed() {
    section!("7", "No Compression Needed");

    let config = AdaptiveCompressionConfig::default();
    let compressor = AdaptiveCompressor::new(config);

    let mut messages = vec![
        make_msg(Role::User, "hello"),
        make_msg(Role::Assistant, "hi there!"),
    ];

    // current_tokens < target → nothing should happen
    let result = compressor.compress_in_place(&mut messages, 100, 200);

    println!("  Tokens before: {}", result.tokens_before);
    println!("  Tokens after:  {}", result.tokens_after);
    println!("  Levels applied: {:?}", result.levels_applied);

    assert!(result.levels_applied.is_empty());
    println!("  ✅ No levels applied when already within budget");
}

/// ContextManager integration: AdaptiveCompressor via ContextCompressor trait
async fn demo_context_manager_integration() {
    section!("8", "ContextManager Integration (ContextCompressor trait)");

    let config = AdaptiveCompressionConfig {
        l1_snip_threshold_tokens: 0,
        l1_max_output_tokens: 99999,
        l1_fold_consecutive_tools: true,
        l1_fold_keep_latest: 1,
        l3_collapse_threshold_tokens: 0,
        l3_keep_recent: 2,
        l4_compact_threshold_tokens: usize::MAX,
        ..Default::default()
    };
    let compressor = AdaptiveCompressor::new(config);

    // AdaptiveCompressor now implements ContextCompressor!
    // It can be used directly with ContextManager::builder().compressor()
    let mut ctx = ContextManager::builder(10) // very low limit to force compression
        .compressor(compressor)
        .with_system("You are a helpful assistant.".to_string())
        .build();

    // Add many messages to trigger compression
    for i in 0..10 {
        ctx.push(make_msg(Role::User, &format!("Question {}", i)));
        ctx.push(make_msg(Role::Assistant, &format!("Answer {}", i)));
    }

    let before_count = ctx.messages().len();
    let before_tokens = ctx.token_estimate();

    // prepare() triggers auto-compression via the ContextCompressor trait
    let result = ctx.prepare(None).await.unwrap();
    let compressed = result.compressed.expect("compression should have occurred");

    println!(
        "  Before: {} messages, {} tokens",
        before_count, before_tokens
    );
    println!(
        "  After:  {} messages, {} tokens",
        compressed.after_count, compressed.after_tokens
    );
    println!("  Evicted: {} messages", compressed.evicted);

    // Check compression metrics
    let metrics = ctx.compression_metrics();
    println!("  Total compressions: {}", metrics.total_compressions);
    println!("  Tokens saved: {}", metrics.total_tokens_saved());
    println!("  Report: {}", metrics.report());

    assert!(compressed.after_count < before_count);
    assert!(metrics.total_compressions >= 1);
    println!("  ✅ AdaptiveCompressor works seamlessly via ContextCompressor trait");
    println!("  ✅ CompressionMetrics tracked automatically");

    // Also demonstrate direct trait usage (without ContextManager)
    println!("\n  --- Direct trait usage ---");
    let compressor2 = AdaptiveCompressor::new(AdaptiveCompressionConfig {
        l3_collapse_threshold_tokens: 0,
        l3_keep_recent: 2,
        ..Default::default()
    });

    let messages = vec![
        make_msg(Role::System, "system"),
        make_msg(Role::User, "msg1"),
        make_msg(Role::Assistant, "msg2"),
        make_msg(Role::User, "msg3"),
        make_msg(Role::Assistant, "msg4"),
        make_msg(Role::User, "msg5"),
    ];

    let input = CompressionInput {
        messages,
        token_limit: 0,
        current_query: None,
    };

    let output: CompressionOutput = compressor2.compress(input).await.unwrap();
    println!("  Output messages: {}", output.messages.len());
    println!("  Evicted messages: {}", output.evicted.len());
    println!("  Compressor name: {}", compressor2.name());
    assert!(output.evicted.len() > 0, "should have evicted messages");
    println!("  ✅ ContextCompressor::compress() works directly on AdaptiveCompressor");
}
