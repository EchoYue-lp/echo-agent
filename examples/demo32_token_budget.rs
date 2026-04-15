//! demo32_token_budget —— Token 预算管控完整演示
//!
//! 演示 `max_tool_output_tokens` 工具输出超限自动截断功能。
//!
//! # 问题场景
//!
//! ```
//! 用户: "读取这个 10MB 的日志文件，分析错误"
//!
//! [传统方案]
//! LLM → read_file("/var/log/huge.log")
//! → 返回 500 万 token（超出上下文！）
//! → 错误或无限等待
//!
//! [Token Budget 方案]
//! LLM → read_file("/var/log/huge.log")
//! → 工具返回 500 万 token
//! → 自动截断到 max_tool_output_tokens（如 2000）
//! → LLM 看到截断提示，可选择分析部分或请求更多
//! ```
//!
//! # 运行方式
//!
//! ```bash
//! cargo run --example demo32_token_budget
//! ```

use echo_agent::prelude::*;
use echo_agent::tools::{Tool, ToolParameters, ToolResult};
use futures::future::BoxFuture;
use serde_json::json;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    println!("═══ Token Budget Control Demo ═══\n");

    // ── Part 1: 配置方式演示 ─────────────────────────────────────────────────
    demo_config();

    // ── Part 2: 实际截断效果演示 ───────────────────────────────────────────────
    demo_actual_truncation().await?;

    println!("═══ Demo Complete ═══");
    Ok(())
}

// ── Part 1: 配置方式演示 ─────────────────────────────────────────────────────

fn demo_config() {
    println!("─────────────────────────────────────────────");
    println!("Part 1: 配置方式");
    println!("─────────────────────────────────────────────\n");

    // AgentConfig 链式配置
    println!("  AgentConfig 链式配置:");
    let config =
        AgentConfig::new("qwen3-max", "budget_agent", "你是助手").max_tool_output_tokens(2000);

    println!(
        "    max_tool_output_tokens = {:?}",
        config.get_max_tool_output_tokens()
    );

    // ReactAgentBuilder 链式配置
    println!("\n  ReactAgentBuilder 链式配置:");
    let agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("budget_agent")
        .max_tool_output_tokens(1500)
        .build()
        .unwrap();

    println!(
        "    max_tool_output_tokens = {:?}",
        agent.config().get_max_tool_output_tokens()
    );

    // 默认值
    let default_config = AgentConfig::new("model", "agent", "prompt");
    println!("\n  默认值:");
    println!(
        "    max_tool_output_tokens = {:?}",
        default_config.get_max_tool_output_tokens()
    );
    println!("    (None 表示不限制)\n");
}

// ── Part 2: 实际截断效果演示 ─────────────────────────────────────────────────

