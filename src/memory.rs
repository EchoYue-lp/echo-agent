//! Dual-layer memory system for persistent agent state.
//!
//! The memory system provides two complementary layers:
//!
//! - **Store** — Long-term key-value storage with namespace isolation.
//!   Backed by [`InMemoryStore`], [`FileStore`], or [`SqliteStore`] (requires feature `sqlite`).
//! - **Checkpointer** — Session history preservation across restarts.
//!   Backed by [`InMemoryCheckpointer`] or [`FileCheckpointer`].
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
//! | [`Checkpointer`] | Trait for session persistence |
//! | [`InMemoryStore`] / [`FileStore`] | Built-in store implementations |
//! | [`SqliteStore`] | SQLite-backed store (feature `sqlite`) |
//! | [`SnapshotManager`] | Capture and restore agent state at any point |

/// Direct re-exports from `echo_state::memory`.
pub mod state {
    pub use echo_state::memory::*;
}

pub use echo_state::memory::*;
