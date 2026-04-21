//! demo33_retry_policy —— 统一错误恢复 + Circuit Breaker 完整演示
//!
//! 演示 `echo-core` 中的 `RetryPolicy` 和 `with_retry` / `with_retry_if` API，
//! 所有外部调用（LLM / MCP / A2A / Sandbox）统一使用。
//!
//! # 核心概念
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    重试策略层级                             │
//! ├─────────────────────────────────────────────────────────────┤
//! │ ① 工具级重试    │ agent 内部处理单个工具失败               │
//! │ ② LLM 调用重试  │ llm_max_retries 配置项                    │
//! │ ③ 统一重试策略  │ with_retry / with_retry_if 核心函数      │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! # 适用场景
//!
//! | 错误类型 | 是否重试 | 原因 |
//! |----------|----------|------|
//! | 网络超时 | ✅ | 临时性问题 |
//! | 429 Too Many Requests | ✅ | 限流，等待后可重试 |
//! | 5xx 服务器错误 | ✅ | 服务端临时故障 |
//! | 4xx 客户端错误 | ❌ | 参数错误，重试无意义 |
//! | 认证失败 | ❌ | 需要更新凭证 |
//!
//! # 运行方式
//!
//! ```bash
//! cargo run --example demo33_retry_policy
//! ```

use echo_agent::prelude::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

#[tokio::main]
async fn main() -> Result<()> {
    println!("═══ Unified Retry Policy Demo ═══\n");

    // ── Part 1: RetryPolicy 构建 ───────────────────────────────────────────────
    demo_policy_builder();

    // ── Part 2: 指数退避延迟计算 ───────────────────────────────────────────────
    demo_backoff_calculation();

    // ── Part 3: with_retry 基础场景 ────────────────────────────────────────────
    demo_with_retry_basic().await;

    // ── Part 4: with_retry_if 选择性重试 ───────────────────────────────────────
    demo_with_retry_if().await;

    // ── Part 5: 实际 LLM 调用重试演示 ──────────────────────────────────────────
    demo_llm_retry().await?;

    // ── Part 6: 推荐使用模式 ───────────────────────────────────────────────────
    demo_best_practices();

    println!("\n═══ Demo Complete ═══");
    Ok(())
}

// ── Part 1: RetryPolicy 构建 ─────────────────────────────────────────────────────

fn demo_policy_builder() {
    println!("─────────────────────────────────────────────");
    println!("Part 1: RetryPolicy 构建方式");
    println!("─────────────────────────────────────────────\n");

    // 1.1 默认策略
    let default_policy = RetryPolicy::default();
    println!("  [1.1] 默认策略 (RetryPolicy::default())");
    println!(
        "    max_retries = {}, base_delay = {:?}, jitter = {}",
        default_policy.max_retries, default_policy.base_delay, default_policy.jitter
    );
    println!();

    // 1.2 自定义策略
    let custom = RetryPolicy::new(5, Duration::from_millis(200))
        .max_delay(Duration::from_secs(10))
        .jitter(true);
    println!("  [1.2] 自定义策略");
    println!("    RetryPolicy::new(5, 200ms).max_delay(10s).jitter(true)");
    println!(
        "    → max_retries={}, base_delay={:?}, max_delay={:?}, jitter={}",
        custom.max_retries, custom.base_delay, custom.max_delay, custom.jitter
    );
    println!();

    // 1.3 预设策略
    println!("  [1.3] 预设策略");
    println!("    RetryPolicy::no_retry()     → max_retries=0");
    println!("    RetryPolicy::aggressive()  → 更多重试，更长延迟");
    println!("    RetryPolicy::conservative()→ 少量重试，快速失败");
    println!();
}

// ── Part 2: 指数退避延迟计算 ─────────────────────────────────────────────────

