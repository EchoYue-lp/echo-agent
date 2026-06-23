//! Token estimation trait and usage tracking.
//!
//! Provides a pluggable token counting capability for [`ContextManager`], replacing the
//! fixed `chars / 4` heuristic.
//!
//! # Built-in Implementations
//!
//! | Type | Algorithm | Accuracy |
//! |------|----------|----------|
//! | [`HeuristicTokenizer`] | ASCII weight 1, CJK weight 2, total / 4 | Medium (recommended for mixed Chinese/English) |
//! | [`SimpleTokenizer`] | `byte_count / 4 + 1` | Low (backward compatible) |
//!
//! # Usage Tracking
//!
//! [`TokenUsageTracker`] provides cross-request token accumulation statistics,
//! comparable to the token usage display capabilities of Claude Code / ChatGPT.
//!
//! # Extension
//!
//! Implement the [`Tokenizer`] trait to integrate an exact tokenizer (e.g. tiktoken-rs).

/// Token counter abstraction
pub trait Tokenizer: Send + Sync {
    /// Estimate how many model tokens the input text will consume.
    fn count_tokens(&self, text: &str) -> usize;
}

impl Tokenizer for Box<dyn Tokenizer> {
    fn count_tokens(&self, text: &str) -> usize {
        (**self).count_tokens(text)
    }
}

/// Heuristic Tokenizer that estimates token count using character weights.
///
/// **Note: This is a rough estimator, not an exact token counter.**
///
/// Estimation rules:
/// - ASCII characters weight 1 (~4 chars = 1 token)
/// - CJK and other non-ASCII characters weight 2 (~1-2 chars = 1 token)
/// - Total weight / 4 yields the estimated token count
/// - Empty string returns 0
///
/// Compared to `byte_count / 4`, accuracy for CJK content improves by ~40-60%,
/// but it should still not be used for scenarios requiring exact token counting
/// (e.g., quota management, billing, etc.).
/// For exact counting, use tiktoken or a model-native tokenizer.
pub struct HeuristicTokenizer;

impl Tokenizer for HeuristicTokenizer {
    fn count_tokens(&self, text: &str) -> usize {
        if text.is_empty() {
            return 0;
        }
        let weight: usize = text.chars().map(|c| if c.is_ascii() { 1 } else { 2 }).sum();
        (weight / 4).max(1)
    }
}

/// Simple Tokenizer: `byte_count / 4 + 1` (backward compatible with old behavior)
pub struct SimpleTokenizer;

impl Tokenizer for SimpleTokenizer {
    fn count_tokens(&self, text: &str) -> usize {
        if text.is_empty() {
            0
        } else {
            text.len() / 4 + 1
        }
    }
}

/// Self-calibrating tokenizer wrapper that improves estimation accuracy using
/// actual API response data.
///
/// Wraps any [`Tokenizer`] implementation and adjusts its estimates by a
/// calibration factor derived from the ratio of actual token counts (reported
/// by the LLM API) to estimated counts.
///
/// # How it works
///
/// 1. `count_tokens()` returns `inner.estimate × calibration_factor`
/// 2. After each LLM call, feed back the actual token count via `calibrate()`
/// 3. The factor is an exponential moving average (EMA) that converges over time
///
/// # Example
///
/// ```rust
/// use echo_core::tokenizer::{CalibratedTokenizer, HeuristicTokenizer, Tokenizer};
/// use std::sync::Arc;
///
/// let base = Arc::new(HeuristicTokenizer);
/// let calibrated = CalibratedTokenizer::new(base);
///
/// // Initial estimate (factor = 1.0)
/// let est = calibrated.count_tokens("hello world");
///
/// // After API returns actual count:
/// calibrated.calibrate(est, 15); // actual was 15 tokens
///
/// // Subsequent estimates are now adjusted
/// let adjusted = calibrated.count_tokens("another text");
/// ```
pub struct CalibratedTokenizer {
    inner: std::sync::Arc<dyn Tokenizer>,
    /// Calibration factor stored as f64 bits in AtomicU64 for lock-free access.
    /// Factor = actual_tokens / estimated_tokens (EMA smoothed).
    factor_bits: std::sync::atomic::AtomicU64,
    /// Number of calibration samples received.
    sample_count: std::sync::atomic::AtomicU64,
    /// EMA smoothing factor (0.0 = no update, 1.0 = replace entirely).
    /// Default 0.3 provides good convergence.
    ema_alpha: f64,
}

impl CalibratedTokenizer {
    /// Create a new calibrated tokenizer wrapping the given base tokenizer.
    ///
    /// Initial calibration factor is 1.0 (no adjustment).
    pub fn new(inner: std::sync::Arc<dyn Tokenizer>) -> Self {
        Self {
            inner,
            factor_bits: std::sync::atomic::AtomicU64::new(1.0_f64.to_bits()),
            sample_count: std::sync::atomic::AtomicU64::new(0),
            ema_alpha: 0.3,
        }
    }

