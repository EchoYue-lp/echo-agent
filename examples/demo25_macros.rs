//! demo25: 宏系统综合演示
//!
//! 展示 echo-agent 提供的所有便捷宏，覆盖从工具定义、Agent 构建到消息组装等场景。
//!
//! ```bash
//! cargo run --example demo25_macros
//! ```

use echo_agent::error::Result;
use echo_agent::prelude::*;
use echo_agent::{agent, callback, chat_request, guard, messages, tool, tool_params};

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// 1. #[tool] —— 一行定义工具
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

#[tool(name = "add", description = "两数相加")]
async fn add(
    /// 第一个数
    a: f64,
    /// 第二个数
    b: f64,
) -> Result<ToolResult> {
    Ok(ToolResult::success(format!("{}", a + b)))
}

#[tool(name = "multiply", description = "两数相乘")]
async fn multiply(
    /// 第一个数
    a: f64,
    /// 第二个数
    b: f64,
) -> Result<ToolResult> {
    Ok(ToolResult::success(format!("{}", a * b)))
}

#[tool(name = "greet", description = "问候用户", permissions = [Execute])]
async fn greet(
    /// 用户姓名
    name: String,
) -> Result<ToolResult> {
    Ok(ToolResult::success(format!("你好，{}！", name)))
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// 2. #[callback] —— 只覆写需要的回调方法
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

struct PrintCallback;

#[callback]
impl PrintCallback {
    async fn on_tool_start(&self, _agent: &str, tool: &str, _args: &serde_json::Value) {
        println!("  [回调] 工具开始: {tool}");
    }
    async fn on_final_answer(&self, _agent: &str, answer: &str) {
        println!("  [回调] 最终答案: {answer}");
    }
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// 3. #[guard] —— 一个函数即是护栏
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

#[guard(name = "length-limit")]
async fn check_length(content: &str, direction: GuardDirection) -> Result<GuardResult> {
    let _ = direction;
    if content.len() > 50000 {
        Ok(GuardResult::Block {
            reason: "内容过长，超过 50000 字符".into(),
        })
    } else {
        Ok(GuardResult::Pass)
    }
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// main
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

#[tokio::main]
async fn main() -> Result<()> {
    println!("═══════════════════════════════════════════════════════════════");
    println!("      Echo Agent × 宏系统综合演示 (demo25)");
    println!("═══════════════════════════════════════════════════════════════\n");

    // ── 4. messages! 宏 ─────────────────────────────────────────────────
    println!("── messages! 宏 ──────────────────────────────────────────────");
    let msgs = messages![
        system("你是一个计算助手"),
        user("你好"),
        assistant("你好！有什么可以帮助你的？"),
        user("帮我算 3 + 5"),
    ];
    println!("  构建了 {} 条消息:", msgs.len());
    for m in &msgs {
        println!(
            "    [{}] {}",
            m.role.as_str(),
            m.content.as_text_ref().unwrap_or("")
        );
    }

    // ── 5. tool_params! 宏 ──────────────────────────────────────────────
    println!("\n── tool_params! 宏 ────────────────────────────────────────────");
    let schema = tool_params! {
        "expression" => (string, required, "数学表达式"),
        "precision"  => (number, "小数精度，默认为 2"),
        "verbose"    => (boolean),
    };
    println!(
        "  生成的 JSON Schema:\n  {}",
        serde_json::to_string_pretty(&schema).unwrap()
    );

    // ── 6. chat_request! 宏 ─────────────────────────────────────────────
    println!("\n── chat_request! 宏 ───────────────────────────────────────────");
    let req = chat_request!(
        messages: [system("你是助手"), user("你好")],
        temperature: 0.7,
        max_tokens: 1024,
    );
    println!("  消息数: {}", req.messages.len());
    println!("  temperature: {:?}", req.temperature);
    println!("  max_tokens: {:?}", req.max_tokens);

    // ── 7. agent! 宏 ────────────────────────────────────────────────────
    println!("\n── agent! 宏 ──────────────────────────────────────────────────");

    let agent = agent! {
        model: "qwen3-max",
        system_prompt: "你是一个计算助手，必须通过工具完成所有计算。",
        name: "macro_demo_agent",
        tools: [AddTool, MultiplyTool, GreetTool],
        callbacks: [PrintCallback],
        guards: [LengthLimitGuard],
        max_iterations: 10,
    }?;

    println!("  Agent 创建成功，注册工具: {:?}", agent.tool_names());

    // 实际对话
    println!("\n── 执行对话 ──────────────────────────────────────────────────");
    let task = "计算 (3 + 7) × 5";
    println!("  任务: {task}\n");
    let answer = agent.execute(task).await?;
    println!("\n  结果: {answer}");

    println!("\n═══════════════════════════════════════════════════════════════");
    println!("  demo25 完成");
    println!("═══════════════════════════════════════════════════════════════");

    Ok(())
}
