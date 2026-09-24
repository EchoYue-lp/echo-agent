//! Layered memory system for persistent agent state.
//!
//! The memory system provides three layers, each with its own purpose:
//!
//! - **Store** — Long-term key-value storage with namespace isolation.
//!   Backed by [`InMemoryStore`], [`FileStore`], or `SqliteStore` (requires feature `sqlite`).
//!   Layered memory writes evidence-bearing Drafts here. An explicit
//!   journal-bound activation makes a Draft eligible for later recall;
//!   raw Store values are general-purpose KV data, not approved memory.
//! - **ConversationStore** — User-visible transcript projection (one row per
//!   message, `StoredMessage` shape). Drives the application UI history panes.
//!   The framework persists this automatically at `run_core_loop` finalization.
//! - **RuntimeStateStore** — ReAct runtime checkpoint (messages +
//!   active_skills + blocked_reason) used to resume an in-flight conversation
//!   across process restarts. The public `current_plan` field only supports
//!   legacy Store round trips and is not restored by ReactAgent. See
//!   [`crate::state`].
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
//!     .with_memory_tools(store)  // approved recall/search; manager enables remember/forget
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
//! | `SqliteStore` | SQLite-backed store (feature `sqlite`) |
//! | [`SnapshotManager`] | Capture and restore agent state at any point |
//! | [`crate::state::RuntimeStateStore`] | Full runtime checkpoint for crash recovery |
//! | [`ConversationStore`] | User-visible transcript projection |

/// Direct re-exports from `echo_state::memory`.
pub mod state {
    pub use echo_state::memory::*;
}

/// Long-term memory store contracts.
pub mod store {
    pub use echo_state::memory::store::*;
}

pub use echo_state::memory::*;
