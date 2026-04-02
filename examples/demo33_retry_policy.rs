//! demo33_retry_policy —— 统一错误恢复 + Circuit Breaker
//!
//! 演示 `echo-core` 中的 `RetryPolicy` 和 `with_retry` / `with_retry_if` API，
//! 所有外部调用（LLM / MCP / A2A / Sandbox）统一使用。
//!
//! ```bash
//! cargo run --example demo33_retry_policy
//! ```

use echo_agent::prelude::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

#[tokio::main]
async fn main() {
    println!("═══ Unified Retry Policy Demo ═══\n");

    // ── 1. RetryPolicy 构建 ───────────────────────────────────────────────
    println!("[1] RetryPolicy 构建方式");

    let default_policy = RetryPolicy::default();
    println!(
        "    Default: max_retries={}, base_delay={:?}, jitter={}",
        default_policy.max_retries, default_policy.base_delay, default_policy.jitter
    );

    let custom = RetryPolicy::new(5, Duration::from_millis(200))
        .max_delay(Duration::from_secs(10))
        .jitter(true);
    println!(
        "    Custom:  max_retries={}, base_delay={:?}, max_delay={:?}",
        custom.max_retries, custom.base_delay, custom.max_delay
    );

    let no_retry = RetryPolicy::no_retry();
    println!("    NoRetry: max_retries={}", no_retry.max_retries);

    // ── 2. 指数退避延迟计算 ───────────────────────────────────────────────
    println!("\n[2] 指数退避延迟计算（jitter=false）");
    let policy = RetryPolicy::new(5, Duration::from_millis(100))
        .max_delay(Duration::from_secs(2))
        .jitter(false);

    for attempt in 1..=5 {
        println!(
            "    attempt {attempt}: delay = {:?}",
            policy.delay_for(attempt)
        );
    }
    println!("    → 100ms → 200ms → 400ms → 800ms → 1600ms（指数增长，不超过 max_delay）");

    // ── 3. with_retry 成功场景 ────────────────────────────────────────────
    println!("\n[3] with_retry — 首次成功");
    let result = with_retry(&RetryPolicy::no_retry(), || async { Ok::<_, String>(42) }).await;
    println!("    result = {}", result.unwrap());

    // ── 4. with_retry 失败后重试成功 ──────────────────────────────────────
    println!("\n[4] with_retry — 前两次失败，第三次成功");
    let counter = Arc::new(AtomicU32::new(0));
    let c = counter.clone();
    let policy = RetryPolicy::new(3, Duration::from_millis(10)).jitter(false);

    let start = Instant::now();
    let result = with_retry(&policy, || {
        let c = c.clone();
        async move {
            let n = c.fetch_add(1, Ordering::SeqCst);
            if n < 2 {
                Err(format!("attempt {} failed", n))
            } else {
                Ok(format!("success on attempt {}", n + 1))
            }
        }
    })
    .await;
    println!(
        "    {} (total calls: {}, elapsed: {:?})",
        result.unwrap(),
        counter.load(Ordering::SeqCst),
        start.elapsed()
    );

    // ── 5. with_retry 全部失败 ────────────────────────────────────────────
    println!("\n[5] with_retry — 全部失败（耗尽重试）");
    let counter = Arc::new(AtomicU32::new(0));
    let c = counter.clone();
    let policy = RetryPolicy::new(2, Duration::from_millis(5)).jitter(false);

    let result = with_retry(&policy, || {
        let c = c.clone();
        async move {
            c.fetch_add(1, Ordering::SeqCst);
            Err::<(), _>("service unavailable".to_string())
        }
    })
    .await;

    assert!(result.is_err());
    println!("    error: {}", result.unwrap_err());
    println!(
        "    total calls: {} (1 initial + 2 retries)",
        counter.load(Ordering::SeqCst)
    );

    // ── 6. with_retry_if 选择性重试 ──────────────────────────────────────
    println!("\n[6] with_retry_if — 仅对可恢复错误重试");

    let counter = Arc::new(AtomicU32::new(0));
    let c = counter.clone();
    let result = with_retry_if(
        &RetryPolicy::new(5, Duration::from_millis(5)),
        || {
            let c = c.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Err::<(), _>("FATAL: authentication failed".to_string())
            }
        },
        |e: &String| !e.starts_with("FATAL"),
    )
    .await;

    assert!(result.is_err());
    println!("    error: {}", result.unwrap_err());
    println!(
        "    total calls: {} (fatal error → 不重试，立即返回)",
        counter.load(Ordering::SeqCst)
    );

    // ── 7. 典型使用模式 ──────────────────────────────────────────────────
    println!("\n[7] 推荐使用模式");
    println!("    // LLM 调用");
    println!("    let policy = RetryPolicy::new(3, Duration::from_millis(500)).jitter(true);");
    println!("    let response = with_retry(&policy, || llm_client.chat(request)).await?;");
    println!();
    println!("    // MCP 调用（仅重试网络错误）");
    println!(
        "    let response = with_retry_if(&policy, || mcp.call(tool), |e| e.is_network()).await?;"
    );

    println!("\n═══ Demo Complete ═══");
}
