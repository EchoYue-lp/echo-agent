//! Worktree isolation for Fork-dispatched writer workers (Sprint 8).
//!
//! When a subagent's [`crate::agent::subagent::types::SubagentDefinition`] declares
//! `isolate_worktree: true`, the [`crate::agent::subagent::executor::SubagentExecutor`]
//! asks the injected [`WorktreeFactory`] for an isolated git worktree, binds the
//! worker agent's `working_dir` to it, runs the worker, and finally asks the
//! factory for a diff summary. This mirrors Claude Code's `isolation: worktree`
//! subagent frontmatter and Codex/Cursor's per-agent worktree checkout
//! (industry-consensus pattern, 2025-2026).
//!
//! # Why a framework trait (not a concrete impl)
//!
//! Worktree creation is git-subprocess + branch-naming + lifecycle bookkeeping
//! — all product-form concerns that live in the application layer (EKO's
//! `RunWorktree` / `UnattendedWriteMode`). The framework (`echo_agent`) must
//! not depend on the application, so it defines this trait and lets the
//! application supply the concrete factory. The framework only needs the
//! *contract*: "give me an isolated path + a way to summarize after".
//!
//! # Safety gate
//!
//! If a worker declares `isolate_worktree: true` but **no factory is
//! configured**, the framework logs a warning and runs without isolation
//! (the application decided not to supply worktrees). But if a factory **is**
//! configured and `create` fails, the Fork dispatch fails hard — never silently
//! continue, since that would let a writer touch the main checkout without the
//! promised isolation (data-loss hazard, AGENTS.md "本地场景下为何仍需要").

use std::path::PathBuf;
use std::sync::Arc;

/// Error returned by worktree operations. Minimal string wrapper — the
/// framework doesn't model git internals; the application's concrete factory
/// surfaces the useful diagnostic in the message.
#[derive(Debug)]
pub struct WorktreeError {
    pub message: String,
}

impl std::fmt::Display for WorktreeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for WorktreeError {}

impl WorktreeError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// A created worktree, returned by [`WorktreeFactory::create`] and consumed by
/// the framework after the worker finishes.
///
/// `finalize` is application-defined: typically it generates a `git diff`
/// summary of the worktree's changes and optionally keeps/removes the worktree.
/// Returning it as a `Box<dyn FnOnce>` keeps the framework free of git deps;
/// the application closes over its own `RunWorktree` handle.
pub struct WorktreeHandle {
    /// Absolute path to the worktree checkout — bound as the worker's
    /// `working_dir` so every shell/file/git tool runs inside it.
    pub path: PathBuf,
    /// Run once after the worker finishes. Returns a diff/summary string
    /// (surfaced to the caller) or an error. Owns the worktree lifecycle
    /// (keep vs remove is the application's policy).
    pub finalize: Box<dyn FnOnce() -> Result<String, WorktreeError> + Send>,
}

impl std::fmt::Debug for WorktreeHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorktreeHandle")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

/// Framework trait: lets an application supply git-worktree isolation to
/// Fork-dispatched workers.
///
/// Implementations must be `Send + Sync` (stored behind an `Arc` in the
/// executor config, shared across spawned dispatches).
pub trait WorktreeFactory: Send + Sync {
    /// Create an isolated worktree for one worker dispatch.
    ///
    /// `label` identifies the dispatch (e.g. `"{agent_name}-{run_id}"`) and is
    /// used by the application to name the worktree branch.
    fn create(&self, label: &str) -> Result<WorktreeHandle, WorktreeError>;
}

/// A no-op factory used as the default (no isolation). Provided for
/// completeness/tests; production supplies a real git-backed factory.
#[derive(Debug, Default, Clone)]
pub struct NoWorktreeFactory;

impl WorktreeFactory for NoWorktreeFactory {
    fn create(&self, _label: &str) -> Result<WorktreeHandle, WorktreeError> {
        Err(WorktreeError::new(
            "NoWorktreeFactory cannot create worktrees (no isolation configured)",
        ))
    }
}

/// Convenience type alias for the shared factory stored in executor config.
pub type SharedWorktreeFactory = Arc<dyn WorktreeFactory>;

#[cfg(test)]
mod tests {
    use super::*;

    /// A test factory that "creates" a temp dir and finalizes with a canned diff.
    struct MockFactory {
        created: std::sync::Mutex<Vec<String>>,
        should_fail: bool,
    }

    impl WorktreeFactory for MockFactory {
        fn create(&self, label: &str) -> Result<WorktreeHandle, WorktreeError> {
            if self.should_fail {
                return Err(WorktreeError::new("mock create failure"));
            }
            self.created.lock().unwrap().push(label.to_string());
            Ok(WorktreeHandle {
                path: PathBuf::from(format!("/tmp/mock-{label}")),
                finalize: Box::new(|| Ok("mock diff summary".to_string())),
            })
        }
    }

    #[test]
    fn mock_factory_create_and_finalize() {
        let factory = MockFactory {
            created: std::sync::Mutex::new(Vec::new()),
            should_fail: false,
        };
        let handle = factory.create("worker-run42").unwrap();
        assert_eq!(handle.path, PathBuf::from("/tmp/mock-worker-run42"));
        assert_eq!((handle.finalize)().unwrap(), "mock diff summary");
        assert_eq!(factory.created.lock().unwrap().len(), 1);
    }

    #[test]
    fn mock_factory_failure_propagates() {
        let factory = MockFactory {
            created: std::sync::Mutex::new(Vec::new()),
            should_fail: true,
        };
        let err = factory.create("x").unwrap_err();
        assert!(err.message.contains("mock create failure"));
    }

    #[test]
    fn no_worktree_factory_errors() {
        let f = NoWorktreeFactory;
        assert!(f.create("x").is_err());
    }

    #[test]
    fn shared_factory_dyn_compatible() {
        // Confirms the trait is object-safe (can be stored as Arc<dyn ...>).
        let f: SharedWorktreeFactory = Arc::new(MockFactory {
            created: std::sync::Mutex::new(Vec::new()),
            should_fail: false,
        });
        assert!(f.create("y").is_ok());
    }
}
