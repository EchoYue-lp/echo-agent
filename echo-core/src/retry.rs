//! 统一重试策略
//!
//! 提供 [`RetryPolicy`] 配置和 [`with_retry`] 执行包装器，
//! 供 LLM / MCP / A2A / Sandbox 等所有外部调用统一使用。
//!
//! # 示例
//!
//! ```rust,no_run
//! use echo_core::retry::{RetryPolicy, with_retry};
//! use std::time::Duration;
//!
//! # async fn example() -> Result<String, String> {
//! let policy = RetryPolicy::new(3, Duration::from_millis(500))
//!     .max_delay(Duration::from_secs(30))
//!     .jitter(true);
//!
//! let result = with_retry(&policy, || async {
//!     // 可能失败的异步操作
//!     Ok::<_, String>("success".to_string())
//! }).await;
//! # result
//! # }
//! ```

use rand::Rng;
use std::fmt;
use std::future::Future;
use std::time::Duration;
use tracing::{debug, warn};

/// 统一重试策略配置
///
/// 所有外部调用（LLM / MCP / A2A / Sandbox）应通过此策略控制重试行为。
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// 最大重试次数（0 = 不重试，只执行一次）
    pub max_retries: u32,
    /// 初始等待时间（指数退避的基数）
    pub base_delay: Duration,
    /// 最大等待时间上限
    pub max_delay: Duration,
    /// 是否添加随机抖动（防止多客户端同时重试的惊群效应）
    pub jitter: bool,
}

impl RetryPolicy {
    /// Create a retry policy with a retry count and base backoff delay.
    pub fn new(max_retries: u32, base_delay: Duration) -> Self {
        Self {
            max_retries,
            base_delay,
            max_delay: Duration::from_secs(60),
            jitter: false,
        }
    }

    /// Set the maximum delay allowed after exponential backoff.
    pub fn max_delay(mut self, delay: Duration) -> Self {
        self.max_delay = delay;
        self
    }

    /// Enable or disable randomized jitter for retry delays.
    pub fn jitter(mut self, enabled: bool) -> Self {
        self.jitter = enabled;
        self
    }

    /// 不重试（仅执行一次）
    pub fn no_retry() -> Self {
        Self {
            max_retries: 0,
            base_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
            jitter: false,
        }
    }

    /// 计算第 `attempt` 次重试的等待时间（attempt 从 1 开始）
    ///
    /// 注意：attempt=0 表示首次执行（无延迟），返回 Duration::ZERO
    pub fn delay_for(&self, attempt: u32) -> Duration {
        // 首次执行无延迟
        if attempt == 0 {
            return Duration::ZERO;
        }
        // Cap exponent at 10 to avoid overflow: base_delay * 2^10 = 1024x base_delay.
        // With a typical base_delay of 500ms, the max pre-capped delay is ~512s.
        let exp = (attempt - 1).min(10);
        let delay = self.base_delay.saturating_mul(2u32.saturating_pow(exp));
        let capped = delay.min(self.max_delay);

        if self.jitter && !capped.is_zero() {
            let mut rng = rand::thread_rng();
            let jitter_range = capped.as_millis() as u64;
            let jitter_ms = rng.gen_range(0..=jitter_range);
            Duration::from_millis(jitter_ms)
        } else {
            capped
        }
    }
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(30),
            jitter: true,
        }
    }
}

