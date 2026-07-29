//! File-backed [`RuntimeStateStore`] — a no-dependency JSON-file backend.
//!
//! One directory per conversation under `<base>/runtime_state/<safe_id>/`:
//!   - `nodes.json`       — the task-node DAG for that conversation
//!   - `checkpoint.json`  — the latest agent checkpoint (single-row upsert)
//!
//! This is the no-SQLite alternative to [`SqliteRuntimeStateStore`](crate::state::sqlite::SqliteRuntimeStateStore)
//! (`sqlite` feature). Suitable for a single-process local agent (typical
//! echo-agent consumer). For multi-process concurrency, use the SQLite backend.
//!
//! ## Robustness
//!
//! - **Path-safe ids.** Conversation ids are sanitized before joining into the
//!   path (rejecting `/`, `\`, `..`, empty) to prevent directory escapes.
//! - **Corrupt JSON is an error.** A malformed `nodes.json` / `checkpoint.json`
//!   surfaces as `ReactError::Other` rather than silently returning `None` /
//!   an empty list (which previously looked indistinguishable from "no data").
//! - **Unique temp names.** Each atomic write uses a uuid-suffixed temp file
//!   (no cross-write collisions; multi-process belt-and-suspenders).
//!
//! Atomic writes use tmp + fsync + rename so a crash never leaves a
//! half-written file.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::Utc;
use echo_core::error::ReactError;
use futures::future::BoxFuture;

use super::{AgentCheckpoint, RuntimeStateStore, TaskNode, TaskNodeStatus};

/// File-backed runtime state store.
pub struct FileRuntimeStateStore {
    base: PathBuf,
    /// Serializes all writes (the framework trait is `&self`; a Mutex keeps the
    /// read-modify-write in `save_node`/`update_status` atomic).
    lock: Mutex<()>,
}

impl FileRuntimeStateStore {
    /// Create a file-backed state store rooted at `base/runtime_state/`.
    pub fn new(base: impl AsRef<Path>) -> Result<Self, ReactError> {
        let base = base.as_ref().join("runtime_state");
        std::fs::create_dir_all(&base)
            .map_err(|e| ReactError::Other(format!("create runtime_state dir: {e}")))?;
        Ok(Self {
            base,
            lock: Mutex::new(()),
        })
    }

    fn conv_dir(&self, conversation_id: &str) -> Result<PathBuf, ReactError> {
        let safe = safe_segment(conversation_id)?;
        Ok(self.base.join(safe))
    }

    fn nodes_path(&self, conversation_id: &str) -> Result<PathBuf, ReactError> {
        Ok(self.conv_dir(conversation_id)?.join("nodes.json"))
    }

    fn checkpoint_path(&self, conversation_id: &str) -> Result<PathBuf, ReactError> {
        Ok(self.conv_dir(conversation_id)?.join("checkpoint.json"))
    }

    /// Read nodes. Missing file → empty Vec; corrupt file → `Err`.
    fn read_nodes_file(path: &Path) -> Result<Vec<TaskNode>, ReactError> {
        match std::fs::read_to_string(path) {
            Ok(s) => serde_json::from_str(&s)
                .map_err(|e| ReactError::Other(format!("parse {}: {e}", path.display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(ReactError::Other(format!("read {}: {e}", path.display()))),
        }
    }

    fn write_nodes_file(path: &Path, nodes: &[TaskNode]) -> Result<(), ReactError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ReactError::Other(format!("create dir: {e}")))?;
        }
        let json = serde_json::to_string_pretty(nodes)
            .map_err(|e| ReactError::Other(format!("serialize nodes: {e}")))?;
        atomic_write(path, json.as_bytes())
            .map_err(|e| ReactError::Other(format!("write nodes: {e}")))
    }

    fn to_react_err(e: impl std::fmt::Display) -> ReactError {
        ReactError::Other(format!("FileRuntimeStateStore: {e}"))
    }
}

