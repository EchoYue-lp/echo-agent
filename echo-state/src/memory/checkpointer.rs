//! Short-term thread state persistence — concrete implementations
//!
//! Trait definition and data types live in [`echo_core::memory::checkpointer`].
//! This module provides [`InMemoryCheckpointer`] and [`FileCheckpointer`].

use crate::util::expand_tilde;
use echo_core::error::{MemoryError, Result};
use echo_core::llm::types::Message;
pub use echo_core::memory::checkpointer::{Checkpoint, Checkpointer, ThreadState};
use futures::future::BoxFuture;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::AsyncWriteExt;
use tokio::sync::RwLock;
use tracing::{debug, info};

// ── InMemoryCheckpointer ──────────────────────────────────────────────────────

/// In-process memory Checkpointer, state is lost on restart, suitable for tests
pub struct InMemoryCheckpointer {
    data: RwLock<HashMap<String, Vec<Checkpoint>>>,
}

impl Default for InMemoryCheckpointer {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryCheckpointer {
    pub fn new() -> Self {
        Self {
            data: RwLock::new(HashMap::new()),
        }
    }

    /// List checkpoints with pagination (offset + limit).
    pub async fn list_with_limit(
        &self,
        session_id: &str,
        offset: usize,
        limit: usize,
    ) -> Vec<Checkpoint> {
        let mut checkpoints = self
            .data
            .read()
            .await
            .get(session_id)
            .cloned()
            .unwrap_or_default();
        checkpoints.reverse();
        checkpoints.into_iter().skip(offset).take(limit).collect()
    }

    /// Clean up snapshots older than `days` days.
    pub async fn cleanup_old(&self, days: u64) -> usize {
        let cutoff = now_secs().saturating_sub(days * 86_400);
        let mut data = self.data.write().await;
        let mut removed = 0;
        for checkpoints in data.values_mut() {
            let before = checkpoints.len();
            checkpoints.retain(|cp| cp.created_at >= cutoff);
            removed += before - checkpoints.len();
        }
        removed
    }
}

impl Checkpointer for InMemoryCheckpointer {
    fn put<'a>(
        &'a self,
        session_id: &'a str,
        messages: Vec<Message>,
    ) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            let checkpoint_id = new_checkpoint_id();
            let checkpoint = Checkpoint {
                session_id: session_id.to_string(),
                checkpoint_id: checkpoint_id.clone(),
                messages,
                parent_checkpoint_id: None,
                summary: None,
                metadata: None,
                created_at: now_secs(),
            };
            self.data
                .write()
                .await
                .entry(session_id.to_string())
                .or_default()
                .push(checkpoint);
            Ok(checkpoint_id)
        })
    }

    fn get<'a>(&'a self, session_id: &'a str) -> BoxFuture<'a, Result<Option<Checkpoint>>> {
        Box::pin(async move {
            Ok(self
                .data
                .read()
                .await
                .get(session_id)
                .and_then(|v| v.last())
                .cloned())
        })
    }

    fn list<'a>(&'a self, session_id: &'a str) -> BoxFuture<'a, Result<Vec<Checkpoint>>> {
        Box::pin(async move {
            let mut checkpoints = self
                .data
                .read()
                .await
                .get(session_id)
                .cloned()
                .unwrap_or_default();
            checkpoints.reverse();
            Ok(checkpoints)
        })
    }

    fn delete_session<'a>(&'a self, session_id: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.data.write().await.remove(session_id);
            Ok(())
        })
    }

    fn list_sessions(&self) -> BoxFuture<'_, Result<Vec<String>>> {
        Box::pin(async move { Ok(self.data.read().await.keys().cloned().collect()) })
    }

    fn put_state<'a>(
        &'a self,
        session_id: &'a str,
        state: ThreadState,
    ) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            let checkpoint_id = new_checkpoint_id();
            let checkpoint = Checkpoint {
                session_id: session_id.to_string(),
                checkpoint_id: checkpoint_id.clone(),
                messages: state.messages,
                parent_checkpoint_id: None,
                summary: state.summary,
                metadata: state.metadata,
                created_at: now_secs(),
            };
            self.data
                .write()
                .await
                .entry(session_id.to_string())
                .or_default()
                .push(checkpoint);
            Ok(checkpoint_id)
        })
    }
}

// ── FileCheckpointer ──────────────────────────────────────────────────────────

/// JSON file-based persistent Checkpointer
pub struct FileCheckpointer {
    path: PathBuf,
    data: RwLock<HashMap<String, Vec<Checkpoint>>>,
}

