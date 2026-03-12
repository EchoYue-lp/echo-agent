//! 命令行人工介入 Provider
//!
//! 这是传统的阻塞式实现，适合简单的命令行应用。
//! 对于聊天应用或 Web 应用，推荐使用 [`super::HumanLoopManager`]。

use std::io::Write as _;

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, BufReader};

use super::{HumanLoopKind, HumanLoopProvider, HumanLoopRequest, HumanLoopResponse};
use crate::error::Result;

/// 基于命令行 stdin 的人工介入 Provider（阻塞式）
///
/// 适合简单的命令行应用。对于复杂的 UI 场景，请使用 [`super::HumanLoopManager`]。
pub struct ConsoleHumanLoopProvider;

#[async_trait]
impl HumanLoopProvider for ConsoleHumanLoopProvider {
    async fn request(&self, req: HumanLoopRequest) -> Result<HumanLoopResponse> {
        match req.kind {
            HumanLoopKind::Approval => {
                println!();
                println!("╔══════════════════════════════════════════════════════════╗");
                println!("║  ⚠️  工具审批请求                                          ║");
                println!("╚══════════════════════════════════════════════════════════╝");
                println!();
                println!("工具: {}", req.tool_name.as_deref().unwrap_or("unknown"));
                if let Some(args) = &req.args {
                    let args_str = serde_json::to_string_pretty(args).unwrap_or_default();
                    let lines: Vec<&str> = args_str.lines().take(10).collect();
                    println!("参数: {}", lines.join("\n       "));
                }
                println!();
                print!("是否批准执行？(y/n): ");
                let _ = std::io::stdout().flush();

                let input = read_line().await?;
                let trimmed = input.trim();

                if trimmed.eq_ignore_ascii_case("y") || trimmed.eq_ignore_ascii_case("yes") {
                    println!("✅ 已批准");
                    Ok(HumanLoopResponse::Approved)
                } else {
                    let reason = if trimmed.is_empty() {
                        None
                    } else {
                        Some(format!("用户输入: {}", trimmed))
                    };
                    println!("❌ 已拒绝");
                    Ok(HumanLoopResponse::Rejected { reason })
                }
            }
            HumanLoopKind::Input => {
                println!();
                println!("╔══════════════════════════════════════════════════════════╗");
                println!("║  📝 Agent 请求输入                                         ║");
                println!("╚══════════════════════════════════════════════════════════╝");
                println!();
                println!("{}", req.prompt);
                print!("> ");
                let _ = std::io::stdout().flush();

                let input = read_line().await?;
                Ok(HumanLoopResponse::Text(input.trim().to_string()))
            }
        }
    }
}

async fn read_line() -> Result<String> {
    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin);
    let mut buf = String::new();
    reader.read_line(&mut buf).await?;
    Ok(buf)
}
