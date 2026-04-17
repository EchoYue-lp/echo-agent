//! 命令行人工介入 Provider
//!
//! 传统的阻塞式实现，适合简单的命令行应用。
//! 对于聊天应用或 Web 应用，推荐使用 [`super::HumanLoopManager`]。
//!
//! ## 功能
//!
//! - 风险等级可视化（颜色 + 标签）
//! - 超时倒计时提示
//! - 参数编辑（修改 JSON 后批准）
//! - 会话级审批（"Allow for session" 语义）

use std::io::Write as _;
use std::time::Duration;

use futures::future::BoxFuture;
use tokio::io::{AsyncBufReadExt, BufReader};

use super::{
    ApprovalScope, HumanLoopKind, HumanLoopProvider, HumanLoopRequest, HumanLoopResponse, RiskLevel,
};
use echo_core::error::Result;

/// 基于命令行 stdin 的人工介入 Provider（阻塞式）
///
/// 适合简单的命令行应用。对于复杂的 UI 场景，请使用 [`super::HumanLoopManager`]。
pub struct ConsoleHumanLoopProvider;

impl HumanLoopProvider for ConsoleHumanLoopProvider {
    fn request(&self, req: HumanLoopRequest) -> BoxFuture<'_, Result<HumanLoopResponse>> {
        Box::pin(async move {
            match req.kind {
                HumanLoopKind::Approval => handle_approval(req).await,
                HumanLoopKind::Input => handle_input(req).await,
            }
        })
    }
}

// ── 审批处理 ──────────────────────────────────────────────────────────────────

async fn handle_approval(req: HumanLoopRequest) -> Result<HumanLoopResponse> {
    let risk_level = req.risk_level.unwrap_or(RiskLevel::Medium);
    let tool_name = req.tool_name.as_deref().unwrap_or("unknown");

    // 风险等级可视化
    print_approval_banner(tool_name, risk_level);

    // 参数展示
    if let Some(args) = &req.args {
        print_args(args);
    }

    // 操作菜单
    print_actions(risk_level, req.timeout.as_ref());

    let input = read_line_with_timeout(req.timeout).await?;

    let trimmed = input.trim().to_lowercase();
    match trimmed.as_str() {
        "y" | "yes" | "" => {
            println!("  Approved");
            Ok(HumanLoopResponse::Approved)
        }
        "s" | "session" => {
            println!("  Approved for this session");
            Ok(HumanLoopResponse::ApprovedWithScope {
                scope: ApprovalScope::Session,
            })
        }
        "a" | "all" => {
            println!("  Approved all tools for this session");
            Ok(HumanLoopResponse::ApprovedWithScope {
                scope: ApprovalScope::SessionAllTools,
            })
        }
        "e" | "edit" => handle_edit_args(&req).await,
        "d" | "defer" => {
            println!("  Deferred");
            Ok(HumanLoopResponse::Deferred)
        }
        _ => {
            let reason = if trimmed.is_empty() {
                None
            } else {
                Some(format!("user input: {trimmed}"))
            };
            println!("  Rejected");
            Ok(HumanLoopResponse::Rejected { reason })
        }
    }
}

// ── 输入处理 ──────────────────────────────────────────────────────────────────

async fn handle_input(req: HumanLoopRequest) -> Result<HumanLoopResponse> {
    println!();
    println!("  Agent requests input:");
    println!("  {}", req.prompt);
    println!();

    if let Some(timeout) = &req.timeout {
        print!("  [timeout: {}s] > ", timeout.as_secs());
    } else {
        print!("  > ");
    }
    let _ = std::io::stdout().flush();

    let input = read_line_with_timeout(req.timeout).await?;
    Ok(HumanLoopResponse::Text(input.trim().to_string()))
}

// ── 参数编辑 ──────────────────────────────────────────────────────────────────

async fn handle_edit_args(req: &HumanLoopRequest) -> Result<HumanLoopResponse> {
    let original_args = req.args.clone().unwrap_or_default();
    let original_json = serde_json::to_string_pretty(&original_args).unwrap_or_default();

    println!();
    println!("  --- Edit parameters ---");
    println!("  Current JSON (press Enter to keep, or paste new JSON):");
    println!();

    // 显示当前参数，每行加缩进
    for line in original_json.lines() {
        println!("    {line}");
    }

    println!();
    println!("  New JSON (empty line = keep original):");
    print!("  > ");
    let _ = std::io::stdout().flush();

    let input = read_line_with_timeout(req.timeout).await?;
    let trimmed = input.trim();

    let new_args = if trimmed.is_empty() {
        // 保持原参数不变
        original_args
    } else {
        match serde_json::from_str::<serde_json::Value>(trimmed) {
            Ok(v) => {
                println!("  Parameters updated, approved");
                v
            }
            Err(e) => {
                println!("  Invalid JSON: {e}, using original parameters");
                original_args
            }
        }
    };

    // 编辑完成后询问 scope
    println!();
    println!("  Scope: (o) once  (s) session  (a) all tools");
    print!("  > ");
    let _ = std::io::stdout().flush();

    let scope_input = read_line_with_timeout(req.timeout).await?;
    let scope = match scope_input.trim().to_lowercase().as_str() {
        "s" | "session" => ApprovalScope::Session,
        "a" | "all" => ApprovalScope::SessionAllTools,
        _ => ApprovalScope::Once,
    };

    Ok(HumanLoopResponse::ModifiedArgs {
        args: new_args,
        scope,
    })
}

