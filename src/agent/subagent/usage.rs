//! LLM usage statistics accumulator for subagent dispatches.
//!
//! Tracks cumulative token usage across all LLM calls within a single
//! subagent dispatch, aligned with [`AgentEvent::LlmUsage`] fields.

use serde::{Deserialize, Serialize};

/// Cumulative LLM usage snapshot across one or more LLM calls within a
/// single subagent dispatch.
///
/// Fields align with `AgentEvent::LlmUsage`. Token counts use `u64` to
/// safely accumulate across many calls without overflow.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct LlmUsageStats {
    /// Model name (last seen — within a single dispatch it's typically the same).
    pub model: String,
    /// Cumulative prompt/input tokens.
    pub prompt_tokens: u64,
    /// Cumulative completion/output tokens.
    pub completion_tokens: u64,
    /// Cumulative total tokens.
    pub total_tokens: u64,
    /// Cumulative cached prompt tokens.
    pub cached_prompt_tokens: u64,
    /// Cumulative cache-creation prompt tokens.
    pub cache_creation_prompt_tokens: u64,
    /// Whether any call reported real usage metadata.
    /// Once set to `true` it stays `true` (sticky).
    pub usage_reported: bool,
    /// Number of LLM calls recorded.
    pub call_count: u64,
}

impl LlmUsageStats {
    /// Record a single LLM usage event, accumulating into the running totals.
    ///
    /// `model` is overwritten each call (last-seen wins), which is fine
    /// because a single dispatch typically uses one model.
    pub fn record(
        &mut self,
        model: &str,
        prompt_tokens: usize,
        completion_tokens: usize,
        total_tokens: usize,
        cached_prompt_tokens: usize,
        cache_creation_prompt_tokens: usize,
        usage_reported: bool,
    ) {
        self.model = model.to_string();
        self.prompt_tokens += prompt_tokens as u64;
        self.completion_tokens += completion_tokens as u64;
        self.total_tokens += total_tokens as u64;
        self.cached_prompt_tokens += cached_prompt_tokens as u64;
        self.cache_creation_prompt_tokens += cache_creation_prompt_tokens as u64;
        if usage_reported {
            self.usage_reported = true;
        }
        self.call_count += 1;
    }

    /// Convert to a JSON payload compatible with
    /// `cacheUsageFromEvents` on the frontend.
    pub fn to_payload(&self, session_id: &str) -> serde_json::Value {
        serde_json::json!({
            "session_id": session_id,
            "model": if self.model.is_empty() { "unknown" } else { &self.model },
            "prompt_tokens": self.prompt_tokens,
            "completion_tokens": self.completion_tokens,
            "total_tokens": self.total_tokens,
            "cached_prompt_tokens": self.cached_prompt_tokens,
            "cache_creation_prompt_tokens": self.cache_creation_prompt_tokens,
            "usage_reported": self.usage_reported,
            "call_count": self.call_count,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accumulates_multiple_calls() {
        let mut stats = LlmUsageStats::default();
        stats.record("claude-x", 100, 50, 150, 80, 10, true);
        stats.record("claude-x", 200, 60, 260, 150, 20, true);
        assert_eq!(stats.prompt_tokens, 300);
        assert_eq!(stats.completion_tokens, 110);
        assert_eq!(stats.total_tokens, 410);
        assert_eq!(stats.cached_prompt_tokens, 230);
        assert_eq!(stats.cache_creation_prompt_tokens, 30);
        assert_eq!(stats.call_count, 2);
        assert!(stats.usage_reported);
    }

    #[test]
    fn usage_reported_stays_false_until_any_true() {
        let mut stats = LlmUsageStats::default();
        stats.record("m", 10, 5, 15, 0, 0, false);
        assert!(!stats.usage_reported);
        stats.record("m", 10, 5, 15, 0, 0, true);
        assert!(stats.usage_reported);
    }

    #[test]
    fn payload_uses_unknown_when_model_empty() {
        let stats = LlmUsageStats::default();
        let p = stats.to_payload("sess-1");
        assert_eq!(p["model"], serde_json::json!("unknown"));
        assert_eq!(p["usage_reported"], serde_json::json!(false));
    }

    #[test]
    fn payload_uses_real_model_when_set() {
        let mut stats = LlmUsageStats::default();
        stats.record("claude-opus-4", 500, 200, 700, 300, 50, true);
        let p = stats.to_payload("sess-2");
        assert_eq!(p["model"], serde_json::json!("claude-opus-4"));
        assert_eq!(p["prompt_tokens"], serde_json::json!(500));
        assert_eq!(p["call_count"], serde_json::json!(1));
    }
}
