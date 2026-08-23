//! 综合示例：代码实验室助手
//!
//! 展示 echo-agent 在代码执行场景中的完整能力：
//!
//! ## 功能清单
//!
//! | 功能模块 | 实现方式 |
//! |---------|---------|
//! | 沙箱执行 | `LocalSandbox` 安全代码执行 |
//! | 重试策略 | `RetryPolicy` + `with_retry()` 处理网络失败 |
//! | 护栏系统 | `RuleGuard` 过滤危险命令 |
//! | 审计日志 | `InMemoryAuditLogger` 记录所有操作 |
//! | 自定义工具 | `#[tool]` 宏定义代码相关工具 |
//! | 流式输出 | `execute_stream()` 实时显示执行进度 |
//!
//! ## 运行方式
//!
//! ```bash
//! # 基础运行（需要 LLM API Key）
//! QWEN_API_KEY=your_key cargo run --example demo44_code_laboratory
//! ```

use echo_agent::audit::AuditEvent;
use echo_agent::guard::GuardDirection;
use echo_agent::prelude::*;
use echo_agent::retry::{RetryPolicy, with_retry};
use echo_agent::sandbox::local::LocalConfig;
use echo_agent::sandbox::{LocalSandbox, SandboxCommand};
use echo_agent::tool;
use futures::StreamExt;
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// 工具定义
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

static EXECUTION_ID: AtomicU32 = AtomicU32::new(1);

#[tool(name = "execute_python", description = "在安全沙箱中执行 Python 代码")]
async fn execute_python(
    /// Python 代码
    code: String,
) -> Result<ToolResult> {
    let id = EXECUTION_ID.fetch_add(1, Ordering::SeqCst);

    // 创建本地沙箱
    let sandbox = LocalSandbox::new(LocalConfig {
        enable_os_sandbox: false,
        ..Default::default()
    });

    let cmd = SandboxCommand::program("python3", vec!["-c".to_string(), code.clone()]);

    println!("    [执行 #{}] 正在运行 Python 代码...", id);

    match sandbox.execute(cmd).await {
        Ok(result) => {
            if result.success() {
                let output = if !result.stdout.is_empty() {
                    result.stdout
                } else {
                    result.stderr
                };
                Ok(ToolResult::success(output))
            } else {
                Ok(ToolResult::error(format!(
                    "执行失败 (exit code {}): {}",
                    result.exit_code, result.stderr
                )))
            }
        }
        Err(e) => Ok(ToolResult::error(e.to_string())),
    }
}

#[tool(name = "analyze_code", description = "分析代码质量和潜在问题")]
async fn analyze_code(
    /// 代码内容
    code: String,
    /// 编程语言
    language: String,
) -> Result<ToolResult> {
    let issues = vec![
        "未处理异常: 第5行可能抛出异常".to_string(),
        "性能建议: 使用列表推导替代循环".to_string(),
    ];

    let result = json!({
        "language": language,
        "lines": code.lines().count(),
        "issues": issues,
        "complexity": "中等"
    });

    Ok(ToolResult::success(result.to_string()))
}

#[tool(name = "check_syntax", description = "检查代码语法错误")]
async fn check_syntax(
    /// 代码内容
    code: String,
    /// 编程语言
    language: String,
) -> Result<ToolResult> {
    // 简单的语法检查模拟
    let has_error = code.contains("syntax error") || code.contains("missing colon");

    if has_error {
        Ok(ToolResult::error("发现语法错误".to_string()))
    } else {
        Ok(ToolResult::success(
            json!({
                "status": "valid",
                "language": language
            })
            .to_string(),
        ))
    }
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Main
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "echo_agent=info,code_lab=info".into()),
        )
        .init();

    print_banner();

    // ── Part 1: 沙箱执行系统 ───────────────────────────────────────────────────
    demo_sandbox_execution().await?;

    // ── Part 2: 重试策略演示 ─────────────────────────────────────────────────────
    demo_retry_policy().await?;

    // ── Part 3: 护栏系统 ─────────────────────────────────────────────────────────
    demo_guard_system().await?;

    // ── Part 4: 审计日志 ─────────────────────────────────────────────────────────
    demo_audit_logging().await?;

    // ── Part 5: 综合代码分析 ─────────────────────────────────────────────────────
    demo_code_analysis().await?;

    println!("\n═══════════════════════════════════════════════════════");
    println!("              综合示例演示完成！");
    println!("═══════════════════════════════════════════════════════");

    Ok(())
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Part 1: 沙箱执行系统
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

