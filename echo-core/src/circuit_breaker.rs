//! LLM 熔断器（Circuit Breaker）
//!
//! 防止 LLM 服务持续不可用时 Agent 陷入无效重试循环。
//!
//! ## 状态机
//!
//! ```text
//! Closed ──(连续失败 ≥ failure_threshold)──→ Open
//!                                             │
//!                                     (等待 timeout 后)
//!                                             ↓
//! Closed ←──(成功 ≥ success_threshold)── HalfOpen
//! Open   ←──(失败)──────────────────────── HalfOpen
//! ```
//!
//! - **Closed**：正常工作，记录失败次数
//! - **Open**：拒绝所有请求，等待 timeout 后转入 HalfOpen
//! - **HalfOpen**：允许少量探测请求，根据结果决定恢复或重新打开

use std::sync::Mutex;
use std::time::{Duration, Instant};
use tracing::{error, info, warn};

/// 熔断器配置
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// 连续失败次数阈值，达到后进入 Open 状态
    pub failure_threshold: u32,
    /// HalfOpen 状态下连续成功次数，达到后恢复 Closed
    pub success_threshold: u32,
    /// Open 状态持续时间，超时后自动转入 HalfOpen
    pub timeout: Duration,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            success_threshold: 2,
            timeout: Duration::from_secs(60),
        }
    }
}

/// 熔断器内部状态
#[derive(Debug)]
enum State {
    /// 正常，记录连续失败次数
    Closed { consecutive_failures: u32 },
    /// 熔断，记录打开时刻
    Open { opened_at: Instant },
    /// 半开，记录连续成功次数
    HalfOpen { consecutive_successes: u32 },
}

/// 熔断器
///
/// 线程安全，可在多个 async 任务间共享（`Arc<CircuitBreaker>`）。
pub struct CircuitBreaker {
    state: Mutex<State>,
    config: CircuitBreakerConfig,
    /// Number of requests rejected while in Open state (for monitoring).
    rejected_count: std::sync::atomic::AtomicU32,
}

impl CircuitBreaker {
    /// Create a circuit breaker with an explicit configuration.
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            state: Mutex::new(State::Closed {
                consecutive_failures: 0,
            }),
            config,
            rejected_count: std::sync::atomic::AtomicU32::new(0),
        }
    }

    /// 使用默认配置创建熔断器
    pub fn default_config() -> Self {
        Self::new(CircuitBreakerConfig::default())
    }

    /// 纯查询：检查当前是否处于 Open 状态（不改变任何状态）。
    ///
    /// 仅用于监控和日志，不触发状态转换。
    pub fn is_open(&self) -> bool {
        let state = self.state.lock().unwrap_or_else(|e| {
            error!("circuit breaker mutex poisoned, recovering: {e}");
            e.into_inner()
        });
        matches!(&*state, State::Open { .. })
    }

    /// 尝试推进状态：若 Open 且已超过 timeout，自动转入 HalfOpen。
    ///
    /// 返回 `true` 表示请求应被拒绝（仍处于 Open 状态），
    /// 返回 `false` 表示可以继续处理（Closed 或 HalfOpen）。
    ///
    /// 调用方应在拒绝请求时调用 `record_rejected()` 来更新统计。
    pub fn try_advance(&self) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|e| {
            error!("circuit breaker mutex poisoned, recovering: {e}");
            e.into_inner()
        });
        match &*state {
            State::Closed { .. } | State::HalfOpen { .. } => false,
            State::Open { opened_at } => {
                if opened_at.elapsed() >= self.config.timeout {
                    info!(
                        timeout_secs = self.config.timeout.as_secs(),
                        "Circuit breaker timeout elapsed, transitioning to HalfOpen"
                    );
                    *state = State::HalfOpen {
                        consecutive_successes: 0,
                    };
                    false
                } else {
                    true
                }
            }
        }
    }

    /// 记录一次被拒绝的请求（在 Open 状态下）。
    pub fn record_rejected(&self) {
        self.rejected_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// 获取被拒绝的请求总数（用于监控）。
    pub fn rejected_count(&self) -> u32 {
        self.rejected_count
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// 记录一次成功调用
    pub fn record_success(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| {
            error!("circuit breaker mutex poisoned, recovering: {e}");
            e.into_inner()
        });
        match &*state {
            State::HalfOpen {
                consecutive_successes,
            } => {
                let new_count = consecutive_successes + 1;
                if new_count >= self.config.success_threshold {
                    info!(
                        successes = new_count,
                        threshold = self.config.success_threshold,
                        "✅ Circuit breaker recovered, transitioning to Closed"
                    );
                    *state = State::Closed {
                        consecutive_failures: 0,
                    };
                } else {
                    *state = State::HalfOpen {
                        consecutive_successes: new_count,
                    };
                }
            }
            State::Closed { .. } => {
                *state = State::Closed {
                    consecutive_failures: 0,
                };
            }
            State::Open { .. } => {}
        }
    }

    /// 记录一次失败调用
    pub fn record_failure(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| {
            error!("circuit breaker mutex poisoned, recovering: {e}");
            e.into_inner()
        });
        match &*state {
            State::Closed {
                consecutive_failures,
            } => {
                let new_count = consecutive_failures + 1;
                if new_count >= self.config.failure_threshold {
                    warn!(
                        failures = new_count,
                        threshold = self.config.failure_threshold,
                        "🔴 Circuit breaker opened due to consecutive failures"
                    );
                    *state = State::Open {
                        opened_at: Instant::now(),
                    };
                } else {
                    *state = State::Closed {
                        consecutive_failures: new_count,
                    };
                }
            }
            State::HalfOpen { .. } => {
                warn!("🔴 Circuit breaker re-opened after HalfOpen probe failed");
                *state = State::Open {
                    opened_at: Instant::now(),
                };
            }
            State::Open { .. } => {
                *state = State::Open {
                    opened_at: Instant::now(),
                };
            }
        }
    }

    /// 获取当前状态描述（用于日志/监控）
    pub fn state_name(&self) -> &'static str {
        let state = self.state.lock().unwrap_or_else(|e| {
            error!("circuit breaker mutex poisoned, recovering: {e}");
            e.into_inner()
        });
        match &*state {
            State::Closed { .. } => "closed",
            State::Open { .. } => "open",
            State::HalfOpen { .. } => "half_open",
        }
    }

    /// 获取当前连续失败次数（仅 Closed 状态下有意义）
    pub fn consecutive_failures(&self) -> u32 {
        let state = self.state.lock().unwrap_or_else(|e| {
            error!("circuit breaker mutex poisoned, recovering: {e}");
            e.into_inner()
        });
        match &*state {
            State::Closed {
                consecutive_failures,
            } => *consecutive_failures,
            _ => 0,
        }
    }
}

