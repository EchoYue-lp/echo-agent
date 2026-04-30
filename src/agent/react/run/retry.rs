//! LLM retry logic + concurrent tool timeout calculation

use crate::error::{AgentError, ReactError, Result};
use std::time::Duration;
use tracing::{info, warn};

use super::super::is_retryable_llm_error;

/// Unified LLM retry logic: exponential backoff + jitter + circuit breaker update
///
/// Shared by `think` and `create_llm_stream` to avoid code duplication.
#[tracing::instrument(skip(agent_name, max_retries, retry_delay_ms, circuit_breaker, call_fn), fields(agent = %agent_name))]
pub(crate) async fn retry_llm_call<F, Fut, T>(
    agent_name: &str,
    max_retries: usize,
    retry_delay_ms: u64,
    circuit_breaker: &Option<std::sync::Arc<echo_core::circuit_breaker::CircuitBreaker>>,
    call_fn: F,
) -> Result<T>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let mut result: Result<T> = Err(ReactError::Agent(AgentError::NoResponse {
        model: "unknown".to_string(),
        agent: agent_name.to_string(),
    }));
    for attempt in 0..=max_retries {
        if attempt > 0 {
            // Exponential backoff with jitter: base * 2^(attempt-1) + rand(0..base/2)
            let base_delay = retry_delay_ms * (1u64 << (attempt - 1).min(5));
            let jitter = fastrand::u64(0..=base_delay / 2);
            let delay_ms = base_delay + jitter;
            warn!(
                agent = %agent_name,
                attempt = attempt,
                max = max_retries,
                delay_ms = delay_ms,
                "⚠️ LLM request failed, retrying in {delay_ms}ms ({attempt}/{max_retries})"
            );
            tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
        }
        result = call_fn().await;
        match &result {
            Ok(_) => {
                if attempt > 0 {
                    info!(agent = %agent_name, attempt, "✅ LLM retry succeeded");
                }
                break;
            }
            Err(e) if attempt < max_retries && is_retryable_llm_error(e) => {
                warn!(agent = %agent_name, error = %e, "LLM retryable error");
            }
            Err(_) => break,
        }
    }

    // Update circuit breaker state
    if let Some(cb) = circuit_breaker {
        if result.is_ok() {
            cb.record_success();
        } else {
            cb.record_failure();
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
        u64::from(config.max_retries) + 1
    } else {
        1
    };

    let retry_delay_total_ms = if config.retry_on_fail {
        (1..=config.max_retries)
            .map(|attempt| config.retry_delay_ms * (1u64 << u64::from((attempt - 1).min(5))))
            .sum::<u64>()
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
    use super::compute_concurrent_tool_batch_timeout;
    use crate::tools::ToolExecutionConfig;
    use std::time::Duration;

    #[test]
    fn test_compute_concurrent_tool_batch_timeout_scales_by_waves() {
        let config = ToolExecutionConfig {
            timeout_ms: 1_000,
            retry_on_fail: true,
            max_retries: 2,
            retry_delay_ms: 200,
            max_concurrency: Some(2),
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
        };

        assert_eq!(
            compute_concurrent_tool_batch_timeout(&config, 8, config.max_concurrency),
            None
        );
    }
}