// ── 显示辅助 ──────────────────────────────────────────────────────────────────

/// 打印审批请求的横幅，根据风险等级使用不同的视觉提示
fn print_approval_banner(tool_name: &str, risk_level: RiskLevel) {
    let (icon, label): (&str, &str) = match risk_level {
        RiskLevel::Low => ("  [LOW]", "low risk"),
        RiskLevel::Medium => ("  [MED]", "medium risk"),
        RiskLevel::High => ("  [HIGH]", "high risk"),
        RiskLevel::Critical => ("  [CRIT]", "CRITICAL risk"),
    };
    let inner_width: usize = 56;

    let title = format!("{icon}  Tool approval request [{label}]");
    let inner_width = inner_width.saturating_sub(4);

    println!();
    println!("  +{:-<inner_width$}+", "");
    println!("  | {:<inner_width$} |", title);
    println!("  +{:-<inner_width$}+", "");
    println!("  Tool: {tool_name}");
}

/// 格式化打印工具参数（限制显示行数）
fn print_args(args: &serde_json::Value) {
    let args_str = serde_json::to_string_pretty(args).unwrap_or_default();
    let lines: Vec<&str> = args_str.lines().collect();

    println!("  Parameters:");
    let display_limit = 15;
    for line in lines.iter().take(display_limit) {
        println!("    {line}");
    }
    if lines.len() > display_limit {
        println!("    ... ({} more lines)", lines.len() - display_limit);
    }
    println!();
}

/// 打印操作菜单
fn print_actions(risk_level: RiskLevel, timeout: Option<&Duration>) {
    let risk_note = if risk_level == RiskLevel::Critical {
        "  WARNING: Critical risk — requires explicit approval + reason"
    } else {
        ""
    };

    if !risk_note.is_empty() {
        println!("{risk_note}");
        println!();
    }

    print!("  Actions: (y) approve  (s) session  (a) all  (e) edit  (n) reject  (d) defer");

    if let Some(dur) = timeout {
        print!("  [timeout: {}s]", dur.as_secs());
    }

    println!();
    print!("  > ");
    let _ = std::io::stdout().flush();
}

// ── 读取辅助 ──────────────────────────────────────────────────────────────────

/// 带可选超时的 stdin 读取
async fn read_line_with_timeout(timeout: Option<Duration>) -> Result<String> {
    match timeout {
        Some(dur) => match tokio::time::timeout(dur, read_line()).await {
            Ok(result) => result,
            Err(_) => {
                println!();
                println!("  Timeout");
                Err(echo_core::error::ReactError::Other(
                    "Approval timeout".to_string(),
                ))
            }
        },
        None => read_line().await,
    }
}

async fn read_line() -> Result<String> {
    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin);
    let mut buf = String::new();
    reader.read_line(&mut buf).await?;
    Ok(buf)
}

// ── 单元测试 ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_print_approval_banner_low() {
        // Should not panic for any risk level
        print_approval_banner("Read", RiskLevel::Low);
        print_approval_banner("Bash", RiskLevel::Medium);
        print_approval_banner("Bash", RiskLevel::High);
        print_approval_banner("DangerousOp", RiskLevel::Critical);
    }

    #[test]
    fn test_print_args_simple() {
        print_args(&json!({"path": "/tmp/test"}));
    }

    #[test]
    fn test_print_args_large() {
        // Create a large JSON that exceeds 15 lines
        let mut map = serde_json::Map::new();
        for i in 0..20 {
            map.insert(format!("key_{i}"), json!(format!("value_{i}")));
        }
        print_args(&serde_json::Value::Object(map));
    }

    #[test]
    fn test_print_actions_no_timeout() {
        print_actions(RiskLevel::Medium, None);
        print_actions(RiskLevel::Critical, Some(&Duration::from_secs(30)));
    }

    #[test]
    fn test_human_loop_request_approval_with_risk() {
        let req =
            HumanLoopRequest::approval_with_risk("Bash", json!({"cmd": "ls"}), RiskLevel::High);
        assert_eq!(req.kind, HumanLoopKind::Approval);
        assert_eq!(req.risk_level, Some(RiskLevel::High));
    }

    #[test]
    fn test_human_loop_request_approval_with_timeout() {
        let req = HumanLoopRequest::approval_with_timeout(
            "Bash",
            json!({"cmd": "ls"}),
            Duration::from_secs(10),
        );
        assert_eq!(req.kind, HumanLoopKind::Approval);
        assert_eq!(req.timeout, Some(Duration::from_secs(10)));
    }
}
