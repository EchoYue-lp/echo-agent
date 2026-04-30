//! LLM Circuit Breaker
//!
//! Prevents the Agent from entering a futile retry loop when the LLM service is persistently unavailable.
//!
//! ## State Machine
//!
//! ```text
//! Closed ──(consecutive failures ≥ failure_threshold)──→ Open
//!                                                        │
//!                                                 (after timeout)
//!                                                        ↓
//! Closed ←──(successes ≥ success_threshold)── HalfOpen
//! Open   ←──(failure)───────────────────────── HalfOpen
//! ```
//!
//! - **Closed**: Normal operation, recording failure count
//! - **Open**: Reject all requests, transition to HalfOpen after timeout
//! - **HalfOpen**: Allow a limited number of probe requests, decide whether to recover or re-open

use parking_lot::Mutex;
use std::time::{Duration, Instant};
use tracing::{info, warn};

/// Circuit breaker configuration
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// Consecutive failure threshold for entering Open state
    pub failure_threshold: u32,
    /// Consecutive success threshold in HalfOpen state for recovering to Closed
    pub success_threshold: u32,
    /// Open state duration; after timeout, automatically transitions to HalfOpen
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

/// Circuit breaker internal state
#[derive(Debug)]
enum State {
    /// Normal, recording consecutive failure count
    Closed { consecutive_failures: u32 },
    /// Open (tripped), recording when it opened
    Open { opened_at: Instant },
    /// Half-open, recording consecutive success count
    HalfOpen { consecutive_successes: u32 },
}

/// Circuit Breaker
///
/// Thread-safe, can be shared across multiple async tasks (`Arc<CircuitBreaker>`).
pub struct CircuitBreaker {
    state: Mutex<State>,
    config: CircuitBreakerConfig,
    /// Number of requests rejected while in Open state (for monitoring).
    rejected_count: std::sync::atomic::AtomicU32,
    /// Number of probe requests currently in flight during HalfOpen.
    /// Limits concurrent probes to 1 to avoid thundering herd on recovery.
    probes_in_flight: std::sync::atomic::AtomicU32,
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
            probes_in_flight: std::sync::atomic::AtomicU32::new(0),
        }
    }

    /// Create a circuit breaker with default configuration.
    pub fn default_config() -> Self {
        Self::new(CircuitBreakerConfig::default())
    }

    /// Pure query: check whether currently in Open state (does not change any state).
    ///
    /// For monitoring and logging only; does not trigger state transitions.
    pub fn is_open(&self) -> bool {
        let state = self.state.lock();
        matches!(&*state, State::Open { .. })
    }

    /// Attempt to advance state: if Open and timeout has elapsed, transition to HalfOpen automatically.
    ///
    /// Returns `true` if the request should be rejected (still in Open state),
    /// returns `false` if processing may proceed (Closed or HalfOpen).
    ///
    /// In HalfOpen state, at most 1 probe request is allowed concurrently,
    /// avoiding thundering herd during recovery.
    ///
    /// Callers should invoke `record_success()` or `record_failure()` after the request completes,
    /// which will automatically release the probe quota.
    /// If the request is rejected, callers should invoke `record_rejected()` to update statistics.
    pub fn try_advance(&self) -> bool {
        let mut state = self.state.lock();
        match &*state {
            State::Closed { .. } => false,
            State::HalfOpen { .. } => {
                // Only allow one probe at a time during HalfOpen
                let current = self
                    .probes_in_flight
                    .load(std::sync::atomic::Ordering::Acquire);
                if current >= 1 {
                    return true; // reject: probe slot already taken
                }
                self.probes_in_flight
                    .fetch_add(1, std::sync::atomic::Ordering::Release);
                false
            }
            State::Open { opened_at } => {
                if opened_at.elapsed() >= self.config.timeout {
                    info!(
                        timeout_secs = self.config.timeout.as_secs(),
                        "Circuit breaker timeout elapsed, transitioning to HalfOpen"
                    );
                    *state = State::HalfOpen {
                        consecutive_successes: 0,
                    };
                    // First probe into HalfOpen
                    self.probes_in_flight
                        .fetch_add(1, std::sync::atomic::Ordering::Release);
                    false
                } else {
                    true
                }
            }
        }
    }

    /// Record a rejected request (while in Open state).
    pub fn record_rejected(&self) {
        self.rejected_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// Get the total number of rejected requests (for monitoring).
    pub fn rejected_count(&self) -> u32 {
        self.rejected_count
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Record a successful call.
    pub fn record_success(&self) {
        // Release probe slot if any
        self.probes_in_flight
            .fetch_update(
                std::sync::atomic::Ordering::Release,
                std::sync::atomic::Ordering::Acquire,
                |v| if v > 0 { Some(v - 1) } else { Some(0) },
            )
            .ok();

        let mut state = self.state.lock();
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

    /// Record a failed call.
    pub fn record_failure(&self) {
        // Release probe slot if any
        self.probes_in_flight
            .fetch_update(
                std::sync::atomic::Ordering::Release,
                std::sync::atomic::Ordering::Acquire,
                |v| if v > 0 { Some(v - 1) } else { Some(0) },
            )
            .ok();

        let mut state = self.state.lock();
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

    /// Get a human-readable description of the current state (for logging/monitoring).
    pub fn state_name(&self) -> &'static str {
        let state = self.state.lock();
        match &*state {
            State::Closed { .. } => "closed",
            State::Open { .. } => "open",
            State::HalfOpen { .. } => "half_open",
        }
    }

    /// Get the current consecutive failure count (meaningful only in Closed state).
    pub fn consecutive_failures(&self) -> u32 {
        let state = self.state.lock();
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
