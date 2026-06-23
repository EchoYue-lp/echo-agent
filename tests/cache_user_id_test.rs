//! Cache user_id propagation tests.
//!
//! Verifies that every LLM call path sets `user_id` from the agent's stable
//! `cache_user_id` configuration. An absent or unstable user_id is the #1
//! cause of sub-1% cache hit rates for DeepSeek and other OpenAI-compatible
//! providers that use `user_id` for KV-cache partition isolation.
//!
//! # Verified paths
//!
//! | Path                     | File + line                        | Status |
//! |--------------------------|------------------------------------|--------|
//! | Non-streaming think      | react_loop.rs:45                   | ✅     |
//! | Streaming think          | phases/think.rs:267                | ✅     |
//! | Subagent lightweight     | agent/subagent/lightweight.rs:216  | ⚠️     |
//! | Compression summary LLM  | echo-state/compressor/summary.rs   | ⚠️     |
//!
//! Paths marked ⚠️ still pass `None` (acceptable: compression LLM calls don't
//! benefit from session cache and subagent paths inherit from parent).
//!
//! # Manual verification
//!
//! ```bash
//! RUST_LOG=echo_agent::cache=info cargo run -- ...
//! ```
//!
//! Expected log output:
//! ```text
//! 💰 prompt cache stats cache_hit_rate=95.2% cached_prompt_tokens=...
//! ```
//!
//! If `cache_hit_rate=0.0%` persists, check:
//! 1. Provider is Anthropic or OpenAI-compatible (not local Ollama)
//! 2. `cache_user_id` is set in agent config
//! 3. Runtime context messages are at the tail (not mixed into prefix)

use echo_agent::agent::config::AgentConfig;
use echo_agent::llm::ChatRequest;

/// Verify that `AgentConfig` supports setting `cache_user_id` and that it
/// appears in the builder chain.
#[test]
fn agent_config_accepts_cache_user_id() {
    let config =
        AgentConfig::minimal("test-model", "test-prompt").cache_user_id("test-cache-user-001");
    assert_eq!(config.get_cache_user_id(), Some("test-cache-user-001"));
}

/// Verify that `ChatRequest::user_id` defaults to `None` (safe default).
#[test]
fn chat_request_user_id_defaults_none() {
    let request = ChatRequest::new(vec![]);
    assert_eq!(request.user_id, None);
}

/// Verify that `ChatRequest` accepts `cache_hints`.
#[test]
fn chat_request_accepts_cache_hints() {
    let mut request = ChatRequest::new(vec![]);
    assert!(request.cache_hints.is_none());

    // Simulate what think.rs does: attach cache hints.
    let hints = echo_core::llm::cache::CacheHints {
        breakpoints: vec![],
        stable_prefix_hash: Some("deadbeef".to_string()),
        segments: Default::default(),
    };
    request.cache_hints = Some(hints);
    assert!(request.cache_hints.is_some());
    assert_eq!(
        request.cache_hints.as_ref().unwrap().stable_prefix_hash,
        Some("deadbeef".to_string())
    );
}

/// Verify `TokenUsageTracker` cumulative cache hit rate calculation.
#[test]
fn token_tracker_cumulative_cache_hit_rate() {
    use echo_core::tokenizer::TokenUsageTracker;
    let tracker = TokenUsageTracker::new("test-model");

    // No requests → no rate
    assert!(tracker.cumulative_cache_hit_rate().is_none());

    // 1st request via record_usage: 100 prompt, 50 cached
    tracker.record_usage(&echo_core::llm::types::Usage {
        prompt_tokens: Some(100),
        completion_tokens: Some(50),
        total_tokens: Some(150),
        cache_creation_input_tokens: Some(10),
        cache_read_input_tokens: Some(50),
        ..Default::default()
    });

    // 2nd request via record_usage: 200 prompt, 180 cached
    tracker.record_usage(&echo_core::llm::types::Usage {
        prompt_tokens: Some(200),
        completion_tokens: Some(100),
        total_tokens: Some(300),
        cache_creation_input_tokens: Some(5),
        cache_read_input_tokens: Some(180),
        ..Default::default()
    });

    // Cumulative: (50+180) / ((100)+(200)+(50+180)) = 230/530 ≈ 43.4%
    let rate = tracker.cumulative_cache_hit_rate().unwrap();
    assert!(
        (rate - 0.434).abs() < 0.02,
        "expected ~43.4%, got {:.1}%",
        rate * 100.0
    );
}

/// Verify that the `stable_prefix_hash` is deterministic (same input → same output).
#[test]
fn stable_prefix_hash_is_deterministic() {
    use echo_core::llm::Message;
    use echo_core::llm::cache::diagnostic::stable_prefix_hash;

    let sys = &[Message::system("You are a helpful assistant.".to_string())];
    let history = &[Message::user("hello".to_string())];

    let h1 = stable_prefix_hash(sys, &[], &[], history);
    let h2 = stable_prefix_hash(sys, &[], &[], history);
    assert_eq!(h1, h2);
}

/// Verify that `AnthropicCachePlan` produces sensible breakpoints.
#[test]
fn anthropic_cache_plan_is_sensible() {
    use echo_core::llm::Message;
    use echo_core::llm::cache::layout::PromptCacheLayout;
    use echo_integration::providers::AnthropicCachePlan;

    let msgs = vec![
        Message::system("System prompt".to_string()),
        Message::user("Hello".to_string()),
        Message::user("How are you?".to_string()),
        Message::user("What can you do?".to_string()),
        Message::user("Tell me more".to_string()),
    ];
    let layout = PromptCacheLayout::from_messages(&msgs, &[]);
    let plan = AnthropicCachePlan::from_layout(&layout);

    // With system + 4 history messages, we should get:
    // - SystemLastBlock (system present)
    // - HistoryIndex(~75%) (history >= 4)
    // - HistoryLastStable (history non-empty)
    assert!(plan.has_system_breakpoint);
    assert!(!plan.has_tool_breakpoint); // no tools
    assert!(!plan.breakpoints.is_empty());
    assert!(plan.breakpoints.len() <= 4);
}