    /// Create with a custom EMA alpha (smoothing factor).
    ///
    /// - `alpha = 0.1`: slow convergence, more stable
    /// - `alpha = 0.3`: balanced (default)
    /// - `alpha = 0.5`: fast convergence, more reactive
    pub fn with_alpha(inner: std::sync::Arc<dyn Tokenizer>, alpha: f64) -> Self {
        Self {
            inner,
            factor_bits: std::sync::atomic::AtomicU64::new(1.0_f64.to_bits()),
            sample_count: std::sync::atomic::AtomicU64::new(0),
            ema_alpha: alpha.clamp(0.01, 1.0),
        }
    }

    /// Feed back actual token count from an API response to improve future estimates.
    ///
    /// - `estimated`: the value returned by `count_tokens()` before the API call
    /// - `actual`: the actual token count from the API response (`usage.prompt_tokens`)
    pub fn calibrate(&self, estimated: usize, actual: u32) {
        if estimated == 0 || actual == 0 {
            return;
        }

        let observed_ratio = actual as f64 / estimated as f64;
        // Clamp ratio to reasonable range to avoid wild swings
        let observed_ratio = observed_ratio.clamp(0.2, 5.0);

        let current_bits = self.factor_bits.load(std::sync::atomic::Ordering::Relaxed);
        let current_factor = f64::from_bits(current_bits);

        // EMA: new_factor = alpha * observed + (1 - alpha) * current
        let new_factor = self.ema_alpha * observed_ratio + (1.0 - self.ema_alpha) * current_factor;

        self.factor_bits
            .store(new_factor.to_bits(), std::sync::atomic::Ordering::Relaxed);
        self.sample_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// Get the current calibration factor.
    pub fn calibration_factor(&self) -> f64 {
        let bits = self.factor_bits.load(std::sync::atomic::Ordering::Relaxed);
        f64::from_bits(bits)
    }

    /// Get the number of calibration samples received.
    pub fn sample_count(&self) -> u64 {
        self.sample_count.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Reset the calibration factor to 1.0.
    pub fn reset_calibration(&self) {
        self.factor_bits
            .store(1.0_f64.to_bits(), std::sync::atomic::Ordering::Relaxed);
        self.sample_count
            .store(0, std::sync::atomic::Ordering::Relaxed);
    }
}

impl Tokenizer for CalibratedTokenizer {
    fn count_tokens(&self, text: &str) -> usize {
        let base = self.inner.count_tokens(text);
        let factor = self.calibration_factor();
        (base as f64 * factor).round() as usize
    }
}

impl std::fmt::Debug for CalibratedTokenizer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CalibratedTokenizer")
            .field("factor", &self.calibration_factor())
            .field("samples", &self.sample_count())
            .finish()
    }
}

// ── Token Usage Tracking ─────────────────────────────────────────────────────────

use std::sync::atomic::{AtomicU64, Ordering};

/// Thread-safe token usage tracker.
///
/// Comparable to the token usage display of Claude Code / ChatGPT.
///
/// ```rust
/// use echo_core::tokenizer::TokenUsageTracker;
///
/// let tracker = TokenUsageTracker::new("gpt-5.5");
/// tracker.record(1500, 800, Some(2300));
///
/// let stats = tracker.summary();
/// assert_eq!(stats.total_prompt_tokens, 1500);
/// ```
pub struct TokenUsageTracker {
    model_name: String,
    total_prompt_tokens: AtomicU64,
    total_completion_tokens: AtomicU64,
    total_tokens: AtomicU64,
    total_cached_prompt_tokens: AtomicU64,
    total_cache_creation_prompt_tokens: AtomicU64,
    request_count: AtomicU64,
}

impl TokenUsageTracker {
    pub fn new(model_name: impl Into<String>) -> Self {
        Self {
            model_name: model_name.into(),
            total_prompt_tokens: AtomicU64::new(0),
            total_completion_tokens: AtomicU64::new(0),
            total_tokens: AtomicU64::new(0),
            total_cached_prompt_tokens: AtomicU64::new(0),
            total_cache_creation_prompt_tokens: AtomicU64::new(0),
            request_count: AtomicU64::new(0),
        }
    }

    /// Number of requests recorded so far.
    pub fn request_count(&self) -> u64 {
        self.request_count.load(Ordering::Relaxed)
    }

