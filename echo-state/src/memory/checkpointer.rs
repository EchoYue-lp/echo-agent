//! 短期会话记忆持久化（Checkpointer）
//!
//! 按 `session_id` 将对话历史序列化到存储后端，支持跨进程恢复同一会话。
//!
//! ## 内置实现
//!
//! | 类型 | 说明 |
//! |------|------|
//! | [`InMemoryCheckpointer`] | 进程内存，重启即清空，适合测试 |
//! | [`FileCheckpointer`] | JSON 文件持久化，适合本地单机场景 |
//!
//! ## 快速上手
//!
//! ```rust,no_run
//! use echo_core::error::Result;
//! use echo_state::memory::checkpointer::FileCheckpointer;
//! use std::sync::Arc;
//!
//! # async fn example() -> Result<()> {
//! let cp = Arc::new(FileCheckpointer::new("~/.echo-agent/checkpoints.json")?);
//! // 将 `cp` 接入你自己的 agent/runtime 层，或通过 `echo_agent` façade 使用。
//! let _ = cp;
//! # Ok(())
//! # }
//! ```

use crate::util::expand_tilde;
use echo_core::error::{MemoryError, Result};
use echo_core::llm::types::Message;
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use tracing::{debug, info};

// ── Checkpoint ────────────────────────────────────────────────────────────────

/// 单次对话状态快照
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    /// 所属会话标识
    pub session_id: String,
    /// 快照唯一 ID（UUID v4）
    pub checkpoint_id: String,
    /// 该时刻的完整消息历史
    pub messages: Vec<Message>,
    /// 创建时间（Unix 秒）
    pub created_at: u64,
}

// ── Checkpointer trait ────────────────────────────────────────────────────────

/// 短期会话记忆的持久化接口
///
/// 实现方可替换为任意存储后端（内存、文件、数据库等）。
pub trait Checkpointer: Send + Sync {
    /// 保存当前会话的消息历史，返回新快照 ID
    fn put<'a>(
        &'a self,
        session_id: &'a str,
        messages: Vec<Message>,
    ) -> BoxFuture<'a, Result<String>>;

    /// 获取指定会话的最新快照（若不存在返回 `None`）
    fn get<'a>(&'a self, session_id: &'a str) -> BoxFuture<'a, Result<Option<Checkpoint>>>;

    /// 获取指定会话的全部历史快照（时间倒序）
    fn list<'a>(&'a self, session_id: &'a str) -> BoxFuture<'a, Result<Vec<Checkpoint>>>;

    /// 删除指定会话的所有快照
    fn delete_session<'a>(&'a self, session_id: &'a str) -> BoxFuture<'a, Result<()>>;

    /// 列出所有已存在的 session_id
    fn list_sessions(&self) -> BoxFuture<'_, Result<Vec<String>>>;
}

// ── InMemoryCheckpointer ──────────────────────────────────────────────────────

/// 进程内存 Checkpointer，重启后状态丢失，适合测试
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

    /// 清理超过 `days` 天的旧快照，释放内存。
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
}

// ── FileCheckpointer ──────────────────────────────────────────────────────────

/// 基于 JSON 文件的持久化 Checkpointer
///
/// 写时立即落盘，读时从内存缓存返回（无需反复解析文件）。
///
/// 存储格式（每个 key 为 `session_id`）：
/// ```json
/// {
///   "alice-session-1": [
///     { "session_id": "alice-session-1", "checkpoint_id": "...", "messages": [...], "created_at": 123 }
///   ]
/// }
/// ```
pub struct FileCheckpointer {
    path: PathBuf,
    data: RwLock<HashMap<String, Vec<Checkpoint>>>,
}

impl FileCheckpointer {
    /// 打开或创建 Checkpointer 文件，自动建父目录
    pub fn new(path: impl AsRef<Path>) -> Result<Self> {
        let path = expand_tilde(path.as_ref());
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| MemoryError::IoError(e.to_string()))?;
        }
        let data: HashMap<String, Vec<Checkpoint>> = if path.exists() {
            let raw =
                std::fs::read_to_string(&path).map_err(|e| MemoryError::IoError(e.to_string()))?;
            serde_json::from_str(&raw).unwrap_or_else(|e| {
                tracing::warn!("Checkpoint 文件解析失败，从空状态开始: {e}");
                HashMap::new()
            })
        } else {
            HashMap::new()
        };
        let session_count = data.len();
        info!(path = %path.display(), sessions = session_count, "🗂️ FileCheckpointer 初始化");
        Ok(Self {
            path,
            data: RwLock::new(data),
        })
    }

    async fn flush(&self) -> Result<()> {
        let data = self.data.read().await;
        let json = serde_json::to_string_pretty(&*data)
            .map_err(|e| MemoryError::SerializationError(e.to_string()))?;
        tokio::fs::write(&self.path, json)
            .await
            .map_err(|e| MemoryError::IoError(e.to_string()))?;
        debug!(path = %self.path.display(), "💾 Checkpoint 已持久化");
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

    /// 清理超过 `days` 天的旧快照，释放内存并刷盘。
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
                created_at: now_secs(),
            };
            info!(session_id = %session_id, checkpoint_id = %checkpoint_id, "🔖 保存 Checkpoint");
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
            info!(session_id = %session_id, "🗑️ 会话 Checkpoint 已删除");
            Ok(())
        })
    }

    fn list_sessions(&self) -> BoxFuture<'_, Result<Vec<String>>> {
        Box::pin(async move { Ok(self.data.read().await.keys().cloned().collect()) })
    }
}

// ── 私有工具函数 ──────────────────────────────────────────────────────────────

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn new_checkpoint_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

// ── 单元测试 ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

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
        // 应该是倒序（最新的在前）
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
            created_at: 1234567890,
        };

        assert_eq!(checkpoint.session_id, "test-session");
        assert_eq!(checkpoint.checkpoint_id, "cp-123");
        assert_eq!(checkpoint.messages.len(), 1);
        assert_eq!(checkpoint.created_at, 1234567890);
    }
}
