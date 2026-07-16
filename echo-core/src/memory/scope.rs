//! Memory scope — layering for different memory lifetimes.
//!
//! # Scopes
//!
//! - **User**: Cross-project user preferences and patterns.
//! - **Project**: Project-specific conventions and rules.
//! - **Repo**: Repository-specific patterns (nested within project).
//! - **Task**: Current task state (active until task completion).
//! - **Session**: Current session context (active until agent restart).
//! - **Run**: Single execution run (cleared after each run).

use serde::{Deserialize, Serialize};

/// The lifetime layer for a memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryScope {
    /// Cross-project user preferences (persisted indefinitely).
    User,
    /// Project-specific conventions (persisted with project).
    Project,
    /// Repository-specific patterns (nested within project).
    Repo,
    /// Current task state (cleared when task completes).
    Task,
    /// Current session context (cleared on agent restart).
    Session,
    /// Single execution run (cleared after each run).
    Run,
}

impl MemoryScope {
    /// Human-readable name.
    pub fn name(&self) -> &str {
        match self {
            Self::User => "user",
            Self::Project => "project",
            Self::Repo => "repo",
            Self::Task => "task",
            Self::Session => "session",
            Self::Run => "run",
        }
    }

    /// Priority order (lower = longer-lived).
    pub fn priority(&self) -> u8 {
        match self {
            Self::User => 0,
            Self::Project => 1,
            Self::Repo => 2,
            Self::Task => 3,
            Self::Session => 4,
            Self::Run => 5,
        }
    }

    /// Whether this scope persists across agent restarts.
    pub fn is_persistent(&self) -> bool {
        matches!(self, Self::User | Self::Project | Self::Repo)
    }

    /// All scopes in priority order.
    pub fn all() -> [Self; 6] {
        [
            Self::User,
            Self::Project,
            Self::Repo,
            Self::Task,
            Self::Session,
            Self::Run,
        ]
    }

    /// Parse from a string.
    ///
    /// This intentionally returns `Option` and accepts aliases, unlike a
    /// strict `FromStr` implementation.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "user" => Some(Self::User),
            "project" | "proj" => Some(Self::Project),
            "repo" => Some(Self::Repo),
            "task" => Some(Self::Task),
            "session" | "sess" => Some(Self::Session),
            "run" => Some(Self::Run),
            _ => None,
        }
    }
}
