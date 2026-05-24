//! Hook type definitions — shared across all workspace crates.
//!
//! This module contains the core types for the unified hook system,
//! defined here in `echo-core` so that both `echo-execution` and
//! `echo-orchestration` can reference them without circular dependencies.

pub mod types;

pub use types::{
    CompressHookStats, HookContext, HookEvent, HookEventCategory, HookResult, HookSource,
    UnifiedHookExecutorFn,
};
