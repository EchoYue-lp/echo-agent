//! Command cell contract — background shell commands with long-poll wait.
//!
//! Defines the [`CommandCellRegistry`] trait and its data types. The contract
//! lives in `echo_core` (pure data + trait, no runtime) so that `echo_tools`
//! (which only depends on `echo_core`) can launch cells from `ShellTool`,
//! while the concrete implementation ([`crate::sandbox::SandboxExecutor`]-style
//! runtime behavior) lives in higher crates such as `echo_orchestration`.
//!
//! # Cell semantics (Codex-style background commands)
//!
//! - `launch` registers and starts a background command, returning a
//!   `cell_id` immediately (non-blocking).
//! - `wait` long-polls a cell: it returns when the cell reaches a terminal
//!   phase, new output appears, or the caller's yield budget expires. It is
//!   **retry-safe**: the terminal state is readable repeatedly (multiple
//!   waiters — e.g. main agent + awaiter subagent — can all observe the same
//!   final state), and output is consumed via a byte-cursor the caller passes
//!   back on the next call.
//! - `stop` kills the underlying process; `list` snapshots every cell.
//!
//! [`CommandCellSnapshot::total_output_bytes`] is a monotonically increasing
//! byte offset into the full (logical) output stream; callers treat the
//! returned [`CommandCellDelta::next_cursor`] as opaque and re-pass it.

use futures::future::BoxFuture;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use super::artifact::{
    ToolOutputArtifactConfig, ToolOutputArtifactIdentity, ToolOutputArtifactRef,
};

/// Application/runtime ownership metadata carried across the tool boundary.
///
/// The generic cell runtime treats this as opaque correlation data. Product
/// adapters can use it to project cell lifecycle events into their own run
/// store without teaching the framework about an application TaskRun model.
#[derive(Debug, Clone, Default)]
pub struct CommandCellOwner {
    pub run_id: Option<String>,
    pub turn_id: Option<String>,
    pub execution_id: Option<String>,
    pub call_id: Option<String>,
}

/// Launch request for a background command cell.
///
/// The caller (e.g. `ShellTool`) is responsible for security validation
/// before calling [`CommandCellRegistry::launch`].
#[derive(Debug, Clone, Default)]
pub struct CommandCellRequest {
    /// Shell command line to execute (via `sh -c`).
    pub command: String,
    /// Optional working directory for the command.
    pub working_dir: Option<String>,
    /// Optional cell lifetime in seconds. `None` uses the registry default;
    /// `Some(0)` may mean "no timeout" for implementations that support it.
    pub timeout_secs: Option<u64>,
    /// The foreground shell had a sandbox executor, so the cell runtime must
    /// not silently downgrade this launch to direct host execution.
    pub require_sandbox: bool,
    /// Optional lifetime cancellation supplied by a registry caller. A normal
    /// foreground Turn token should not be used because cells may outlive the
    /// Turn; products with pause/resume semantics should stop cells explicitly
    /// on terminal run cancellation.
    pub cancel: Option<Arc<CancellationToken>>,
    /// Opaque application correlation metadata.
    pub owner: CommandCellOwner,
    /// Complete-output spill policy inherited from the tool context.
    pub output_artifacts: Option<ToolOutputArtifactConfig>,
    /// Stable artifact identity paired with `output_artifacts`.
    pub artifact_identity: Option<ToolOutputArtifactIdentity>,
}

/// Lifecycle phase of a command cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandCellPhase {
    /// Process is running (or queued to run).
    Running,
    /// Process exited with status success.
    Succeeded,
    /// Process exited with a non-zero status or timed out.
    Failed,
    /// Cancellation was requested and the process was killed.
    Cancelled,
    /// The process could not be spawned at all.
    LaunchFailed,
}

impl CommandCellPhase {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::LaunchFailed => "launch_failed",
        }
    }

    /// Whether this phase is terminal (the cell will not change state again).
    pub const fn is_terminal(self) -> bool {
        !matches!(self, Self::Running)
    }
}

/// Non-blocking snapshot of a cell's current state.
#[derive(Debug, Clone)]
pub struct CommandCellSnapshot {
    pub cell_id: String,
    /// UTF-8 safe preview of the command (first N chars).
    pub name: String,
    pub phase: CommandCellPhase,
    /// Exit code, available once the phase is terminal (e.g. `None` for
    /// timeouts and launch failures).
    pub exit_code: Option<i32>,
    /// Monotonic byte count of the full output stream (including any bytes
    /// dropped from the in-memory retention buffer).
    pub total_output_bytes: u64,
    /// Whether the in-memory retention buffer is capped (only the tail is
    /// retained; earlier output bytes were discarded).
    pub output_truncated: bool,
    /// Complete output artifact once the cell has finished and crossed the
    /// configured spill threshold.
    pub output_artifact: Option<ToolOutputArtifactRef>,
}

/// Result of one `wait` long-poll round.
#[derive(Debug, Clone)]
pub struct CommandCellDelta {
    pub snapshot: CommandCellSnapshot,
    /// Incremental output since the caller's cursor (UTF-8 lossy, byte-capped).
    pub new_output: String,
    /// Byte cursor to re-pass on the next `wait` call.
    pub next_cursor: u64,
    /// Whether `new_output` was elided: either the single-response byte cap
    /// hit, or output before the cursor had already been discarded from the
    /// retention buffer.
    pub output_elided: bool,
}

/// Registry of background command cells.
///
/// Implementations must be `Send + Sync` and support multiple concurrent
/// waiters on the same cell. All methods are non-blocking except `wait`,
/// which returns within the caller's yield budget.
pub trait CommandCellRegistry: Send + Sync {
    /// Register and start a background command cell immediately.
    /// Returns the new `cell_id`. The caller is responsible for security
    /// validation of `request.command`.
    fn launch(&self, request: CommandCellRequest) -> std::result::Result<String, String>;

    /// Long-poll a cell: return when it is terminal, when new output appears
    /// after `cursor`, or when `yield_ms` elapses (whichever comes first).
    /// Retry-safe: calling again with the returned `next_cursor` is always
    /// valid, and terminal states stay readable forever (within retention).
    /// `yield_ms = 0` is a non-blocking poll.
    fn wait(
        &self,
        cell_id: &str,
        cursor: u64,
        yield_ms: u64,
    ) -> BoxFuture<'_, std::result::Result<CommandCellDelta, String>>;

    /// Request that a cell's process be killed. Returns whether the cell
    /// exists (cancellation itself is asynchronous).
    fn stop(&self, cell_id: &str) -> bool;

    /// Snapshot every tracked cell (running and terminal).
    fn list(&self) -> BoxFuture<'_, Vec<CommandCellSnapshot>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_as_str_and_terminality() {
        assert_eq!(CommandCellPhase::Running.as_str(), "running");
        assert_eq!(CommandCellPhase::Succeeded.as_str(), "succeeded");
        assert_eq!(CommandCellPhase::Failed.as_str(), "failed");
        assert_eq!(CommandCellPhase::Cancelled.as_str(), "cancelled");
        assert_eq!(CommandCellPhase::LaunchFailed.as_str(), "launch_failed");

        assert!(!CommandCellPhase::Running.is_terminal());
        assert!(CommandCellPhase::Succeeded.is_terminal());
        assert!(CommandCellPhase::Failed.is_terminal());
        assert!(CommandCellPhase::Cancelled.is_terminal());
        assert!(CommandCellPhase::LaunchFailed.is_terminal());
    }
}