fn demo_backoff_calculation() {
    println!("─────────────────────────────────────────────");
    println!("Part 2: 指数退避延迟计算");
    println!("─────────────────────────────────────────────\n");

    println!("  策略: max_retries=5, base_delay=100ms, max_delay=2s, jitter=false\n");

    let policy = RetryPolicy::new(5, Duration::from_millis(100))
        .max_delay(Duration::from_secs(2))
        .jitter(false);

    println!("  重试次数 | 延迟计算 | 实际延迟");
    println!("  ---------|----------|----------");
    for attempt in 0..=5 {
        let delay = policy.delay_for(attempt);
        let formula = if attempt == 0 {
            "0 ms (首次)".to_string()
        } else {
            format!("min(100ms × 2^{}, 2000ms) = {:?}", attempt - 1, delay)
        };
        println!("    {}     | {} | {:?}", attempt, formula, delay);
    }

    println!("\n  说明:");
    println!("    - 首次(attempt=0): 立即执行，无延迟");
    println!("    - 第1次重试(attempt=1): 100ms × 2^0 = 100ms");
    println!("    - 第2次重试(attempt=2): 100ms × 2^1 = 200ms");
    println!("    - 第3次重试(attempt=3): 100ms × 2^2 = 400ms");
    println!("    - 第4次重试(attempt=4): 100ms × 2^3 = 800ms");
    println!("    - 第5次重试(attempt=5): 100ms × 2^4 = 1600ms");
    println!("    - 继续重试: 受 max_delay=2s 限制\n");
}

// ── Part 3: with_retry 基础场景 ─────────────────────────────────────────────────

async fn demo_with_retry_basic() {
    println!("─────────────────────────────────────────────");
    println!("Part 3: with_retry 基础场景");
    println!("─────────────────────────────────────────────\n");

    // 3.1 首次成功
    println!("  [3.1] 首次成功");
    let result = with_retry(&RetryPolicy::no_retry(), || async {
        Ok::<_, String>("成功")
    })
    .await;
    println!("    结果: {:?}\n", result.unwrap());

    // 3.2 前两次失败，第三次成功
    println!("  [3.2] 前两次失败，第三次成功");
    let counter = Arc::new(AtomicU32::new(0));
    let c = counter.clone();
    let policy = RetryPolicy::new(3, Duration::from_millis(10)).jitter(false);

    let start = Instant::now();
    let result = with_retry(&policy, || {
        let c = c.clone();
        async move {
            let n = c.fetch_add(1, Ordering::SeqCst);
            if n < 2 {
                Err(format!("尝试 {} 失败", n + 1))
            } else {
                Ok(format!("第 {} 次成功", n + 1))
            }
        }
    })
    .await;

    println!("    {}", result.unwrap());
    println!(
        "    总调用次数: {}, 耗时: {:?}\n",
        counter.load(Ordering::SeqCst),
        start.elapsed()
    );

    // 3.3 全部失败（耗尽重试）
    println!("  [3.3] 全部失败（耗尽重试）");
    let counter = Arc::new(AtomicU32::new(0));
    let c = counter.clone();
    let policy = RetryPolicy::new(2, Duration::from_millis(5)).jitter(false);

    let result = with_retry(&policy, || {
        let c = c.clone();
        async move {
            c.fetch_add(1, Ordering::SeqCst);
            Err::<(), _>("服务暂时不可用".to_string())
        }
    })
    .await;

    println!("    错误: {}", result.unwrap_err());
    println!(
        "    总调用次数: {} (1 初始 + 2 重试)\n",
        counter.load(Ordering::SeqCst)
    );
}

// ── Part 4: with_retry_if 选择性重试 ───────────────────────────────────────────

async fn demo_with_retry_if() {
    println!("─────────────────────────────────────────────");
    println!("Part 4: with_retry_if 选择性重试");
    println!("─────────────────────────────────────────────\n");

    // 4.1 仅对可恢复错误重试
    println!("  [4.1] 区分可恢复和致命错误");

    #[derive(Debug)]
    enum ApiError {
        Recoverable(String),
        Fatal(String),
    }

    impl std::fmt::Display for ApiError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                ApiError::Recoverable(s) => write!(f, "Recoverable: {}", s),
                ApiError::Fatal(s) => write!(f, "Fatal: {}", s),
            }
        }
    }

    let counter = Arc::new(AtomicU32::new(0));
    let c = counter.clone();

    let result = with_retry_if(
        &RetryPolicy::new(5, Duration::from_millis(5)),
        || {
            let c = c.clone();
            async move {
                let n = c.fetch_add(1, Ordering::SeqCst);
                match n {
                    0 => Err(ApiError::Recoverable("网络超时".to_string())),
                    1 => Err(ApiError::Fatal("认证失败".to_string())),
                    _ => Ok(format!("成功 (第 {} 次)", n + 1)),
                }
            }
        },
        |e: &ApiError| matches!(e, ApiError::Recoverable(_)), // 只重试可恢复错误
    )
    .await;

    println!("    错误: {:?}", result.unwrap_err());
    println!(
        "    总调用次数: {} (第2次遇到致命错误，立即返回)\n",
        counter.load(Ordering::SeqCst)
    );

    // 4.2 实际应用场景
    println!("  [4.2] 实际应用：HTTP 状态码判断");
    println!("    with_retry_if(&policy, || http_request(), |e| {{");
    println!("        matches!(e.status(), Some(408 | 429 | 500..=599))");
    println!("    }}).await?;");
    println!();
}