impl RuntimeStateStore for FileRuntimeStateStore {
    fn save_node<'a>(
        &'a self,
        conversation_id: &'a str,
        node: &'a TaskNode,
    ) -> BoxFuture<'a, crate::error::Result<()>> {
        Box::pin(async move {
            let _g = self.lock.lock().map_err(Self::to_react_err)?;
            let path = self.nodes_path(conversation_id)?;
            let mut nodes = Self::read_nodes_file(&path)?;
            // Upsert: replace if same id, else push.
            if let Some(existing) = nodes.iter_mut().find(|n| n.id == node.id) {
                *existing = node.clone();
            } else {
                nodes.push(node.clone());
            }
            Self::write_nodes_file(&path, &nodes)?;
            Ok(())
        })
    }

    fn load_nodes<'a>(
        &'a self,
        conversation_id: &'a str,
    ) -> BoxFuture<'a, crate::error::Result<Vec<TaskNode>>> {
        Box::pin(async move {
            let _g = self.lock.lock().map_err(Self::to_react_err)?;
            Self::read_nodes_file(&self.nodes_path(conversation_id)?)
        })
    }

    fn update_status<'a>(
        &'a self,
        conversation_id: &'a str,
        node_id: &'a str,
        status: TaskNodeStatus,
    ) -> BoxFuture<'a, crate::error::Result<()>> {
        Box::pin(async move {
            let _g = self.lock.lock().map_err(Self::to_react_err)?;
            let path = self.nodes_path(conversation_id)?;
            let mut nodes = Self::read_nodes_file(&path)?;
            let now = Utc::now();
            let mut found = false;
            for n in nodes.iter_mut() {
                if n.id == node_id {
                    n.status = status.clone();
                    n.updated_at = now;
                    found = true;
                    break;
                }
            }
            if found {
                Self::write_nodes_file(&path, &nodes)?;
            }
            // Match SQL semantics: UPDATE on 0 rows is a no-op (no error).
            Ok(())
        })
    }

    fn get_checkpoint<'a>(
        &'a self,
        conversation_id: &'a str,
    ) -> BoxFuture<'a, crate::error::Result<Option<AgentCheckpoint>>> {
        Box::pin(async move {
            let _g = self.lock.lock().map_err(Self::to_react_err)?;
            let path = self.checkpoint_path(conversation_id)?;
            match std::fs::read_to_string(&path) {
                Ok(s) => {
                    let cp: AgentCheckpoint = serde_json::from_str(&s)
                        .map_err(|e| ReactError::Other(format!("parse {}: {e}", path.display())))?;
                    Ok(Some(cp))
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(Self::to_react_err(e)),
            }
        })
    }

    fn save_checkpoint<'a>(
        &'a self,
        checkpoint: &'a AgentCheckpoint,
    ) -> BoxFuture<'a, crate::error::Result<()>> {
        Box::pin(async move {
            let _g = self.lock.lock().map_err(Self::to_react_err)?;
            let path = self.checkpoint_path(&checkpoint.conversation_id)?;
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| ReactError::Other(format!("create dir: {e}")))?;
            }
            let json = serde_json::to_string_pretty(checkpoint)
                .map_err(|e| ReactError::Other(format!("serialize checkpoint: {e}")))?;
            atomic_write(&path, json.as_bytes())
                .map_err(|e| ReactError::Other(format!("write checkpoint: {e}")))?;
            Ok(())
        })
    }

    fn clear_conversation<'a>(
        &'a self,
        conversation_id: &'a str,
    ) -> BoxFuture<'a, crate::error::Result<()>> {
        Box::pin(async move {
            let _g = self.lock.lock().map_err(Self::to_react_err)?;
            let dir = self.conv_dir(conversation_id)?;
            // Remove the conversation directory if it exists; tolerate absence.
            match std::fs::remove_dir_all(&dir) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(Self::to_react_err(e)),
            }
        })
    }
}

/// Write `bytes` to `path` atomically (unique tmp + fsync + rename).
///
/// On Unix, the parent directory is fsynced after rename so the directory entry
/// is durable as well as the file content.
fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other(format!("path has no parent: {}", path.display())))?;
    std::fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("data"),
        uuid::Uuid::new_v4()
    ));
    let write_result = (|| {
        use std::io::Write;
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()
    })();
    if let Err(error) = write_result {
        let _ = std::fs::remove_file(&tmp);
        return Err(error);
    }
    if let Err(error) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(error);
    }
    sync_parent_directory(parent)?;
    Ok(())
}

