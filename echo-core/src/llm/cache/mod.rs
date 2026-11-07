//! Prompt cache layout and diagnostics.
//!
//! This module provides a **read-only, zero-copy** view over the flat messages
//! array that identifies cache-relevant segments (system, canonical context,
//! conversation history, appended dynamic facts). Providers can use this view to:
//!
//! 1. Place explicit cache breakpoints (Anthropic `cache_control`)
//! 2. Verify prefix stability for automatic prefix caches (OpenAI-compatible)
//! 3. Compute stable prefix hashes and post-adapter full-prompt fingerprints
//!    for cache invalidation diagnostics
//!
//! 分段来自框架投影元数据（投影信封 + version），不再按产品 marker 或正文
//! 前缀猜测；`dynamic` 只表示尾部追加式事实，属于可复用前缀之后的可变尾段。
//!
//! This does **not** change how messages are stored — it is purely a view.

pub mod diagnostic;
pub mod layout;
pub mod wire;

pub use diagnostic::{
    FullPromptFingerprint, PromptCacheFingerprint, PromptFieldDifference, PromptPrefixDiff,
    full_prompt_fingerprint, prompt_cache_fingerprint, prompt_prefix_diff, stable_prefix_hash,
};
pub use layout::{
    BreakpointTarget, CacheHints, PromptCacheLayout, SegmentRange, SegmentRanges,
    is_dynamic_tail_message,
};
