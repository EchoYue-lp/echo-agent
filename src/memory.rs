//! Layered memory system for persistent agent state.
//!
//! The memory system provides three layers, each with its own purpose:
//!
//! - **Store** — Long-term key-value storage with namespace isolation.
//!   Backed by [`InMemoryStore`], [`FileStore`], or [`SqliteStore`] (requires feature `sqlite`).
//!   Used for L3 memory promotion (compression evicts → write here → recall later).
//! - **ConversationStore** — User-visible transcript projection (one row per
//!   message, `StoredMessage` shape). Drives the GUI/TUI history panes.
//!   The framework persists this automatically at `run_core_loop` finalization.
//! - **RuntimeStateStore** — Full runtime checkpoint (messages + plan +
//!   active_skills + blocked_reason + TaskNode DAG) used to resume an
//!   in-flight conversation across process restarts. See [`crate::state`].
//!
//! # Quick Start
//!
//! ```rust,no_run
//! use echo_agent::prelude::*;
//! use std::sync::Arc;
//!
//! # fn main() -> echo_agent::error::Result<()> {
//! let store = Arc::new(InMemoryStore::new());
//! let agent = ReactAgentBuilder::new()
//!     .model("qwen3-max")
//!     .with_memory_tools(store)  // registers remember, recall, search_memory, forget
//!     .build()?;
//! # Ok(())
//! # }
//! ```
//!
//! # Key Types
//!
//! | Type | Description |
//! |------|-------------|
//! | [`Store`] | Trait for long-term memory backends |
//! | [`InMemoryStore`] / [`FileStore`] | Built-in store implementations |
//! | [`SqliteStore`] | SQLite-backed store (feature `sqlite`) |
//! | [`SnapshotManager`] | Capture and restore agent state at any point |
//! | [`crate::state::RuntimeStateStore`] | Full runtime checkpoint for crash recovery |
//! | [`ConversationStore`] | User-visible transcript projection |

/// Direct re-exports from `echo_state::memory`.
pub mod state {
    pub use echo_state::memory::*;
}

pub use echo_state::memory::*;
