//! LLM retry logic + concurrent tool timeout calculation

use crate::error::{AgentError, ReactError, Result};
use echo_core::retry::RetryPolicy;
use std::time::Duration;
use tracing::{info, warn};

use super::super::is_retryable_llm_error;

/// Unified LLM retry logic: exponential backoff + jitter + circuit breaker update
///
/// Shared by `think` and `create_llm_stream` to avoid code duplication.
#[tracing::instrument(skip(agent_name, max_retries, retry_delay_ms, circuit_breaker, cancel, call_fn), fields(agent = %agent_name))]
pub(crate) async fn retry_llm_call<F, Fut, T>(
    agent_name: &str,
    max_retries: usize,
    retry_delay_ms: u64,
    circuit_breaker: &Option<std::sync::Arc<echo_core::circuit_breaker::CircuitBreaker>>,
    cancel: Option<&crate::agent::CancellationToken>,
    call_fn: F,
) -> Result<T>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let permit = match circuit_breaker {
        Some(breaker) => Some(
            breaker
                .acquire()
                .ok_or_else(|| ReactError::Other("LLM circuit breaker is open".to_string()))?,
        ),
        None => None,
    };
    let policy = RetryPolicy::new(
        u32::try_from(max_retries).unwrap_or(u32::MAX),
        Duration::from_millis(retry_delay_ms),
    )
    .max_delay(Duration::from_secs(30))
    // Cancellation tests and callers depend on a real safe point between
    // attempts; provider-level jitter must not collapse that delay to zero.
    .jitter(false);
    let mut result: Result<T> = Err(ReactError::Agent(Box::new(AgentError::NoResponse {
        model: "unknown".to_string(),
        agent: agent_name.to_string(),
    })));
    for attempt in 0..=policy.max_retries {
        if attempt > 0 {
            let delay = policy.delay_for(attempt);
            warn!(
                agent = %agent_name,
                attempt = attempt,
                max = policy.max_retries,
                delay_ms = delay.as_millis() as u64,
                "⚠️ LLM request failed, retrying in {}ms ({}/{})",
                delay.as_millis(),
                attempt,
                policy.max_retries
            );
            let delay = tokio::time::sleep(delay);
            tokio::pin!(delay);
            if let Some(cancel) = cancel {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        return Err(ReactError::Agent(Box::new(AgentError::Cancelled(
                            "LLM retry backoff".to_string(),
                        ))));
                    }
                    _ = &mut delay => {}
                }
            } else {
                delay.await;
            }
        }
        if cancel.is_some_and(crate::agent::CancellationToken::is_cancelled) {
            return Err(ReactError::Agent(Box::new(AgentError::Cancelled(
                "LLM request".to_string(),
            ))));
        }
        result = call_fn().await;
        match &result {
            Ok(_) => {
                if attempt > 0 {
                    info!(agent = %agent_name, attempt, "✅ LLM retry succeeded");
                }
                break;
            }
            Err(e) if attempt < policy.max_retries && is_retryable_llm_error(e) => {
                warn!(agent = %agent_name, error = %e, "LLM retryable error");
            }
            Err(_) => break,
        }
    }

    if let Some(permit) = permit {
        if result.is_ok() {
            permit.success();
        } else {
            permit.failure();
        }
    }

    result
}

pub(crate) fn compute_concurrent_tool_batch_timeout(
    config: &crate::tools::ToolExecutionConfig,
    tool_count: usize,
    max_concurrency: Option<usize>,
) -> Option<Duration> {
    if tool_count == 0 || config.timeout_ms == 0 {
        return None;
    }

    let attempts_per_tool = if config.retry_on_fail {
        u64::from(config.max_retries).saturating_add(1)
    } else {
        1
    };

    let retry_delay_total_ms = if config.retry_on_fail {
        (1..=config.max_retries).fold(0u64, |total, attempt| {
            total.saturating_add(
                config
                    .retry_delay_ms
                    .saturating_mul(1u64 << u64::from((attempt - 1).min(5))),
            )
        })
    } else {
        0
    };

    let per_wave_budget_ms = config
        .timeout_ms
        .saturating_mul(attempts_per_tool)
        .saturating_add(retry_delay_total_ms);

    let waves = match max_concurrency {
        Some(0) | None => 1,
        Some(limit) => tool_count.div_ceil(limit) as u64,
    };

    let grace_ms = 250u64.saturating_mul(waves);
    Some(Duration::from_millis(
        per_wave_budget_ms
            .saturating_mul(waves)
            .saturating_add(grace_ms),
    ))
}

#[cfg(test)]
mod tests {
    use super::{compute_concurrent_tool_batch_timeout, retry_llm_call};
    use crate::agent::CancellationToken;
    use crate::error::{LlmError, ReactError, Result};
    use crate::tools::ToolExecutionConfig;
    use echo_core::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[test]
    fn test_compute_concurrent_tool_batch_timeout_scales_by_waves() {
        let config = ToolExecutionConfig {
            timeout_ms: 1_000,
            retry_on_fail: true,
            max_retries: 2,
            retry_delay_ms: 200,
            max_concurrency: Some(2),
            max_read_concurrency: Some(32),
        };

        let timeout = compute_concurrent_tool_batch_timeout(&config, 5, config.max_concurrency);
        assert_eq!(
            timeout,
            Some(Duration::from_millis((1_000 * 3 + (200 + 400)) * 3 + 750))
        );
    }

    #[test]
    fn test_compute_concurrent_tool_batch_timeout_disabled_when_per_tool_timeout_is_zero() {
        let config = ToolExecutionConfig {
            timeout_ms: 0,
            retry_on_fail: true,
            max_retries: 3,
            retry_delay_ms: 200,
            max_concurrency: Some(4),
            max_read_concurrency: Some(32),
        };

        assert_eq!(
            compute_concurrent_tool_batch_timeout(&config, 8, config.max_concurrency),
            None
        );
    }

    #[tokio::test]
    async fn open_circuit_rejects_before_calling_provider() -> Result<()> {
        let breaker = Arc::new(CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 1,
            success_threshold: 1,
            timeout: Duration::from_secs(60),
        }));
        breaker.record_failure();
        let calls = Arc::new(AtomicUsize::new(0));
        let result = retry_llm_call("test", 0, 0, &Some(breaker.clone()), None, || {
            let calls = calls.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok::<_, ReactError>(())
            }
        })
        .await;
        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(breaker.rejected_count(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn cancellation_during_backoff_starts_no_next_attempt() -> Result<()> {
        let cancel = CancellationToken::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let cancel_task = cancel.clone();
        let cancellation = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            cancel_task.cancel();
        });
        let result = retry_llm_call("test", 3, 250, &None, Some(&cancel), || {
            let calls = calls.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Err::<(), _>(ReactError::Llm(Box::new(LlmError::ApiError {
                    status: 429,
                    message: "retry".to_string(),
                })))
            }
        })
        .await;
        cancellation
            .await
            .map_err(|error| ReactError::Other(format!("cancellation task failed: {error}")))?;
        assert!(matches!(
            result,
            Err(ReactError::Agent(error)) if matches!(*error, crate::error::AgentError::Cancelled(_))
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        Ok(())
    }
}
