//! demo65_context_assembler.rs - ContextAssembler 完整示例
//!
//! 本示例演示如何使用 ContextAssembler 构建上下文消息列表，
//! 包括优先级排序和预算感知的截断。
//!
//! 运行方式: cargo run --example demo65_context_assembler

use echo_agent::context::{ContextAssembler, ContextBudget, ContextSources};
use echo_agent::llm::types::{Message, Role};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== demo65: Context Assembler ===\n");

    // 示例 1: 基本用法（无预算限制）
    println!("--- 示例 1: 基本用法 ---");
    basic_usage()?;

    // 示例 2: 预算配置
    println!("\n--- 示例 2: 预算配置 ---");
    with_budget()?;

    // 示例 3: 优先级排序
    println!("\n--- 示例 3: 优先级排序 ---");
    priority_ordering()?;

    // 示例 4: 动态预算调整
    println!("\n--- 示例 4: 动态预算调整 ---");
    dynamic_budget()?;

    println!("\n=== demo65 完成 ===");
    Ok(())
}

/// 示例 1: 基本用法（无预算限制）
fn basic_usage() -> Result<(), Box<dyn std::error::Error>> {
    let assembler = ContextAssembler::new();

    let sources = ContextSources {
        system_prompt: Some("You are a helpful assistant.".to_string()),
        conversation_history: vec![
            Message::user("What is Rust?".to_string()),
            Message::assistant(
                "Rust is a systems programming language focused on safety and performance."
                    .to_string(),
            ),
            Message::user("What about memory management?".to_string()),
            Message::assistant(
                "Rust uses ownership and borrowing for memory safety without GC.".to_string(),
            ),
        ],
        ..Default::default()
    };

    let messages = assembler.assemble(sources);

    println!("组装了 {} 条消息:", messages.len());
    for (i, msg) in messages.iter().enumerate() {
        let content = msg.content.as_text_ref().unwrap_or("");
        let preview = if content.chars().count() > 60 {
            let truncated: String = content.chars().take(60).collect();
            format!("{truncated}...")
        } else {
            content.to_string()
        };
        println!("  [{}] {}: {}", i, msg.role.as_str(), preview);
    }

    let total_tokens = estimate_tokens(&messages);
    println!("估算 token 数: {}", total_tokens);

    Ok(())
}

/// 示例 2: 预算配置
fn with_budget() -> Result<(), Box<dyn std::error::Error>> {
    // 设置严格的预算限制
    let budget = ContextBudget {
        total_tokens: 2000,
        user_reserve: 200,
        history_max: 800,
        tool_results_max: 500,
        memory_max: 300,
    };

    let assembler = ContextAssembler::new().with_budget(budget);

    // 创建大量上下文（超过预算）
    let mut history = Vec::new();
    for i in 0..20 {
        history.push(Message::user(format!("Question {}", i)));
        history.push(Message::assistant(format!(
            "Answer {} with detailed explanation...",
            i
        )));
    }

    let memory_text = (0..10)
        .map(|i| format!("Memory item {}: important context", i))
        .collect::<Vec<_>>()
        .join("\n");

    let sources = ContextSources {
        system_prompt: Some("You are a coding assistant.".to_string()),
        conversation_history: history,
        memory_recall: Some(memory_text),
        ..Default::default()
    };

    let messages = assembler.assemble(sources);
    let total_tokens = estimate_tokens(&messages);

    println!("原始上下文估算: ~{} tokens", 20 * 2 * 50 + 10 * 30);
    println!("组装后估算: {} tokens", total_tokens);
    println!("消息数: {}", messages.len());
    println!("预算限制: 2000 tokens");

    // 验证符合预算
    assert!(total_tokens <= 2000, "Exceeded budget!");
    println!("✓ 符合预算限制");

    Ok(())
}