async fn demo_sandbox_execution() -> Result<()> {
    println!("═══════════════════════════════════════════════════════");
    println!("Part 1: 沙箱执行系统");
    println!("═══════════════════════════════════════════════════════\n");

    let sandbox = LocalSandbox::new(LocalConfig {
        enable_os_sandbox: false,
        ..Default::default()
    });

    let test_cases = vec![
        ("安全: 简单计算", "python3 -c \"print(2 + 2)\""),
        ("安全: 数据处理", "python3 -c \"print(sum([1,2,3,4,5]))\""),
        ("安全: 日期命令", "date"),
    ];

    println!("  测试沙箱执行:\n");

    let mut success_count = 0usize;
    for (desc, cmd) in test_cases {
        println!("  [{}] {}", desc, cmd);

        match sandbox.execute(SandboxCommand::shell(cmd)).await {
            Ok(result) => {
                if result.success() {
                    success_count += 1;
                    println!("    ✓ 执行成功: {}", result.stdout.trim());
                } else {
                    return Err(echo_agent::error::ReactError::Other(format!(
                        "综合验收失败：沙箱命令 `{cmd}` 执行失败 (exit code {}): {}",
                        result.exit_code,
                        result.stderr.trim()
                    )));
                }
                println!("    耗时: {:?}", result.duration);
            }
            Err(e) => {
                return Err(echo_agent::error::ReactError::Other(format!(
                    "综合验收失败：沙箱命令 `{cmd}` 执行错误: {e}"
                )));
            }
        }
        println!();
    }

    if success_count != 3 {
        return Err(echo_agent::error::ReactError::Other(format!(
            "综合验收失败：仅有 {success_count} 个沙箱命令成功"
        )));
    }

    Ok(())
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Part 2: 重试策略演示
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

async fn demo_retry_policy() -> Result<()> {
    println!("═══════════════════════════════════════════════════════");
    println!("Part 2: 重试策略");
    println!("═══════════════════════════════════════════════════════\n");

    // 配置重试策略
    let policy = RetryPolicy::new(3, Duration::from_millis(100))
        .max_delay(Duration::from_secs(1))
        .jitter(true);

    println!("  重试策略配置:");
    println!("    最大重试: {} 次", policy.max_retries);
    println!("    基础延迟: {:?}", policy.base_delay);
    println!("    最大延迟: {:?}", policy.max_delay);
    println!("    抖动: {}\n", policy.jitter);

    // 模拟不稳定的外部API调用
    let attempt = Arc::new(AtomicU32::new(0));
    let a = attempt.clone();
    let unstable_api = move || {
        let a = a.clone();
        async move {
            let current = a.fetch_add(1, Ordering::SeqCst);
            if current < 3 {
                Err(std::io::Error::new(
                    std::io::ErrorKind::ConnectionReset,
                    "连接重置",
                ))
            } else {
                Ok("API响应成功".to_string())
            }
        }
    };

    println!("  模拟不稳定API调用:\n");

    let result = with_retry(&policy, unstable_api).await?;
    let attempts = attempt.load(Ordering::SeqCst);
    if result != "API响应成功" || attempts != 4 {
        return Err(echo_agent::error::ReactError::Other(format!(
            "综合验收失败：重试结果不符合预期（result={result}, attempts={attempts}）"
        )));
    }
    println!("    ✓ 成功: {} (经过 {} 次尝试)", result, attempts);
    println!();

    Ok(())
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Part 3: 护栏系统
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

async fn demo_guard_system() -> Result<()> {
    println!("═══════════════════════════════════════════════════════");
    println!("Part 3: 护栏系统");
    println!("═══════════════════════════════════════════════════════\n");

    // 创建输入护栏
    let input_guard = Arc::new(
        RuleGuardBuilder::new("code-security")
            .blocked_keyword("rm -rf")
            .blocked_keyword("DROP TABLE")
            .blocked_pattern(r"(?i)(password|secret|token)")
            .max_length(10000)
            .direction(GuardDirection::Input)
            .build(),
    );

    println!("  护栏规则:");
    println!("    • 阻止危险文件操作命令 (rm -rf)");
    println!("    • 阻止危险SQL语句 (DROP TABLE)");
    println!("    • 阻止敏感关键词 (password, secret, token)");
    println!("    • 最大输入长度: 10000 字符\n");

    // 测试用例
    let normal = "请帮我计算 2 + 3 的结果";
    let dangerous_file = "请执行 rm -rf /home/user/documents";
    let dangerous_sql = "执行 DROP TABLE users;";
    let long_content = "x".repeat(15000);

    let test_inputs: Vec<(&str, &str)> = vec![
        ("正常: 计算两数之和", normal),
        ("危险: 文件删除", dangerous_file),
        ("危险: SQL注入", dangerous_sql),
        ("超长: 长内容检测", &long_content),
    ];

    let mut blocked = 0usize;
    let mut passed = 0usize;
    for (desc, input) in test_inputs {
        println!("  [测试] {}", desc);
        match input_guard.check(input, GuardDirection::Input).await {
            Ok(GuardResult::Pass) => {
                passed += 1;
                println!("    ✓ 通过: 内容检查");
            }
            Ok(GuardResult::Block { reason }) => {
                blocked += 1;
                println!("    🚫 阻止: {}", reason);
            }
            Ok(GuardResult::Warn { reasons }) => {
                passed += 1;
                println!("    ⚠️ 告警: {}", reasons.join("；"));
            }
            Ok(GuardResult::Transform { content, reasons }) => {
                passed += 1;
                println!("    ⚠️ 已变换: {content} ({})", reasons.join("；"));
            }
            Err(e) => {
                return Err(echo_agent::error::ReactError::Other(format!(
                    "综合验收失败：护栏执行出错: {e}"
                )));
            }
        }
        println!();
    }

    if passed != 1 || blocked != 3 {
        return Err(echo_agent::error::ReactError::Other(format!(
            "综合验收失败：护栏结果不符合预期（passed={passed}, blocked={blocked}）"
        )));
    }

    Ok(())
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Part 4: 审计日志
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

async fn demo_audit_logging() -> Result<()> {
    println!("═══════════════════════════════════════════════════════");
    println!("Part 4: 审计日志");
    println!("═══════════════════════════════════════════════════════\n");

    use echo_agent::audit::{AuditEventType, AuditLogger};

    let logger = Arc::new(InMemoryAuditLogger::new());

    // 记录一些审计事件
    let events = vec![
        AuditEvent::now(
            Some("session-123".to_string()),
            "code-lab".to_string(),
            AuditEventType::UserInput {
                content: "请执行这段代码".to_string(),
            },
        ),
        AuditEvent::now(
            Some("session-123".to_string()),
            "code-lab".to_string(),
            AuditEventType::ToolCall {
                tool: "execute_python".to_string(),
                input: json!({"code": "print(1+1)"}),
                output: "2".to_string(),
                success: true,
                duration_ms: 150,
            },
        ),
        AuditEvent::now(
            Some("session-123".to_string()),
            "code-lab".to_string(),
            AuditEventType::FinalAnswer {
                content: "代码执行成功，结果为 2".to_string(),
            },
        ),
    ];

    for event in events {
        logger.log(event).await?;
    }

    println!("  ✓ 已记录 3 条审计事件\n");

    // 查询审计日志
    use echo_agent::audit::AuditFilter;
    let audit_events = logger.query(AuditFilter::default()).await?;
    if audit_events.len() != 3 {
        return Err(echo_agent::error::ReactError::Other(format!(
            "综合验收失败：审计日志条数不符合预期（{}）",
            audit_events.len()
        )));
    }

    println!("  审计日志记录:\n");
    for (i, event) in audit_events.iter().enumerate() {
        let time = event.timestamp.format("%H:%M:%S");
        let summary = match &event.event_type {
            AuditEventType::UserInput { content } => {
                format!("输入: {}", content.chars().take(30).collect::<String>())
            }
            AuditEventType::ToolCall { tool, success, .. } => {
                format!("工具: {} (成功: {})", tool, success)
            }
            AuditEventType::FinalAnswer { .. } => "最终答案".to_string(),
            _ => format!("{:?}", event.event_type),
        };
        println!("    [{}] {} - {}", i + 1, time, summary);
    }
    println!();

    Ok(())
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Part 5: 综合代码分析
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

async fn demo_code_analysis() -> Result<()> {
    println!("═══════════════════════════════════════════════════════");
    println!("Part 5: 综合代码分析");
    println!("═══════════════════════════════════════════════════════\n");

    let mut agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("code-analyst")
        .system_prompt(
            "你是代码分析专家，能够：
1. 使用 check_syntax 检查代码语法
2. 使用 analyze_code 分析代码质量
3. 使用 execute_python 在沙箱中执行代码
4. 最后给出分析报告",
        )
        .enable_tools()
        .max_iterations(10)
        .build()?;

    // 添加自定义工具
    agent.add_tool(Box::new(CheckSyntaxTool));
    agent.add_tool(Box::new(AnalyzeCodeTool));
    agent.add_tool(Box::new(ExecutePythonTool));

    println!("  可用工具: {:?}\n", agent.tool_names());

    let task = r#"请分析以下 Python 代码：

def calculate_fibonacci(n):
    if n <= 1:
        return n
    return calculate_fibonacci(n-1) + calculate_fibonacci(n-2)

result = calculate_fibonacci(10)
print(f"Fibonacci(10) = {result}")

请执行以下步骤：
1. 检查语法是否正确
2. 分析代码质量和潜在问题
3. 在沙箱中执行代码验证结果
4. 给出改进建议"#;

    println!("  📋 分析任务:\n{}\n", task);
    println!("  执行中...\n");

    let mut stream = agent.execute_stream(task).await?;
    let mut used_syntax = false;
    let mut used_analysis = false;
    let mut used_execution = false;
    let mut final_answer = String::new();

    while let Some(event) = stream.next().await {
        match event? {
            AgentEvent::ThinkStart => print!("🤔 "),
            AgentEvent::ThinkEnd { .. } => println!(),
            AgentEvent::ToolCall { invocation, .. } => {
                let name = invocation.name;
                match name.as_str() {
                    "check_syntax" => used_syntax = true,
                    "analyze_code" => used_analysis = true,
                    "execute_python" => used_execution = true,
                    _ => {}
                }
                println!("🔧 使用工具: {}", name);
            }
            AgentEvent::ToolResult { result, .. } => {
                let preview: String = result.output.chars().take(100).collect();
                println!("   ✓ 结果: {}...", preview);
            }
            AgentEvent::Token(token) => {
                final_answer.push_str(&token);
                print!("{}", token);
            }
            AgentEvent::FinalAnswer(_) => println!(),
            _ => {}
        }
    }

    if !used_syntax || !used_analysis || !used_execution {
        return Err(echo_agent::error::ReactError::Other(format!(
            "综合验收失败：综合代码分析未完整调用工具（syntax={used_syntax}, analyze={used_analysis}, execute={used_execution}）"
        )));
    }
    if final_answer.trim().is_empty() {
        return Err(echo_agent::error::ReactError::Other(
            "综合验收失败：综合代码分析返回空答案".to_string(),
        ));
    }

    println!();

    Ok(())
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// 辅助函数
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

fn print_banner() {
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║          Echo Agent 代码实验室助手 - 综合示例               ║");
    println!("║                                                                ║");
    println!("║  展示核心能力：                                                 ║");
    println!("║  • 沙箱执行 • 重试策略 • 护栏系统 • 审计日志                  ║");
    println!("║  • 自定义工具 • 流式输出                                       ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");
}
