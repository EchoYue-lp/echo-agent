use std::io::Write as _;

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, BufReader};

use super::{HumanLoopKind, HumanLoopProvider, HumanLoopRequest, HumanLoopResponse};
use crate::error::Result;

/// 基于命令行 stdin 的人工介入 Provider（异步，不阻塞 tokio 工作线程）。
pub struct ConsoleHumanLoopProvider;

#[async_trait]
impl HumanLoopProvider for ConsoleHumanLoopProvider {
    async fn request(&self, req: HumanLoopRequest) -> Result<HumanLoopResponse> {
        match req.kind {
            HumanLoopKind::Approval => {
                println!("\n⚠️  {}", req.prompt);
                if let Some(args) = &req.args {
                    println!(
                        "参数:\n{}",
                        serde_json::to_string_pretty(args).unwrap_or_default()
                    );
                }
                print!("(y/n): ");
                let _ = std::io::stdout().flush();

                let input = read_line().await?;
                let trimmed = input.trim();
                if trimmed.eq_ignore_ascii_case("y") || trimmed.eq_ignore_ascii_case("yes") {
                    Ok(HumanLoopResponse::Approved)
                } else {
                    Ok(HumanLoopResponse::Rejected { reason: None })
                }
            }
            HumanLoopKind::Input => {
                println!("\n{}", req.prompt);
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