impl std::fmt::Debug for CircuitBreaker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CircuitBreaker")
            .field("state", &self.state_name())
            .field("config", &self.config)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn fast_config() -> CircuitBreakerConfig {
        CircuitBreakerConfig {
            failure_threshold: 3,
            success_threshold: 2,
            timeout: Duration::from_millis(10),
        }
    }

    #[test]
    fn test_initial_state_closed() {
        let cb = CircuitBreaker::new(fast_config());
        assert!(!cb.is_open());
        assert_eq!(cb.state_name(), "closed");
    }

    #[test]
    fn test_opens_after_threshold_failures() {
        let cb = CircuitBreaker::new(fast_config());
        cb.record_failure();
        cb.record_failure();
        assert!(!cb.is_open());
        cb.record_failure();
        assert!(cb.is_open());
        assert_eq!(cb.state_name(), "open");
    }

    #[test]
    fn test_success_resets_failure_count() {
        let cb = CircuitBreaker::new(fast_config());
        cb.record_failure();
        cb.record_failure();
        cb.record_success();
        cb.record_failure();
        cb.record_failure();
        assert!(!cb.is_open());
    }

    #[test]
    fn test_transitions_to_half_open_after_timeout() {
        let cb = CircuitBreaker::new(fast_config());
        for _ in 0..3 {
            cb.record_failure();
        }
        assert!(cb.is_open());
        std::thread::sleep(Duration::from_millis(20));
        // try_advance transitions from Open to HalfOpen
        assert!(!cb.try_advance());
        assert_eq!(cb.state_name(), "half_open");
    }

    #[test]
    fn test_try_advance_rejects_while_open() {
        let cb = CircuitBreaker::new(fast_config());
        for _ in 0..3 {
            cb.record_failure();
        }
        // Still within timeout, should reject
        assert!(cb.try_advance());
        assert!(cb.is_open());
    }

    #[test]
    fn test_rejected_count_tracking() {
        let cb = CircuitBreaker::new(fast_config());
        for _ in 0..3 {
            cb.record_failure();
        }
        assert_eq!(cb.rejected_count(), 0);
        cb.record_rejected();
        cb.record_rejected();
        cb.record_rejected();
        assert_eq!(cb.rejected_count(), 3);
    }

    #[test]
    fn test_recovers_after_half_open_successes() {
        let cb = CircuitBreaker::new(fast_config());
        for _ in 0..3 {
            cb.record_failure();
        }
        std::thread::sleep(Duration::from_millis(20));
        cb.try_advance(); // transition to HalfOpen
        cb.record_success();
        assert_eq!(cb.state_name(), "half_open");
        cb.record_success();
        assert_eq!(cb.state_name(), "closed");
    }

    #[test]
    fn test_reopens_on_half_open_failure() {
        let cb = CircuitBreaker::new(fast_config());
        for _ in 0..3 {
            cb.record_failure();
        }
        std::thread::sleep(Duration::from_millis(20));
        cb.try_advance(); // transition to HalfOpen
        cb.record_failure();
        assert_eq!(cb.state_name(), "open");
    }
}
