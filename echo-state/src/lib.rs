//! # echo-state
//!
//! State management layer for the [echo-agent](https://crates.io/crates/echo_agent) framework.
//!
//! ## Modules
//!
//! | Module | Description |
//! |--------|-------------|
//! | [`memory`] | Dual-layer memory: `Store` (long-term KV) + `Checkpointer` (session persistence) |
//! | [`compression`] | Context compression: SlidingWindow, LLM Summary, and Hybrid strategies |
//! | [`audit`] | Structured audit logging with pluggable backends (in-memory, file) |
//!
//! ## Feature Flags
//!
//! - `sqlite` — Enable `SqliteStore` for disk-backed persistent memory
//!
//! Most users should depend on `echo_agent` (the facade crate) instead of
//! depending on `echo_state` directly.

pub mod audit;
pub mod compression;
pub mod memory;
mod util;