async fn demo_actual_truncation() -> echo_agent::error::Result<()> {
    println!("─────────────────────────────────────────────");
    println!("Part 2: 实际截断效果演示");
    println!("─────────────────────────────────────────────\n");

    // 模拟一个可以控制输出长度的工具
    struct ConfigurableOutputTool {
        output_length: usize,
    }

    impl Tool for ConfigurableOutputTool {
        fn name(&self) -> &str {
            "get_file_content"
        }

        fn description(&self) -> &str {
            "获取文件内容（可配置输出长度）"
        }

        fn parameters(&self) -> serde_json::Value {
            json!({
                "type": "object",
                "properties": {
                    "include_metadata": {
                        "type": "boolean",
                        "description": "是否包含元数据"
                    }
                }
            })
        }

        fn execute(
            &self,
            _params: ToolParameters,
        ) -> BoxFuture<'_, echo_agent::error::Result<ToolResult>> {
            let length = self.output_length;

            Box::pin(async move {
                // 生成指定长度的输出（模拟文件内容）
                let mut content = String::from("=== 文件开始 ===\n\n");

                // 每行约 50 字符
                let lines = (length - 100) / 50;
                for i in 0..lines {
                    content.push_str(&format!(
                        "Line {:05}: 这是一段测试文本，包含各种数据用于填充内容。数字={}, 字母=ABCDEF\n",
                        i, i * 123
                    ));
                }

                content.push_str("\n=== 文件结束 ===\n");
                content.push_str(&format!("[文件共 {} 字符]\n", content.len()));

                // 在中间位置标记关键信息
                if length > 500 {
                    let mid = content.len() / 2;
                    content.insert_str(mid, ">>> 重要信息在第 100 行 <<<\n");
                }

                Ok(ToolResult::success(content))
            })
        }
    }

    // 2.1 无限制版本（返回 5000 字符）
    println!("  [2.1] 无限制版本（返回 5000 字符）");
    let mut agent_unlimited = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("unlimited_agent")
        .system_prompt("你是一个分析助手。请报告工具返回内容的长度和关键信息。")
        .build()?;

    agent_unlimited.add_tool(Box::new(ConfigurableOutputTool {
        output_length: 5000,
    }));

    println!("    执行: 调用 get_file_content 获取文件...\n");

    match agent_unlimited
        .execute("调用 get_file_content 工具，然后告诉我返回了多少字符，以及里面有什么关键信息。")
        .await
    {
        Ok(result) => {
            let char_count = result.chars().count();
            println!("    ✓ 返回结果 (共 {} 字符):", char_count);
            // 只显示前 300 字符
            let preview: String = result.chars().take(300).collect();
            println!("    {}\n    ... (更多内容省略)\n", preview);
        }
        Err(e) => {
            if char_count_str(&e.to_string()).is_some() {
                println!("    ✓ LLM 正确报告了内容长度\n");
            } else {
                println!("    错误: {}\n", e);
            }
        }
    }

    // 2.2 有限制版本（截断到 500 字符）
    println!("  {}", "─".repeat(55));
    println!("  [2.2] 限制版本 (max_tool_output_tokens=500)");
    println!("  {}", "-".repeat(55));

    let mut agent_limited = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("limited_agent")
        .system_prompt("你是一个分析助手。如果工具输出显示被截断，请在回复中说明这一点。")
        .max_tool_output_tokens(500)
        .build()?;

    agent_limited.add_tool(Box::new(ConfigurableOutputTool {
        output_length: 5000,
    }));

    println!("    执行: 调用 get_file_content（将被截断）...\n");

    match agent_limited
        .execute("调用 get_file_content 工具，然后告诉我返回了多少字符。如果输出显示被截断，请明确说明。")
        .await
    {
        Ok(result) => {
            let char_count = result.chars().count();
            println!("    ✓ 返回结果 (共 {} 字符):", char_count);
            println!("    {}\n", result);

            // 检查是否提到了截断
            if result.contains("截断") || result.contains("truncated") {
                println!("    ✓ LLM 正确识别到输出被截断\n");
            }
        }
        Err(e) => {
            println!("    错误: {}\n", e);
        }
    }

    // 2.3 对比说明
    let separator = "─".repeat(55);
    println!("  {}", separator);
    println!("  [2.3] 对比说明");
    println!("  {}", separator);
    println!();
    println!("  无限制版本:");
    println!("    - 工具返回 5000 字符 → LLM 收到全部内容");
    println!("    - 优点: 信息完整");
    println!("    - 缺点: 占用大量 token，可能超出上下文限制\n");
    println!("  限制版本 (max=500):");
    println!("    - 工具返回 5000 字符 → 自动截断到 500 字符");
    println!("    - LLM 看到: \"[输出已截断，共 5000 tokens，保留前 500]\"");
    println!("    - 优点: 控制上下文使用，防止溢出");
    println!("    - 缺点: 信息不完整，需要 LLM 主动要求更多\n");

    Ok(())
}

// 辅助函数：从错误消息中提取字符数
fn char_count_str(s: &str) -> Option<String> {
    // 匹配类似 "5000 字符" 的模式
    use regex::Regex;
    let re = Regex::new(r"(\d+)\s*字符").ok()?;
    re.captures(s).map(|cap| cap[1].to_string())
}