    /// Record token usage for a single request.
    pub fn record(&self, prompt: u32, completion: u32, total: Option<u32>) {
        self.total_prompt_tokens
            .fetch_add(prompt as u64, Ordering::Relaxed);
        self.total_completion_tokens
            .fetch_add(completion as u64, Ordering::Relaxed);
        let t = total.unwrap_or(prompt + completion);
        self.total_tokens.fetch_add(t as u64, Ordering::Relaxed);
        self.request_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Record usage from an API response.
    pub fn record_usage(&self, usage: &crate::llm::types::Usage) {
        let prompt = usage.prompt_tokens.unwrap_or(0);
        let completion = usage.completion_tokens.unwrap_or(0);
        self.record(prompt, completion, usage.total_tokens);
        self.total_cached_prompt_tokens
            .fetch_add(usage.cached_prompt_tokens() as u64, Ordering::Relaxed);
        self.total_cache_creation_prompt_tokens.fetch_add(
            usage.cache_creation_prompt_tokens() as u64,
            Ordering::Relaxed,
        );
    }

    /// Get usage summary.
    pub fn summary(&self) -> UsageSummary {
        let total_prompt = self.total_prompt_tokens.load(Ordering::Relaxed);
        let total_completion = self.total_completion_tokens.load(Ordering::Relaxed);
        let total = self.total_tokens.load(Ordering::Relaxed);
        let total_cached_prompt = self.total_cached_prompt_tokens.load(Ordering::Relaxed);
        let total_cache_creation_prompt = self
            .total_cache_creation_prompt_tokens
            .load(Ordering::Relaxed);
        let requests = self.request_count.load(Ordering::Relaxed);

        UsageSummary {
            model_name: self.model_name.clone(),
            total_prompt_tokens: total_prompt,
            total_completion_tokens: total_completion,
            total_tokens: total,
            total_cached_prompt_tokens: total_cached_prompt,
            total_cache_creation_prompt_tokens: total_cache_creation_prompt,
            request_count: requests,
        }
    }

    /// Cumulative prompt cache hit rate (0.0–1.0).
    ///
    /// Computed as `cached_prompt / (prompt + cached_prompt)` across all requests.
    /// Returns `None` if no requests with prompt tokens have been recorded.
    pub fn cumulative_cache_hit_rate(&self) -> Option<f64> {
        let prompt = self.total_prompt_tokens.load(Ordering::Relaxed);
        let cached = self.total_cached_prompt_tokens.load(Ordering::Relaxed);
        let total_prompt = prompt.saturating_add(cached);
        if total_prompt == 0 {
            None
        } else {
            Some(cached as f64 / total_prompt as f64)
        }
    }

    /// Log cumulative cache stats at `info` level (for periodic observability).
    pub fn log_cumulative_cache_stats(&self, agent: &str) {
        let requests = self.request_count.load(Ordering::Relaxed);
        if let Some(rate) = self.cumulative_cache_hit_rate() {
            let cached = self.total_cached_prompt_tokens.load(Ordering::Relaxed);
            let creation = self
                .total_cache_creation_prompt_tokens
                .load(Ordering::Relaxed);
            tracing::info!(
                target: "echo_agent::cache",
                agent = %agent,
                requests,
                cumulative_cache_hit_rate = format!("{:.1}%", rate * 100.0),
                total_cached_prompt_tokens = cached,
                total_cache_creation_prompt_tokens = creation,
                "📊 cumulative cache performance"
            );
        }
    }

    /// Reset all counters.
    pub fn reset(&self) {
        self.total_prompt_tokens.store(0, Ordering::Relaxed);
        self.total_completion_tokens.store(0, Ordering::Relaxed);
        self.total_tokens.store(0, Ordering::Relaxed);
        self.total_cached_prompt_tokens.store(0, Ordering::Relaxed);
        self.total_cache_creation_prompt_tokens
            .store(0, Ordering::Relaxed);
        self.request_count.store(0, Ordering::Relaxed);
    }
}

/// Token usage summary snapshot
#[derive(Debug, Clone)]
pub struct UsageSummary {
    pub model_name: String,
    pub total_prompt_tokens: u64,
    pub total_completion_tokens: u64,
    pub total_tokens: u64,
    pub total_cached_prompt_tokens: u64,
    pub total_cache_creation_prompt_tokens: u64,
    pub request_count: u64,
}

impl std::fmt::Display for UsageSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Token Usage [{model}]:
  Requests:   {requests}
  Input tokens:  {prompt}
  Cached input:  {cached_prompt}
  Cache writes:  {cache_creation_prompt}
  Output tokens: {completion}
  Total tokens:  {total}",
            model = self.model_name,
            requests = self.request_count,
            prompt = self.total_prompt_tokens,
            cached_prompt = self.total_cached_prompt_tokens,
            cache_creation_prompt = self.total_cache_creation_prompt_tokens,
            completion = self.total_completion_tokens,
            total = self.total_tokens,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn test_heuristic_ascii() {
        let t = HeuristicTokenizer;
        // 16 ASCII chars → weight 16 → 16/4 = 4 tokens
        assert_eq!(t.count_tokens("hello world 1234"), 4);
    }