impl FileCheckpointer {
    /// Open or create the Checkpointer file, auto-create parent directories
    pub fn new(path: impl AsRef<Path>) -> Result<Self> {
        let path = expand_tilde(path.as_ref());
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| MemoryError::IoError(e.to_string()))?;
        }
        let data: HashMap<String, Vec<Checkpoint>> = if path.exists() {
            let raw =
                std::fs::read_to_string(&path).map_err(|e| MemoryError::IoError(e.to_string()))?;
            serde_json::from_str(&raw).unwrap_or_else(|e| {
                tracing::warn!("Checkpoint file parse failed, starting from empty state: {e}");
                HashMap::new()
            })
        } else {
            HashMap::new()
        };
        let session_count = data.len();
        info!(path = %path.display(), sessions = session_count, "FileCheckpointer initialized");
        Ok(Self {
            path,
            data: RwLock::new(data),
        })
    }

    async fn flush(&self) -> Result<()> {
        let data = self.data.read().await;
        let json = serde_json::to_string_pretty(&*data)
            .map_err(|e| MemoryError::SerializationError(e.to_string()))?;
        let tmp_path = self.path.with_extension(format!(
            "{}.tmp",
            self.path
                .extension()
                .and_then(|ext| ext.to_str())
                .unwrap_or("json")
        ));
        let mut file = tokio::fs::File::create(&tmp_path)
            .await
            .map_err(|e| MemoryError::IoError(e.to_string()))?;
        file.write_all(json.as_bytes())
            .await
            .map_err(|e| MemoryError::IoError(e.to_string()))?;
        file.sync_all()
            .await
            .map_err(|e| MemoryError::IoError(e.to_string()))?;
        drop(file);

        if let Err(e) = tokio::fs::rename(&tmp_path, &self.path).await {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return Err(MemoryError::IoError(e.to_string()).into());
        }
        debug!(path = %self.path.display(), "Checkpoint persisted");
        Ok(())
    }

    /// List checkpoints with pagination (offset + limit).
    pub async fn list_with_limit(
        &self,
        session_id: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<Checkpoint>> {
        let mut checkpoints = self
            .data
            .read()
            .await
            .get(session_id)
            .cloned()
            .unwrap_or_default();
        checkpoints.reverse();
        Ok(checkpoints.into_iter().skip(offset).take(limit).collect())
    }

    /// Clean up snapshots older than `days` days.
    pub async fn cleanup_old(&self, days: u64) -> Result<usize> {
        let cutoff = now_secs().saturating_sub(days * 86_400);
        let mut removed = 0;
        {
            let mut data = self.data.write().await;
            for checkpoints in data.values_mut() {
                let before = checkpoints.len();
                checkpoints.retain(|cp| cp.created_at >= cutoff);
                removed += before - checkpoints.len();
            }
        }
        if removed > 0 {
            self.flush().await?;
        }
        Ok(removed)
    }
}

impl Checkpointer for FileCheckpointer {
    fn put<'a>(
        &'a self,
        session_id: &'a str,
        messages: Vec<Message>,
    ) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            let checkpoint_id = new_checkpoint_id();
            let checkpoint = Checkpoint {
                session_id: session_id.to_string(),
                checkpoint_id: checkpoint_id.clone(),
                messages,
                parent_checkpoint_id: None,
                summary: None,
                metadata: None,
                created_at: now_secs(),
            };
            info!(session_id = %session_id, checkpoint_id = %checkpoint_id, "Saving checkpoint");
            {
                let mut data = self.data.write().await;
                data.entry(session_id.to_string())
                    .or_default()
                    .push(checkpoint);
            }
            self.flush().await?;
            Ok(checkpoint_id)
        })
    }

    fn get<'a>(&'a self, session_id: &'a str) -> BoxFuture<'a, Result<Option<Checkpoint>>> {
        Box::pin(async move {
            Ok(self
                .data
                .read()
                .await
                .get(session_id)
                .and_then(|v| v.last())
                .cloned())
        })
    }

    fn list<'a>(&'a self, session_id: &'a str) -> BoxFuture<'a, Result<Vec<Checkpoint>>> {
        Box::pin(async move {
            let mut checkpoints = self
                .data
                .read()
                .await
                .get(session_id)
                .cloned()
                .unwrap_or_default();
            checkpoints.reverse();
            Ok(checkpoints)
        })
    }

    fn delete_session<'a>(&'a self, session_id: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            {
                self.data.write().await.remove(session_id);
            }
            self.flush().await?;
            info!(session_id = %session_id, "Session checkpoint deleted");
            Ok(())
        })
    }

    fn list_sessions(&self) -> BoxFuture<'_, Result<Vec<String>>> {
        Box::pin(async move { Ok(self.data.read().await.keys().cloned().collect()) })
    }

    fn put_state<'a>(
        &'a self,
        session_id: &'a str,
        state: ThreadState,
    ) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            let checkpoint_id = new_checkpoint_id();
            let checkpoint = Checkpoint {
                session_id: session_id.to_string(),
                checkpoint_id: checkpoint_id.clone(),
                messages: state.messages,
                parent_checkpoint_id: None,
                summary: state.summary,
                metadata: state.metadata,
                created_at: now_secs(),
            };
            info!(session_id = %session_id, checkpoint_id = %checkpoint_id, "Saving thread state");
            {
                let mut data = self.data.write().await;
                data.entry(session_id.to_string())
                    .or_default()
                    .push(checkpoint);
            }
            self.flush().await?;
            Ok(checkpoint_id)
        })
    }
}