/// 使用给定的重试策略执行异步操作。
///
/// `is_retryable` 用于判断错误是否值得重试，返回 `false` 时立即终止。
///
/// # 示例
///
/// ```rust,no_run
/// use echo_core::retry::{RetryPolicy, with_retry_if};
/// use std::time::Duration;
///
/// # async fn example() {
/// let policy = RetryPolicy::default();
/// let result = with_retry_if(
///     &policy,
///     || async { Err::<(), String>("temporary error".into()) },
///     |e| e.contains("temporary"),
/// ).await;
/// # }
/// ```
pub async fn with_retry_if<F, Fut, T, E, P>(
    policy: &RetryPolicy,
    mut op: F,
    is_retryable: P,
) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    E: fmt::Display,
    P: Fn(&E) -> bool,
{
    let mut last_err: Option<E> = None;

    for attempt in 0..=policy.max_retries {
        if attempt > 0 {
            let delay = policy.delay_for(attempt);
            warn!(
                attempt = attempt,
                max = policy.max_retries,
                delay_ms = delay.as_millis() as u64,
                "重试中..."
            );
            tokio::time::sleep(delay).await;
        }

        match op().await {
            Ok(val) => {
                if attempt > 0 {
                    debug!(attempt, "重试成功");
                }
                return Ok(val);
            }
            Err(e) if attempt < policy.max_retries && is_retryable(&e) => {
                warn!(attempt, error = %e, "可重试错误");
                last_err = Some(e);
            }
            Err(e) => return Err(e),
        }
    }

    Err(last_err.expect("with_retry_if: invariants guarantee last_err is set after retry loop"))
}

/// 使用给定的重试策略执行异步操作（所有错误均视为可重试）。
pub async fn with_retry<F, Fut, T, E>(policy: &RetryPolicy, op: F) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    E: fmt::Display,
{
    with_retry_if(policy, op, |_| true).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn test_retry_policy_defaults() {
        let policy = RetryPolicy::default();
        assert_eq!(policy.max_retries, 3);
        assert!(policy.jitter);
    }

    #[test]
    fn test_delay_exponential_backoff() {
        let policy = RetryPolicy::new(5, Duration::from_millis(100))
            .max_delay(Duration::from_secs(10))
            .jitter(false);

        assert_eq!(policy.delay_for(1), Duration::from_millis(100));
        assert_eq!(policy.delay_for(2), Duration::from_millis(200));
        assert_eq!(policy.delay_for(3), Duration::from_millis(400));
        assert_eq!(policy.delay_for(4), Duration::from_millis(800));
    }

    #[test]
    fn test_delay_capped() {
        let policy = RetryPolicy::new(5, Duration::from_secs(5))
            .max_delay(Duration::from_secs(10))
            .jitter(false);

        assert_eq!(policy.delay_for(1), Duration::from_secs(5));
        assert_eq!(policy.delay_for(2), Duration::from_secs(10));
        assert_eq!(policy.delay_for(3), Duration::from_secs(10));
    }

    #[test]
    fn test_no_retry() {
        let policy = RetryPolicy::no_retry();
        assert_eq!(policy.max_retries, 0);
    }

    #[tokio::test]
    async fn test_with_retry_success_on_first() {
        let policy = RetryPolicy::no_retry();
        let result = with_retry(&policy, || async { Ok::<_, String>(42) }).await;
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn test_with_retry_success_after_failures() {
        let counter = Arc::new(AtomicU32::new(0));
        let policy = RetryPolicy::new(3, Duration::from_millis(1)).jitter(false);

        let c = counter.clone();
        let result = with_retry(&policy, || {
            let c = c.clone();
            async move {
                let n = c.fetch_add(1, Ordering::SeqCst);
                if n < 2 {
                    Err(format!("attempt {} failed", n))
                } else {
                    Ok(n)
                }
            }
        })
        .await;

        assert_eq!(result.unwrap(), 2);
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_with_retry_all_failures() {
        let policy = RetryPolicy::new(2, Duration::from_millis(1)).jitter(false);
        let counter = Arc::new(AtomicU32::new(0));

        let c = counter.clone();
        let result = with_retry(&policy, || {
            let c = c.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Err::<(), _>("always fails".to_string())
            }
        })
        .await;

        assert!(result.is_err());
        assert_eq!(counter.load(Ordering::SeqCst), 3); // 1 + 2 retries
    }

    #[tokio::test]
    async fn test_with_retry_if_non_retryable() {
        let policy = RetryPolicy::new(3, Duration::from_millis(1));
        let counter = Arc::new(AtomicU32::new(0));

        let c = counter.clone();
        let result = with_retry_if(
            &policy,
            || {
                let c = c.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Err::<(), _>("fatal error".to_string())
                }
            },
            |e: &String| !e.contains("fatal"),
        )
        .await;

        assert!(result.is_err());
        assert_eq!(counter.load(Ordering::SeqCst), 1); // no retries for fatal
    }
}