    #[test]
    fn test_heuristic_cjk() {
        let t = HeuristicTokenizer;
        // 4 non-ASCII chars → weight 8 → 8/4 = 2 tokens
        assert_eq!(t.count_tokens("éñôà"), 2);
    }

    #[test]
    fn test_heuristic_mixed() {
        let t = HeuristicTokenizer;
        // "hello éñ" → 6 ASCII(6) + 2 non-ASCII(4) = weight 10 → 10/4 = 2
        assert_eq!(t.count_tokens("hello éñ"), 2);
    }

    #[test]
    fn test_heuristic_empty() {
        let t = HeuristicTokenizer;
        assert_eq!(t.count_tokens(""), 0); // empty string returns 0
    }

    #[test]
    fn test_simple_tokenizer() {
        let t = SimpleTokenizer;
        assert_eq!(t.count_tokens("hello"), 2); // 5/4+1 = 2
    }

    #[test]
    fn test_calibrated_tokenizer_initial_factor() {
        let base = Arc::new(HeuristicTokenizer);
        let calibrated = CalibratedTokenizer::new(base);

        // Initial factor should be 1.0
        assert!((calibrated.calibration_factor() - 1.0).abs() < f64::EPSILON);
        assert_eq!(calibrated.sample_count(), 0);
    }

    #[test]
    fn test_calibrated_tokenizer_calibration() {
        let base = Arc::new(HeuristicTokenizer);
        let calibrated = CalibratedTokenizer::with_alpha(base.clone(), 1.0); // alpha=1.0 for instant update

        let text = "hello world test";
        let estimated = base.count_tokens(text); // HeuristicTokenizer estimate
        let actual = estimated * 2; // Simulate actual being 2x the estimate

        calibrated.calibrate(estimated, actual as u32);

        // With alpha=1.0, factor should jump directly to 2.0
        assert!((calibrated.calibration_factor() - 2.0).abs() < f64::EPSILON);
        assert_eq!(calibrated.sample_count(), 1);

        // Now count_tokens should return ~2x the base estimate
        let adjusted = calibrated.count_tokens(text);
        assert_eq!(adjusted, estimated * 2);
    }

    #[test]
    fn test_calibrated_tokenizer_ema_smoothing() {
        let base = Arc::new(HeuristicTokenizer);
        let calibrated = CalibratedTokenizer::with_alpha(base.clone(), 0.5);

        let estimated = 100;

        // First calibration: actual = 200 (ratio = 2.0)
        calibrated.calibrate(estimated, 200);
        let f1 = calibrated.calibration_factor();
        // With alpha=0.5: 0.5 * 2.0 + 0.5 * 1.0 = 1.5
        assert!((f1 - 1.5).abs() < 0.01);

        // Second calibration: actual = 200 again (ratio = 2.0)
        calibrated.calibrate(estimated, 200);
        let f2 = calibrated.calibration_factor();
        // With alpha=0.5: 0.5 * 2.0 + 0.5 * 1.5 = 1.75
        assert!((f2 - 1.75).abs() < 0.01);
    }

    #[test]
    fn test_calibrated_tokenizer_reset() {
        let base = Arc::new(HeuristicTokenizer);
        let calibrated = CalibratedTokenizer::new(base);

        calibrated.calibrate(100, 200);
        assert!(calibrated.calibration_factor() > 1.0);

        calibrated.reset_calibration();
        assert!((calibrated.calibration_factor() - 1.0).abs() < f64::EPSILON);
        assert_eq!(calibrated.sample_count(), 0);
    }

    #[test]
    fn test_calibrated_tokenizer_skips_zero() {
        let base = Arc::new(HeuristicTokenizer);
        let calibrated = CalibratedTokenizer::new(base);

        // Should not update on zero values
        calibrated.calibrate(0, 100);
        calibrated.calibrate(100, 0);
        assert!((calibrated.calibration_factor() - 1.0).abs() < f64::EPSILON);
        assert_eq!(calibrated.sample_count(), 0);
    }

    #[test]
    fn test_token_usage_tracker_records_prompt_cache_tokens() {
        let tracker = TokenUsageTracker::new("test-model");
        tracker.record_usage(&crate::llm::types::Usage {
            prompt_tokens: Some(1000),
            completion_tokens: Some(20),
            total_tokens: Some(1020),
            prompt_tokens_details: Some(crate::llm::types::TokenUsageDetails {
                cached_tokens: Some(980),
            }),
            ..Default::default()
        });

        let summary = tracker.summary();
        assert_eq!(summary.total_prompt_tokens, 1000);
        assert_eq!(summary.total_cached_prompt_tokens, 980);
        assert_eq!(summary.total_cache_creation_prompt_tokens, 0);
    }
}