/// 示例 3: 优先级排序
fn priority_ordering() -> Result<(), Box<dyn std::error::Error>> {
    let assembler = ContextAssembler::new();

    let sources = ContextSources {
        // Critical (10) - System prompt
        system_prompt: Some("You are an expert Rust developer.".to_string()),

        // High (8) - Developer instructions and project rules
        developer_instructions: vec![
            "Always use idiomatic Rust".to_string(),
            "Prefer Result over panic".to_string(),
        ],
        project_rules: vec!["Focus on performance optimization".to_string()],

        // High (8) - Task state
        task_state: Some("Current task: Fix memory leak in connection pool".to_string()),

        // Medium (5) - Conversation history
        conversation_history: vec![
            Message::user("The program is slow".to_string()),
            Message::assistant("Let me analyze the performance profile".to_string()),
        ],

        // Low (3) - Memory recall
        memory_recall: Some("User prefers detailed explanations".to_string()),

        // BestEffort (1) - Hook injected
        hook_injected: vec!["Additional context from lifecycle hook".to_string()],

        ..Default::default()
    };

    let messages = assembler.assemble(sources);

    println!("消息按优先级排序:");
    for (i, msg) in messages.iter().enumerate() {
        let content = msg.content.as_text_ref().unwrap_or("");
        let preview = if content.chars().count() > 50 {
            let truncated: String = content.chars().take(50).collect();
            format!("{truncated}...")
        } else {
            content.to_string()
        };

        // 根据内容猜测优先级
        let priority = if content.contains("expert Rust") {
            "Critical"
        } else if content.contains("idiomatic")
            || content.contains("Result over panic")
            || content.contains("performance optimization")
            || content.contains("Current task")
        {
            "High"
        } else if msg.role == Role::User || msg.role == Role::Assistant {
            "Medium"
        } else if content.contains("prefers") {
            "Low"
        } else {
            "BestEffort"
        };

        println!(
            "  [{}] {} ({}): {}",
            i,
            msg.role.as_str(),
            priority,
            preview
        );
    }

    Ok(())
}

/// 示例 4: 动态预算调整
fn dynamic_budget() -> Result<(), Box<dyn std::error::Error>> {
    // 模拟不同复杂度的任务
    let tasks = vec![
        ("Simple question", false),
        ("Complex debugging task with multiple files", true),
    ];

    for (task_desc, is_complex) in tasks {
        println!("\n任务: {}", task_desc);

        let budget = if is_complex {
            println!("  → 使用高预算配置");
            ContextBudget {
                total_tokens: 15000,
                user_reserve: 1000,
                history_max: 6000,
                tool_results_max: 4000,
                memory_max: 2000,
            }
        } else {
            println!("  → 使用低预算配置");
            ContextBudget {
                total_tokens: 4000,
                user_reserve: 300,
                history_max: 1500,
                tool_results_max: 1000,
                memory_max: 500,
            }
        };

        let assembler = ContextAssembler::new().with_budget(budget.clone());

        // 创建大量上下文
        let mut history = Vec::new();
        for i in 0..30 {
            history.push(Message::user(format!("Message {}", i)));
            history.push(Message::assistant(format!("Response {} with details", i)));
        }

        let sources = ContextSources {
            system_prompt: Some("You are a helpful assistant.".to_string()),
            conversation_history: history,
            ..Default::default()
        };

        let messages = assembler.assemble(sources);
        let total_tokens = estimate_tokens(&messages);

        println!("  预算限制: {} tokens", budget.total_tokens);
        println!("  实际使用: {} tokens", total_tokens);
        println!("  消息数: {}", messages.len());
        println!("  ✓ 符合预算");
    }

    Ok(())
}

/// 估算 token 数（粗略估计：每 4 个字符约等于 1 个 token）
fn estimate_tokens(messages: &[Message]) -> usize {
    messages
        .iter()
        .filter_map(|m| m.content.as_text_ref())
        .map(|text| text.len() / 4)
        .sum()
}
