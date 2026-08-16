//! File-backed [`RuntimeStateStore`] — a no-dependency JSON-file backend.
//!
//! One directory per conversation under `<base>/runtime_state/<safe_id>/`:
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
//! - **Corrupt JSON is an error.** A malformed `checkpoint.json` surfaces as
//!   `ReactError::Other` rather than silently returning `None`.
//! - **Unique temp names.** Each atomic write uses a uuid-suffixed temp file
//!   (no cross-write collisions; multi-process belt-and-suspenders).
//!
//! Atomic writes use tmp + fsync + rename so a crash never leaves a
//! half-written file.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use echo_core::error::ReactError;
use futures::future::BoxFuture;

use super::{AgentCheckpoint, RuntimeStateStore};

/// File-backed runtime state store.
pub struct FileRuntimeStateStore {
    base: PathBuf,
    /// Serializes checkpoint reads, writes, and deletion.
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

    fn checkpoint_path(&self, conversation_id: &str) -> Result<PathBuf, ReactError> {
        Ok(self.conv_dir(conversation_id)?.join("checkpoint.json"))
    }

    fn to_react_err(e: impl std::fmt::Display) -> ReactError {
        ReactError::Other(format!("FileRuntimeStateStore: {e}"))
    }
}

impl RuntimeStateStore for FileRuntimeStateStore {
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
            echo_core::utils::fs::atomic_write(&path, json.as_bytes())
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
    use chrono::Utc;

    #[tokio::test]
    async fn file_runtime_state_lifecycle() -> crate::error::Result<()> {
        let tmp = std::env::temp_dir().join(format!(
            "echo-file-runtime-state-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let store = FileRuntimeStateStore::new(&tmp)?;

        let checkpoint = AgentCheckpoint {
            conversation_id: "conv-1".to_string(),
            messages_json: "[]".to_string(),
            current_plan: Some("plan".to_string()),
            active_skills: vec!["coding".to_string()],
            blocked_reason: None,
            working_dir: None,
            timestamp: Utc::now(),
        };
        store.save_checkpoint(&checkpoint).await?;
        let cp = store
            .get_checkpoint("conv-1")
            .await?
            .ok_or_else(|| ReactError::Other("checkpoint missing after save".to_string()))?;
        assert_eq!(cp.active_skills, vec!["coding"]);

        store.clear_conversation("conv-1").await?;
        assert!(store.get_checkpoint("conv-1").await?.is_none());

        // clear on a never-existing conversation is a no-op.
        store.clear_conversation("never").await?;

        let _ = std::fs::remove_dir_all(&tmp);
        Ok(())
    }

    #[tokio::test]
    async fn corrupt_checkpoint_surfaces_as_error() -> crate::error::Result<()> {
        let tmp = std::env::temp_dir().join(format!(
            "echo-file-runtime-state-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let store = FileRuntimeStateStore::new(&tmp)?;
        std::fs::create_dir_all(tmp.join("runtime_state").join("c1"))
            .map_err(|error| ReactError::Other(error.to_string()))?;
        std::fs::write(
            tmp.join("runtime_state").join("c1").join("checkpoint.json"),
            b"{ not valid json",
        )
        .map_err(|error| ReactError::Other(error.to_string()))?;
        let err = store
            .get_checkpoint("c1")
            .await
            .err()
            .ok_or_else(|| ReactError::Other("corrupt checkpoint was accepted".to_string()))?;
        assert!(err.to_string().contains("parse"));
        let _ = std::fs::remove_dir_all(&tmp);
        Ok(())
    }

    #[tokio::test]
    async fn path_traversal_conversation_id_is_rejected() -> crate::error::Result<()> {
        let tmp = std::env::temp_dir().join(format!(
            "echo-file-runtime-state-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let store = FileRuntimeStateStore::new(&tmp)?;
        let err = store
            .get_checkpoint("../escape")
            .await
            .err()
            .ok_or_else(|| ReactError::Other("unsafe conversation id was accepted".to_string()))?;
        assert!(err.to_string().contains("path segment") || err.to_string().contains("unsafe"));
        // No directory was created outside base.
        let parent = tmp
            .parent()
            .ok_or_else(|| ReactError::Other("temporary path has no parent".to_string()))?;
        assert!(!parent.join("escape").exists());
        let _ = std::fs::remove_dir_all(&tmp);
        Ok(())
    }
}
