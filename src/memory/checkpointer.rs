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
//! use echo_agent::memory::checkpointer::{FileCheckpointer, Checkpointer};
//! use echo_agent::prelude::ReactAgent;
//! use std::sync::Arc;
//!
//! # async fn example() -> echo_agent::error::Result<()> {
//! let cp = Arc::new(FileCheckpointer::new("~/.echo-agent/checkpoints.json")?);
//! let mut agent = ReactAgent::new(config);
//! agent.set_checkpointer(cp, "alice-session-1".to_string());
//! // execute() 自动恢复上次对话，结束后自动保存
//! # Ok(())
//! # }
//! ```

use crate::error::{MemoryError, Result};
use crate::llm::types::Message;
use async_trait::async_trait;
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
#[async_trait]
pub trait Checkpointer: Send + Sync {
    /// 保存当前会话的消息历史，返回新快照 ID
    async fn put(&self, session_id: &str, messages: Vec<Message>) -> Result<String>;

    /// 获取指定会话的最新快照（若不存在返回 `None`）
    async fn get(&self, session_id: &str) -> Result<Option<Checkpoint>>;

    /// 获取指定会话的全部历史快照（时间倒序）
    async fn list(&self, session_id: &str) -> Result<Vec<Checkpoint>>;

    /// 删除指定会话的所有快照
    async fn delete_session(&self, session_id: &str) -> Result<()>;

    /// 列出所有已存在的 session_id
    async fn list_sessions(&self) -> Result<Vec<String>>;
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
}

#[async_trait]
impl Checkpointer for InMemoryCheckpointer {
    async fn put(&self, session_id: &str, messages: Vec<Message>) -> Result<String> {
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
    }

    async fn get(&self, session_id: &str) -> Result<Option<Checkpoint>> {
        Ok(self
            .data
            .read()
            .await
            .get(session_id)
            .and_then(|v| v.last())
            .cloned())
    }

    async fn list(&self, session_id: &str) -> Result<Vec<Checkpoint>> {
        let mut checkpoints = self
            .data
            .read()
            .await
            .get(session_id)
            .cloned()
            .unwrap_or_default();
        checkpoints.reverse();
        Ok(checkpoints)
    }

    async fn delete_session(&self, session_id: &str) -> Result<()> {
        self.data.write().await.remove(session_id);
        Ok(())
    }

    async fn list_sessions(&self) -> Result<Vec<String>> {
        Ok(self.data.read().await.keys().cloned().collect())
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
            std::fs::create_dir_all(parent)
                .map_err(|e| MemoryError::IoError(format!("创建目录失败: {e}")))?;
        }
        let data: HashMap<String, Vec<Checkpoint>> = if path.exists() {
            let raw = std::fs::read_to_string(&path)
                .map_err(|e| MemoryError::IoError(format!("读取 checkpoint 文件失败: {e}")))?;
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
            .map_err(|e| MemoryError::IoError(format!("写入 checkpoint 文件失败: {e}")))?;
        debug!(path = %self.path.display(), "💾 Checkpoint 已持久化");
        Ok(())
    }
}

#[async_trait]
impl Checkpointer for FileCheckpointer {
    async fn put(&self, session_id: &str, messages: Vec<Message>) -> Result<String> {
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
    }

    async fn get(&self, session_id: &str) -> Result<Option<Checkpoint>> {
        Ok(self
            .data
            .read()
            .await
            .get(session_id)
            .and_then(|v| v.last())
            .cloned())
    }

    async fn list(&self, session_id: &str) -> Result<Vec<Checkpoint>> {
        let mut checkpoints = self
            .data
            .read()
            .await
            .get(session_id)
            .cloned()
            .unwrap_or_default();
        checkpoints.reverse();
        Ok(checkpoints)
    }

    async fn delete_session(&self, session_id: &str) -> Result<()> {
        {
            self.data.write().await.remove(session_id);
        }
        self.flush().await?;
        info!(session_id = %session_id, "🗑️ 会话 Checkpoint 已删除");
        Ok(())
    }

    async fn list_sessions(&self) -> Result<Vec<String>> {
        Ok(self.data.read().await.keys().cloned().collect())
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

fn expand_tilde(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if s.starts_with("~/") {
        if let Some(home) = std::env::var("HOME")
            .ok()
            .or_else(|| std::env::var("USERPROFILE").ok())
        {
            return PathBuf::from(home).join(&s[2..]);
        }
    }
    path.to_path_buf()
}
