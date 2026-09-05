//! Extension object identity and generation fences.
//!
//! Framework objects that cross the SDK boundary are referenced by stable,
//! non-empty domain identities plus a generation counter (design §8). Domain
//! identity never borrows the JSON-RPC request id: ACP request ids follow the
//! official schema and may repeat or be numeric, while framework identity is
//! assigned once and lives as long as the object's facts do.
//!
//! A handle whose generation no longer matches must resolve to a typed
//! stale/closed error (see `error::ExtensionErrorCode`), never silently bind
//! to a newer object.

use serde::{Deserialize, Serialize};

use crate::scalar::WireU64;

/// What kind of framework object a handle refers to. Typed kinds keep stale
/// errors diagnosable across the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HandleKind {
    Agent,
    Session,
    Run,
    Stream,
    TaskRun,
    PlanTask,
    Subagent,
    Extension,
}

impl HandleKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            HandleKind::Agent => "agent",
            HandleKind::Session => "session",
            HandleKind::Run => "run",
            HandleKind::Stream => "stream",
            HandleKind::TaskRun => "task_run",
            HandleKind::PlanTask => "plan_task",
            HandleKind::Subagent => "subagent",
            HandleKind::Extension => "extension",
        }
    }
}

/// Opaque handle to a framework object: non-empty domain id + generation.
///
/// Ids are strings assigned by the framework authority; the SDK never
/// interprets their content, never exposes memory addresses, and never
/// reuses an id across kinds.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WireHandle {
    /// Non-empty domain identity assigned by the framework.
    #[schemars(length(min = 1, max = 256))]
    pub id: String,
    /// Monotonic generation of the owning Host; incremented across restarts
    /// so pre-restart handles fail as stale instead of rebinding.
    pub generation: WireU64,
    pub kind: HandleKind,
}

impl WireHandle {
    /// Validate the handle shape; empty ids are contract violations reported
    /// as `invalid_value`, never panics.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.id.trim().is_empty() {
            return Err("handle id must be non-empty");
        }
        if self.id.chars().count() > 256 {
            return Err("handle id exceeds 256 characters");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_round_trip() {
        let handle = WireHandle {
            id: "run-7".to_string(),
            generation: WireU64::from_u64(3),
            kind: HandleKind::Run,
        };
        let json = serde_json::to_string(&handle).unwrap_or_default();
        let back: WireHandle = serde_json::from_str(&json).unwrap_or(WireHandle {
            id: String::new(),
            generation: WireU64::from_u64(0),
            kind: HandleKind::Agent,
        });
        assert_eq!(back, handle);
        assert_eq!(back.kind.as_str(), "run");
    }

    #[test]
    fn empty_id_fails_validation() {
        let handle = WireHandle {
            id: "  ".to_string(),
            generation: WireU64::from_u64(1),
            kind: HandleKind::Session,
        };
        assert!(handle.validate().is_err());
    }
}
