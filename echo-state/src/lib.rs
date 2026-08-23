//! # echo-state
//!
//! State management layer for the [echo-agent](https://crates.io/crates/echo_agent) framework.
//!
//! ## Modules
//!
//! | Module | Description |
//! |--------|-------------|
//! | [`memory`] | Long-term KV memory (`Store`) + user-visible transcript projection (`ConversationStore`) |
//! | [`compression`] | Context compression: SlidingWindow, LLM Summary, and Hybrid strategies |
//! | [`audit`] | Structured audit logging with pluggable backends (in-memory, file) |
//! | [`journal`] | Sequenced event journal with checkpoint-compounded reduction |
//! | [`skill_telemetry`] | Skill execution telemetry: activation tracking, success/failure metrics |
//! | [`profiles`] | Agent capability profile + User preference profile with prompt injection |
//!
//! ## Feature Flags
//!
//! - `sqlite` — Enable `SqliteStore` for disk-backed persistent memory
//!
//! Most users should depend on `echo_agent` (the facade crate) instead of
//! depending on `echo_state` directly.

pub mod audit;
pub mod compression;
pub mod journal;
pub mod memory;
pub mod profiles;
pub mod skill_telemetry;
mod util;
