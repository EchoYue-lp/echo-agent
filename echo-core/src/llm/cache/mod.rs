//! Prompt cache layout and diagnostics.
//!
//! This module provides a **read-only, zero-copy** view over the flat messages
//! array that identifies cache-relevant segments (system, canonical context,
//! conversation history, runtime context). Providers can use this view to:
//!
//! 1. Place explicit cache breakpoints (Anthropic `cache_control`)
//! 2. Verify prefix stability for automatic prefix caches (OpenAI-compatible)
//! 3. Compute stable prefix hashes for cache invalidation diagnostics
//!
//! This does **not** change how messages are stored — it is purely a view.

pub mod diagnostic;
pub mod layout;

pub use diagnostic::stable_prefix_hash;
pub use layout::{BreakpointTarget, CacheHints, PromptCacheLayout, SegmentRange, SegmentRanges};
