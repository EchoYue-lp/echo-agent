//! Revisioned tool-visibility control shared by framework consumers.
//!
//! The control owns only the caller's explicit disabled-tool policy. Agent
//! creation, capability filtering, workspace scope, and surface receipts stay
//! with the embedding application.

use std::collections::HashSet;
use std::sync::RwLock;

/// Errors returned while mutating the framework tool-visibility policy.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ToolControlError {
    #[error("tool name cannot be empty")]
    EmptyName,
    #[error("tool '{name}' is not registered")]
    NotRegistered { name: String },
    #[error("tool-control generation is exhausted")]
    GenerationExhausted,
    #[error("tool-control policy lock is poisoned")]
    StateUnavailable,
}

/// Snapshot of the explicit tool-visibility policy.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolControlSnapshot {
    pub revision: u64,
    pub disabled_tools: HashSet<String>,
}

impl ToolControlSnapshot {
    /// Return the invocation-level representation expected by an Agent.
    pub fn disabled_option(&self) -> Option<HashSet<String>> {
        (!self.disabled_tools.is_empty()).then(|| self.disabled_tools.clone())
    }
}

/// Result of one idempotent policy mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolControlMutation {
    pub name: String,
    pub policy_enabled: bool,
    pub changed: bool,
    pub revision: u64,
}

/// Shared revisioned authority for explicit tool visibility choices.
#[derive(Debug, Default)]
pub struct ToolControlService {
    state: RwLock<ToolControlSnapshot>,
}

impl ToolControlService {
    /// Read the current policy snapshot.
    pub fn snapshot(&self) -> Result<ToolControlSnapshot, ToolControlError> {
        self.state
            .read()
            .map(|state| state.clone())
            .map_err(|_| ToolControlError::StateUnavailable)
    }

    /// Enable or disable one named tool. Repeating the same choice is
    /// idempotent and does not advance the revision.
    pub fn set_enabled(
        &self,
        name: &str,
        enabled: bool,
    ) -> Result<ToolControlMutation, ToolControlError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(ToolControlError::EmptyName);
        }
        let mut state = self
            .state
            .write()
            .map_err(|_| ToolControlError::StateUnavailable)?;
        let changed = if enabled {
            state.disabled_tools.contains(name)
        } else {
            !state.disabled_tools.contains(name)
        };
        let revision = if changed {
            state
                .revision
                .checked_add(1)
                .ok_or(ToolControlError::GenerationExhausted)?
        } else {
            state.revision
        };
        if enabled {
            state.disabled_tools.remove(name);
        } else {
            state.disabled_tools.insert(name.to_string());
        }
        state.revision = revision;
        Ok(ToolControlMutation {
            name: name.to_string(),
            policy_enabled: enabled,
            changed,
            revision,
        })
    }
}