#[cfg(unix)]
fn sync_parent_directory(parent: &Path) -> std::io::Result<()> {
    std::fs::File::open(parent)?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent_directory(_parent: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Sanitize an arbitrary id string into a single safe filesystem segment.
///
/// Mirrors the conversation-store version: rejects empty, path separators
/// (`/`, `\`), the traversal segment `..`, and non-path-safe characters.
/// Character-safe (no byte slicing).
fn safe_segment(id: &str) -> Result<String, ReactError> {
    if id.is_empty() {
        return Err(ReactError::Other("conversation id is empty".into()));
    }
    for ch in id.chars() {
        let safe = ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ':' | '~');
        if !safe {
            return Err(ReactError::Other(format!(
                "conversation id contains unsafe character {ch:?}"
            )));
        }
    }
    if id == ".." || id == "." || id.contains('/') || id.contains('\\') {
        return Err(ReactError::Other(format!(
            "conversation id is a path segment: {id:?}"
        )));
    }
    Ok(id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn file_runtime_state_lifecycle() {
        let tmp = std::env::temp_dir().join(format!(
            "echo-file-runtime-state-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let store = FileRuntimeStateStore::new(&tmp).unwrap();

        let node = TaskNode::new("node-1", "Plan task")
            .with_status(TaskNodeStatus::Running)
            .with_dependencies(vec!["dep-1".to_string()]);
        store.save_node("conv-1", &node).await.unwrap();

        let nodes = store.load_nodes("conv-1").await.unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].id, "node-1");
        assert!(matches!(nodes[0].status, TaskNodeStatus::Running));

        store
            .update_status("conv-1", "node-1", TaskNodeStatus::Success)
            .await
            .unwrap();
        let nodes = store.load_nodes("conv-1").await.unwrap();
        assert!(matches!(nodes[0].status, TaskNodeStatus::Success));

        let checkpoint = AgentCheckpoint {
            conversation_id: "conv-1".to_string(),
            messages_json: "[]".to_string(),
            current_plan: Some("plan".to_string()),
            active_skills: vec!["coding".to_string()],
            blocked_reason: None,
            working_dir: None,
            timestamp: Utc::now(),
        };
        store.save_checkpoint(&checkpoint).await.unwrap();
        let cp = store.get_checkpoint("conv-1").await.unwrap().unwrap();
        assert_eq!(cp.active_skills, vec!["coding"]);

        // update_status on a missing node is a no-op (matches SQL).
        store
            .update_status("conv-1", "nope", TaskNodeStatus::Failed)
            .await
            .unwrap();
        let nodes = store.load_nodes("conv-1").await.unwrap();
        assert_eq!(nodes.len(), 1);

        store.clear_conversation("conv-1").await.unwrap();
        assert!(store.load_nodes("conv-1").await.unwrap().is_empty());
        assert!(store.get_checkpoint("conv-1").await.unwrap().is_none());

        // clear on a never-existing conversation is a no-op.
        store.clear_conversation("never").await.unwrap();

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn corrupt_nodes_file_surfaces_as_error() {
        let tmp = std::env::temp_dir().join(format!(
            "echo-file-runtime-state-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let store = FileRuntimeStateStore::new(&tmp).unwrap();
        // Seed a corrupt nodes.json on disk.
        std::fs::create_dir_all(tmp.join("runtime_state").join("c1")).unwrap();
        std::fs::write(
            tmp.join("runtime_state").join("c1").join("nodes.json"),
            b"{ not valid json",
        )
        .unwrap();
        let err = store.load_nodes("c1").await.unwrap_err();
        assert!(err.to_string().contains("parse"));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn path_traversal_conversation_id_is_rejected() {
        let tmp = std::env::temp_dir().join(format!(
            "echo-file-runtime-state-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let store = FileRuntimeStateStore::new(&tmp).unwrap();
        let err = store.load_nodes("../escape").await.unwrap_err();
        assert!(err.to_string().contains("path segment") || err.to_string().contains("unsafe"));
        // No directory was created outside base.
        assert!(!tmp.parent().unwrap().join("escape").exists());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