// ── Private utility functions ──────────────────────────────────────────────────────

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn new_checkpoint_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

// ── Unit tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[tokio::test]
    async fn test_in_memory_checkpointer_put_and_get() {
        let checkpointer = InMemoryCheckpointer::new();

        let messages = vec![
            Message::system("You are a helper".to_string()),
            Message::user("Hello".to_string()),
        ];

        let checkpoint_id = checkpointer
            .put("session1", messages.clone())
            .await
            .unwrap();
        assert!(!checkpoint_id.is_empty());

        let checkpoint = checkpointer.get("session1").await.unwrap();
        assert!(checkpoint.is_some());
        let cp = checkpoint.unwrap();
        assert_eq!(cp.messages.len(), 2);
        assert_eq!(cp.session_id, "session1");
    }

    #[tokio::test]
    async fn test_in_memory_checkpointer_get_nonexistent() {
        let checkpointer = InMemoryCheckpointer::new();

        let checkpoint = checkpointer.get("nonexistent").await.unwrap();
        assert!(checkpoint.is_none());
    }

    #[tokio::test]
    async fn test_in_memory_checkpointer_list() {
        let checkpointer = InMemoryCheckpointer::new();

        checkpointer
            .put("session1", vec![Message::user("m1".to_string())])
            .await
            .unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        checkpointer
            .put("session1", vec![Message::user("m2".to_string())])
            .await
            .unwrap();

        let checkpoints = checkpointer.list("session1").await.unwrap();
        assert_eq!(checkpoints.len(), 2);
        assert_eq!(checkpoints[0].messages[0].content.as_text_ref(), Some("m2"));
    }

    #[tokio::test]
    async fn test_in_memory_checkpointer_delete_session() {
        let checkpointer = InMemoryCheckpointer::new();

        checkpointer
            .put("session1", vec![Message::user("msg".to_string())])
            .await
            .unwrap();
        checkpointer.delete_session("session1").await.unwrap();

        let checkpoint = checkpointer.get("session1").await.unwrap();
        assert!(checkpoint.is_none());
    }

    #[tokio::test]
    async fn test_in_memory_checkpointer_list_sessions() {
        let checkpointer = InMemoryCheckpointer::new();

        checkpointer.put("session1", vec![]).await.unwrap();
        checkpointer.put("session2", vec![]).await.unwrap();
        checkpointer.put("session3", vec![]).await.unwrap();

        let sessions = checkpointer.list_sessions().await.unwrap();
        assert_eq!(sessions.len(), 3);
        assert!(sessions.contains(&"session1".to_string()));
    }

    #[tokio::test]
    async fn test_in_memory_checkpointer_multiple_sessions() {
        let checkpointer = InMemoryCheckpointer::new();

        checkpointer
            .put("session1", vec![Message::user("s1-msg".to_string())])
            .await
            .unwrap();
        checkpointer
            .put("session2", vec![Message::user("s2-msg".to_string())])
            .await
            .unwrap();

        let cp1 = checkpointer.get("session1").await.unwrap().unwrap();
        let cp2 = checkpointer.get("session2").await.unwrap().unwrap();

        assert_eq!(cp1.messages[0].content.as_text_ref(), Some("s1-msg"));
        assert_eq!(cp2.messages[0].content.as_text_ref(), Some("s2-msg"));
    }

    #[test]
    fn test_checkpoint_structure() {
        let checkpoint = Checkpoint {
            session_id: "test-session".to_string(),
            checkpoint_id: "cp-123".to_string(),
            messages: vec![Message::user("test".to_string())],
            parent_checkpoint_id: None,
            summary: None,
            metadata: None,
            created_at: 1234567890,
        };

        assert_eq!(checkpoint.session_id, "test-session");
        assert_eq!(checkpoint.checkpoint_id, "cp-123");
        assert_eq!(checkpoint.messages.len(), 1);
        assert_eq!(checkpoint.created_at, 1234567890);
    }

    #[tokio::test]
    async fn test_file_checkpointer_flush_is_atomicish() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("echo-checkpointer-{unique}.json"));
        let tmp_path = path.with_extension("json.tmp");

        let checkpointer = FileCheckpointer::new(&path).unwrap();
        checkpointer
            .put("session1", vec![Message::user("persist me".to_string())])
            .await
            .unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("persist me"));
        assert!(!tmp_path.exists(), "temporary file should be cleaned up");

        let _ = std::fs::remove_file(&path);
    }
}
