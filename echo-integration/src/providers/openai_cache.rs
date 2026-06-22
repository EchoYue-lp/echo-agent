//! OpenAI-compatible prefix cache observability helper.
//!
//! OpenAI/DeepSeek/Qwen and other compatible providers use **automatic prefix
//! caching** — there are no explicit `cache_control` breakpoints. Cache hit
//! rate depends entirely on:
//!
//! 1. **Stable prefix**: system + tools + history must be byte-identical across
//!    consecutive requests.
//! 2. **Stable `user_id`** (DeepSeek): the `user` field serves as a KV-cache
//!    partition key; an unstable or absent user_id means cache is never reused.
//!
//! This module provides:
//! - A diagnostic helper to log prefix stability before each request.
//! - Assertions that the runtime context segment is at the tail (not mixed into
//!   the stable prefix).
//! - No protocol-level changes — these providers already have automatic prefix
//!   caching; we just verify the conditions are met.

use echo_core::llm::cache::layout::PromptCacheLayout;
use echo_core::llm::cache::diagnostic::stable_prefix_hash;

/// Pre-request cache diagnostic for OpenAI-compatible providers.
///
/// Call this before sending a chat request to verify that the prompt is
/// structured correctly for automatic prefix caching.
pub struct OpenAICacheDiagnostic {
    /// Whether the `user_id` field is set (required for DeepSeek KV-cache).
    pub user_id_set: bool,
    /// SHA-256 of the stable prefix (system + canonical + tools + history).
    pub stable_prefix_hash: String,
    /// Number of messages in the stable prefix.
    pub stable_prefix_msg_count: usize,
    /// Number of runtime-context messages (at the tail — should not break prefix).
    pub runtime_context_msg_count: usize,
    /// Whether runtime context messages are correctly isolated at the tail.
    pub runtime_context_at_tail: bool,
    /// The segment layout for detailed inspection.
    pub segments: echo_core::llm::cache::SegmentRanges,
}

impl OpenAICacheDiagnostic {
    /// Build a diagnostic from the layout and user_id.
    pub fn from_layout(layout: &PromptCacheLayout<'_>, user_id: Option<&str>) -> Self {
        let hash = stable_prefix_hash(
            layout.system,
            layout.canonical,
            layout.tools,
            layout.history,
        );
        let stable_count = layout.system.len()
            + layout.canonical.len()
            + layout.history.len();
        let runtime_count = layout.runtime_context.len();

        // Verify runtime context is at the tail: stable prefix messages are
        // contiguous from position 0, followed by runtime context.
        let segments = layout.segment_ranges();
        let runtime_at_tail = segments.runtime_context.start >= segments.history.end;

        Self {
            user_id_set: user_id.is_some(),
            stable_prefix_hash: hash,
            stable_prefix_msg_count: stable_count,
            runtime_context_msg_count: runtime_count,
            runtime_context_at_tail: runtime_at_tail,
            segments,
        }
    }

    /// Log the diagnostic at `info` level for observability.
    pub fn log(&self, agent: &str) {
        tracing::info!(
            target: "echo_agent::cache::openai",
            agent = %agent,
            user_id_set = self.user_id_set,
            stable_prefix_hash = %self.stable_prefix_hash,
            stable_prefix_msg_count = self.stable_prefix_msg_count,
            runtime_context_msg_count = self.runtime_context_msg_count,
            runtime_context_at_tail = self.runtime_context_at_tail,
            "🔍 OpenAI-compatible cache diagnostic"
        );
    }

    /// Assert that the prompt is correctly structured for prefix caching.
    /// Returns `Err` with a description of what's wrong, or `Ok(())`.
    pub fn validate(&self) -> Result<(), String> {
        if !self.runtime_context_at_tail {
            return Err(
                "runtime context is NOT at the tail — will break prefix cache".to_string()
            );
        }
        // Warn but don't fail for DeepSeek-like providers that need user_id:
        // the provider layer already handles filling it in; we just report.
        Ok(())
    }
}

impl std::fmt::Display for OpenAICacheDiagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "OpenAICacheDiagnostic(uid_set={}, hash={}, stable_msgs={}, rt_msgs={}, rt_at_tail={})",
            self.user_id_set,
            self.stable_prefix_hash,
            self.stable_prefix_msg_count,
            self.runtime_context_msg_count,
            self.runtime_context_at_tail,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::llm::types::Message;

    fn sys(t: &str) -> Message {
        Message::system(t.to_string())
    }
    fn user(t: &str) -> Message {
        Message::user(t.to_string())
    }
    fn rt(t: &str) -> Message {
        Message::user(format!("[runtime_context:{t}]"))
    }

    #[test]
    fn diagnostic_runtime_at_tail_when_correctly_placed() {
        let msgs = vec![sys("S"), user("hi"), rt("turn\ncwd: /tmp")];
        let layout = PromptCacheLayout::from_messages(&msgs, &[]);
        let diag = OpenAICacheDiagnostic::from_layout(&layout, Some("uid"));
        assert!(diag.runtime_context_at_tail);
        assert!(diag.user_id_set);
    }

    #[test]
    fn diagnostic_user_id_not_set_is_visible() {
        let msgs = vec![sys("S"), user("hi")];
        let layout = PromptCacheLayout::from_messages(&msgs, &[]);
        let diag = OpenAICacheDiagnostic::from_layout(&layout, None);
        assert!(!diag.user_id_set);
        assert_eq!(diag.runtime_context_msg_count, 0);
    }

    #[test]
    fn diagnostic_calculates_hash() {
        let msgs = vec![sys("S"), user("h1")];
        let layout = PromptCacheLayout::from_messages(&msgs, &[]);
        let d1 = OpenAICacheDiagnostic::from_layout(&layout, None);
        let d2 = OpenAICacheDiagnostic::from_layout(&layout, None);
        // Same input → same hash
        assert_eq!(d1.stable_prefix_hash, d2.stable_prefix_hash);
    }
}
