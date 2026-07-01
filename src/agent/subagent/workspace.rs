//! Data/research workspace isolation for Fork-dispatched workers (Sprint 10).
//!
//! Companion to [`super::worktree`]: where worktree isolation suits *code
//! writers* (git checkout + diff/merge lifecycle), **data/research workers**
//! emit generated artifacts (CSVs, parquet, charts) with no source tree to
//! diff against, often outside any git repo. They need a **disjoint working
//! directory per worker** so parallel data-shaper/analyst workers don't
//! overwrite each other's output — but NOT git coupling.
//!
//! This module defines the framework trait [`DataWorkspaceFactory`]; the
//! application supplies a concrete impl (a `tempfile::TempDir`-per-worker
//! handle). When a subagent's [`crate::agent::subagent::types::SubagentDefinition`]
//! declares `isolate_workspace: true`, the
//! [`crate::agent::subagent::executor::SubagentExecutor`] asks the injected
//! factory for a fresh workspace, binds the worker's `working_dir` to it, runs
//! the worker, and finally asks the factory to summarize (typically: list
//! generated files). The data tools (Polars-based `export_data`, etc.) then
//! naturally write disjoint output files — satisfying the Sprint 10 acceptance
//! ("data worker 并行跑不互相污染; 输出文件不相交; analyst 能综合") without
//! csv/parquet mid-stream merge.
//!
//! Mirrors the Sprint 8 `WorktreeFactory` shape so the executor treats both
//! isolation kinds uniformly (a worker declares AT MOST ONE of
//! `isolate_worktree` / `isolate_workspace`).

use std::path::PathBuf;
use std::sync::Arc;

/// Error returned by data-workspace operations. Minimal string wrapper — the
/// framework doesn't model tmpdir internals; the application's concrete factory
/// surfaces the useful diagnostic.
#[derive(Debug)]
pub struct WorkspaceError {
    pub message: String,
}

impl std::fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for WorkspaceError {}

impl WorkspaceError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// A created workspace, returned by [`DataWorkspaceFactory::create`] and
/// consumed by the framework after the worker finishes.
///
/// `finalize` is application-defined: typically it lists the generated files in
/// the workspace (so the orchestrator/analyst can find each worker's outputs
/// for concat/synthesize). Returning it as `Box<dyn FnOnce>` keeps the
/// framework free of fs deps; the application closes over its own
/// `TempDir`/cleanup handle.
pub struct DataWorkspaceHandle {
    /// Absolute path to the workspace dir — bound as the worker's `working_dir`
    /// so every data/shell/file tool runs inside it (output files land here).
    pub path: PathBuf,
    /// Run once after the worker finishes. Returns a summary string (e.g.
    /// listing of generated files) surfaced to the caller, or an error. Owns
    /// the workspace lifecycle (keep for collect vs clean up is the
    /// application's policy).
    pub finalize: Box<dyn FnOnce() -> Result<String, WorkspaceError> + Send>,
}

impl std::fmt::Debug for DataWorkspaceHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DataWorkspaceHandle")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

/// Framework trait: lets an application supply a per-worker disjoint working
/// directory for Fork-dispatched data/research workers.
///
/// Implementations must be `Send + Sync` (stored behind an `Arc` in the
/// executor config, shared across spawned dispatches).
pub trait DataWorkspaceFactory: Send + Sync {
    /// Create an isolated workspace for one worker dispatch.
    ///
    /// `label` identifies the dispatch (e.g. `"{agent_name}-{run_id}"`) and may
    /// be used by the application to name a subdirectory for traceability.
    fn create(&self, label: &str) -> Result<DataWorkspaceHandle, WorkspaceError>;
}

/// A no-op factory used as the default (no workspace isolation). Provided for
/// completeness/tests; production supplies a tmpdir-backed factory.
#[derive(Debug, Default, Clone)]
pub struct NoWorkspaceFactory;

impl DataWorkspaceFactory for NoWorkspaceFactory {
    fn create(&self, _label: &str) -> Result<DataWorkspaceHandle, WorkspaceError> {
        Err(WorkspaceError::new(
            "NoWorkspaceFactory cannot create workspaces (no data-workspace isolation configured)",
        ))
    }
}

/// Convenience type alias for the shared factory stored in executor config.
pub type SharedDataWorkspaceFactory = Arc<dyn DataWorkspaceFactory>;

#[cfg(test)]
mod tests {
    use super::*;

    /// A test factory that "creates" a fake dir and finalizes with a canned
    /// file listing.
    struct MockFactory {
        created: std::sync::Mutex<Vec<String>>,
        should_fail: bool,
    }

    impl DataWorkspaceFactory for MockFactory {
        fn create(&self, label: &str) -> Result<DataWorkspaceHandle, WorkspaceError> {
            if self.should_fail {
                return Err(WorkspaceError::new("mock workspace create failed"));
            }
            self.created.lock().unwrap().push(label.to_string());
            Ok(DataWorkspaceHandle {
                path: PathBuf::from(format!("/tmp/mock-ws-{label}")),
                finalize: Box::new(|| {
                    Ok("generated: run_001_clean.parquet\nrun_001_stats.json".to_string())
                }),
            })
        }
    }

    #[test]
    fn mock_factory_create_and_finalize() {
        let factory = MockFactory {
            created: std::sync::Mutex::new(Vec::new()),
            should_fail: false,
        };
        let handle = factory.create("analyst-run42").unwrap();
        assert_eq!(handle.path, PathBuf::from("/tmp/mock-ws-analyst-run42"));
        let summary = (handle.finalize)().unwrap();
        assert!(summary.contains("run_001_clean.parquet"));
        assert_eq!(factory.created.lock().unwrap().len(), 1);
    }

    #[test]
    fn mock_factory_failure_propagates() {
        let factory = MockFactory {
            created: std::sync::Mutex::new(Vec::new()),
            should_fail: true,
        };
        let err = factory.create("x").unwrap_err();
        assert!(err.message.contains("mock workspace create failed"));
    }

    #[test]
    fn no_workspace_factory_errors() {
        let f = NoWorkspaceFactory;
        assert!(f.create("x").is_err());
    }

    #[test]
    fn shared_factory_dyn_compatible() {
        // Confirms the trait is object-safe.
        let f: SharedDataWorkspaceFactory = Arc::new(MockFactory {
            created: std::sync::Mutex::new(Vec::new()),
            should_fail: false,
        });
        assert!(f.create("y").is_ok());
    }
}