// ── Part 5: 实际 LLM 调用重试演示 ─────────────────────────────────────────────

async fn demo_llm_retry() -> Result<()> {
    println!("─────────────────────────────────────────────");
    println!("Part 5: 实际 LLM 调用重试演示");
    println!("─────────────────────────────────────────────\n");

    use echo_agent::llm::chat;
    use echo_agent::llm::types::Message;

    let client = reqwest::Client::new();

    // 5.1 模拟网络波动场景
    println!("  [5.1] 模拟网络波动（前两次失败）");

    let call_count = Arc::new(AtomicU32::new(0));
    let c = call_count.clone();
    if std::env::var("OPENAI_API_KEY").is_err()
        && std::env::var("DEEPSEEK_API_KEY").is_err()
        && std::env::var("QWEN_API_KEY").is_err()
    {
        return Err(echo_agent::error::ReactError::Other(
            "demo33 验收失败：未检测到任何 LLM API 密钥".to_string(),
        )
        .into());
    }

    // 模拟带重试的 LLM 调用
    let policy = RetryPolicy::new(3, Duration::from_millis(500)).jitter(true);

    let result = with_retry(&policy, || {
        let c = c.clone();
        let client = client.clone();
        async move {
            let n = c.fetch_add(1, Ordering::SeqCst);
            println!("    → 第 {} 次 LLM 调用...", n + 1);

            // 前两次模拟失败
            if n < 2 {
                return Err(format!("模拟网络错误 (尝试 {})", n));
            }

            // 第三次真正调用
            let messages = vec![Message::user("用一句话介绍 Rust".to_string())];
            chat(
                client.into(),
                "qwen3-max",
                &messages,
                None,
                None,
                Some(false),
                None,
                None,
                None,
            )
            .await
            .map_err(|e| format!("LLM 调用失败: {}", e))
            .map(|resp| {
                resp.choices
                    .first()
                    .and_then(|c| c.message.content.as_text())
                    .unwrap_or_default()
            })
        }
    })
    .await;

    match result {
        Ok(answer) => {
            if answer.trim().is_empty() {
                return Err(echo_agent::error::ReactError::Other(
                    "demo33 验收失败：LLM 重试最终返回空答案".to_string(),
                )
                .into());
            }
            println!("    ✓ 最终成功: {}\n", answer);
        }
        Err(e) => {
            return Err(echo_agent::error::ReactError::Other(format!(
                "demo33 验收失败：LLM 重试最终失败: {}",
                e
            ))
            .into());
        }
    }
    Ok(())
}

// ── Part 6: 推荐使用模式 ───────────────────────────────────────────────────────

fn demo_best_practices() {
    println!("─────────────────────────────────────────────");
    println!("Part 6: 推荐使用模式");
    println!("─────────────────────────────────────────────\n");

    println!("  [6.1] LLM 调用重试");
    println!("    let policy = RetryPolicy::new(3, Duration::from_millis(500))");
    println!("        .jitter(true)  // 添加抖动避免惊群效应");
    println!("        .max_delay(Duration::from_secs(5));");
    println!("    let response = with_retry(&policy, || {{");
    println!("        llm_client.chat(request)");
    println!("    }}).await?;\n");

    println!("  [6.2] MCP 调用重试（仅重试网络错误）");
    println!("    let response = with_retry_if(&policy, || {{");
    println!("        mcp_client.call_tool(name, args)");
    println!("    }}, |e| e.is_network() || e.is_timeout()).await?;\n");

    println!("  [6.3] 工具执行重试（Agent 内部）");
    println!("    // 配置工具执行策略");
    println!("    AgentConfig {{");
    println!("        tool_execution: ToolExecutionConfig {{");
    println!("            retry_on_fail: true,");
    println!("            max_retries: 2,");
    println!("        }}");
    println!("    }}\n");

    println!("  [6.4] LLM 调用级别重试");
    println!("    // 配置 LLM 调用重试");
    println!("    AgentConfig {{");
    println!("        llm_max_retries: 3,");
    println!("        llm_retry_delay_ms: 500,");
    println!("    }}\n");
}
