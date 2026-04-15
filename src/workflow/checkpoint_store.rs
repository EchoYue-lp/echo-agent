//! Checkpoint 持久化存储
//!
//! 支持 Graph 执行状态的保存和恢复，实现 LangGraph 风格的 interrupt/checkpoint 机制。
//!
//! ## 设计
//!
//! - `CheckpointStore` trait: 抽象存储接口
//! - `MemoryCheckpointStore`: 内存存储（默认，不持久化）
//! - `FileCheckpointStore`: 文件存储（支持持久化）
//!
//! ## 使用场景
//!
//! - 长时间运行的工作流暂停/恢复
//! - 需要人工审批的节点 interrupt
//! - 故障恢复和重试

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::sync::RwLock;

use crate::error::Result;
use crate::workflow::SharedState;

// ── Checkpoint ────────────────────────────────────────────────────────────────

/// Checkpoint - 图执行状态的快照
///
/// 包含恢复执行所需的所有信息：
/// - 执行路径和步数
/// - 共享状态快照
/// - 待处理的节点信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    /// 唯一标识符
    pub id: String,
    /// 图名称
    pub graph_name: String,
    /// 当前节点
    pub current_node: String,
    /// 状态快照（JSON 序列化）
    pub state_snapshot: serde_json::Value,
    /// 执行路径
    pub path: Vec<String>,
    /// 执行步数
    pub step_count: usize,
    /// 创建时间
    pub created_at: DateTime<Utc>,
    /// 待处理的工具调用（如果有）
    pub pending_action: Option<serde_json::Value>,
    /// Interrupt 类型
    pub interrupt_type: InterruptType,
}

impl Checkpoint {
    /// 创建新的 Checkpoint
    pub fn new(
        graph_name: String,
        current_node: String,
        state: &SharedState,
        path: Vec<String>,
        step_count: usize,
        interrupt_type: InterruptType,
    ) -> Self {
        let id = uuid::Uuid::new_v4().to_string();
        let state_snapshot = state.to_json_value().unwrap_or_default();

        Self {
            id,
            graph_name,
            current_node,
            state_snapshot,
            path,
            step_count,
            created_at: Utc::now(),
            pending_action: None,
            interrupt_type,
        }
    }

    /// 从 Checkpoint 恢复 SharedState
    pub fn restore_state(&self) -> Result<SharedState> {
        SharedState::from_json(&self.state_snapshot)
            .map_err(|e| crate::error::ReactError::Other(format!("Failed to restore state: {}", e)))
    }
}

/// Interrupt 类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InterruptType {
    /// 进入节点前暂停
    BeforeNode,
    /// 节点执行后暂停
    AfterNode,
    /// 工具审批暂停
    ToolApproval,
    /// 用户请求暂停
    UserRequest,
}

/// Checkpoint 信息摘要（用于列表显示）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointInfo {
    pub id: String,
    pub graph_name: String,
    pub current_node: String,
    pub step_count: usize,
    pub created_at: DateTime<Utc>,
    pub interrupt_type: InterruptType,
}

impl From<&Checkpoint> for CheckpointInfo {
    fn from(cp: &Checkpoint) -> Self {
        Self {
            id: cp.id.clone(),
            graph_name: cp.graph_name.clone(),
            current_node: cp.current_node.clone(),
            step_count: cp.step_count,
            created_at: cp.created_at,
            interrupt_type: cp.interrupt_type,
        }
    }
}

// ── CheckpointStore Trait ──────────────────────────────────────────────────────

/// Checkpoint 存储接口
#[async_trait]
pub trait CheckpointStore: Send + Sync {
    /// 保存 Checkpoint
    async fn save(&self, checkpoint: &Checkpoint) -> Result<()>;

    /// 加载 Checkpoint
    async fn load(&self, id: &str) -> Result<Option<Checkpoint>>;

    /// 列出所有 Checkpoint 信息
    async fn list(&self) -> Result<Vec<CheckpointInfo>>;

    /// 删除 Checkpoint
    async fn delete(&self, id: &str) -> Result<()>;

    /// 清空所有 Checkpoint
    async fn clear(&self) -> Result<()>;
}

// ── Memory Checkpoint Store ────────────────────────────────────────────────────

/// 内存存储实现（默认，不持久化）
pub struct MemoryCheckpointStore {
    checkpoints: RwLock<HashMap<String, Checkpoint>>,
}

impl MemoryCheckpointStore {
    pub fn new() -> Self {
        Self {
            checkpoints: RwLock::new(HashMap::new()),
        }
    }
}

impl Default for MemoryCheckpointStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CheckpointStore for MemoryCheckpointStore {
    async fn save(&self, checkpoint: &Checkpoint) -> Result<()> {
        let mut checkpoints = self.checkpoints.write().await;
        checkpoints.insert(checkpoint.id.clone(), checkpoint.clone());
        Ok(())
    }

    async fn load(&self, id: &str) -> Result<Option<Checkpoint>> {
        let checkpoints = self.checkpoints.read().await;
        Ok(checkpoints.get(id).cloned())
    }

    async fn list(&self) -> Result<Vec<CheckpointInfo>> {
        let checkpoints = self.checkpoints.read().await;
        Ok(checkpoints.values().map(CheckpointInfo::from).collect())
    }

    async fn delete(&self, id: &str) -> Result<()> {
        let mut checkpoints = self.checkpoints.write().await;
        checkpoints.remove(id);
        Ok(())
    }

    async fn clear(&self) -> Result<()> {
        let mut checkpoints = self.checkpoints.write().await;
        checkpoints.clear();
        Ok(())
    }
}

// ── File Checkpoint Store ──────────────────────────────────────────────────────

/// 文件存储实现（支持持久化）
pub struct FileCheckpointStore {
    base_path: PathBuf,
}

impl FileCheckpointStore {
    pub fn new<P: Into<PathBuf>>(base_path: P) -> Self {
        Self {
            base_path: base_path.into(),
        }
    }

    fn checkpoint_path(&self, id: &str) -> PathBuf {
        self.base_path.join(format!("{}.json", id))
    }

    fn ensure_dir_exists(&self) -> Result<()> {
        if !self.base_path.exists() {
            std::fs::create_dir_all(&self.base_path).map_err(|e| {
                crate::error::ReactError::Other(format!("Failed to create checkpoint dir: {}", e))
            })?;
        }
        Ok(())
    }
}

#[async_trait]
impl CheckpointStore for FileCheckpointStore {
    async fn save(&self, checkpoint: &Checkpoint) -> Result<()> {
        self.ensure_dir_exists()?;
        let path = self.checkpoint_path(&checkpoint.id);
        let json = serde_json::to_string_pretty(checkpoint).map_err(|e| {
            crate::error::ReactError::Other(format!("Failed to serialize checkpoint: {}", e))
        })?;
        // 原子写入：先写临时文件再 rename，避免崩溃时损坏
        let tmp_path = path.with_extension("json.tmp");
        tokio::fs::write(&tmp_path, &json).await.map_err(|e| {
            crate::error::ReactError::Other(format!("Failed to write temp checkpoint: {}", e))
        })?;
        tokio::fs::rename(&tmp_path, &path).await.map_err(|e| {
            crate::error::ReactError::Other(format!("Failed to rename checkpoint file: {}", e))
        })?;
        Ok(())
    }

    async fn load(&self, id: &str) -> Result<Option<Checkpoint>> {
        let path = self.checkpoint_path(id);
        if !path.exists() {
            return Ok(None);
        }
        let json = tokio::fs::read_to_string(path).await.map_err(|e| {
            crate::error::ReactError::Other(format!("Failed to read checkpoint: {}", e))
        })?;
        let checkpoint: Checkpoint = serde_json::from_str(&json).map_err(|e| {
            crate::error::ReactError::Other(format!("Failed to parse checkpoint: {}", e))
        })?;
        Ok(Some(checkpoint))
    }

    async fn list(&self) -> Result<Vec<CheckpointInfo>> {
        self.ensure_dir_exists()?;
        let mut entries = tokio::fs::read_dir(&self.base_path).await.map_err(|e| {
            crate::error::ReactError::Other(format!("Failed to read checkpoint dir: {}", e))
        })?;

        let mut infos = Vec::new();
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| crate::error::ReactError::Other(format!("Failed to read entry: {}", e)))?
        {
            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false)
                && let Ok(json) = tokio::fs::read_to_string(&path).await
                && let Ok(cp) = serde_json::from_str::<Checkpoint>(&json)
            {
                infos.push(CheckpointInfo::from(&cp));
            }
        }

        // 按创建时间排序（最新在前）
        infos.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(infos)
    }

    async fn delete(&self, id: &str) -> Result<()> {
        let path = self.checkpoint_path(id);
        if path.exists() {
            tokio::fs::remove_file(path).await.map_err(|e| {
                crate::error::ReactError::Other(format!("Failed to delete checkpoint: {}", e))
            })?;
        }
        Ok(())
    }

    async fn clear(&self) -> Result<()> {
        if !self.base_path.exists() {
            return Ok(());
        }
        let mut entries = tokio::fs::read_dir(&self.base_path).await.map_err(|e| {
            crate::error::ReactError::Other(format!("Failed to read checkpoint dir: {}", e))
        })?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| crate::error::ReactError::Other(format!("Failed to read entry: {}", e)))?
        {
            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                let _ = tokio::fs::remove_file(path).await;
            }
        }
        Ok(())
    }
}

// ── 单元测试 ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_checkpoint_create() {
        let state = SharedState::new();
        state.set("test_key", "test_value").unwrap();

        let cp = Checkpoint::new(
            "test_graph".to_string(),
            "node1".to_string(),
            &state,
            vec!["start".to_string(), "node1".to_string()],
            2,
            InterruptType::BeforeNode,
        );

        assert!(!cp.id.is_empty());
        assert_eq!(cp.graph_name, "test_graph");
        assert_eq!(cp.current_node, "node1");
        assert_eq!(cp.path.len(), 2);
        assert_eq!(cp.step_count, 2);
        assert_eq!(cp.interrupt_type, InterruptType::BeforeNode);
    }

    #[test]
    fn test_checkpoint_restore_state() {
        let state = SharedState::new();
        state.set("key", "value").unwrap();

        let cp = Checkpoint::new(
            "test".to_string(),
            "node".to_string(),
            &state,
            vec![],
            0,
            InterruptType::UserRequest,
        );

        let restored = cp.restore_state().unwrap();
        assert_eq!(restored.get::<String>("key"), Some("value".to_string()));
    }

    #[tokio::test]
    async fn test_memory_store() {
        let store = MemoryCheckpointStore::new();

        let state = SharedState::new();
        state.set("x", 42).unwrap();

        let cp = Checkpoint::new(
            "graph".to_string(),
            "node".to_string(),
            &state,
            vec![],
            0,
            InterruptType::BeforeNode,
        );

        let id = cp.id.clone();

        // Save
        store.save(&cp).await.unwrap();

        // Load
        let loaded = store.load(&id).await.unwrap();
        assert!(loaded.is_some());
        let loaded = loaded.unwrap();
        assert_eq!(loaded.graph_name, "graph");

        // List
        let list = store.list().await.unwrap();
        assert_eq!(list.len(), 1);

        // Delete
        store.delete(&id).await.unwrap();
        let loaded = store.load(&id).await.unwrap();
        assert!(loaded.is_none());
    }

    #[tokio::test]
    async fn test_file_store() {
        // 使用当前目录下的临时路径
        let temp_path = std::env::temp_dir().join(format!("echo_test_{}", uuid::Uuid::new_v4()));
        let store = FileCheckpointStore::new(&temp_path);

        let state = SharedState::new();
        state.set("data", "test").unwrap();

        let cp = Checkpoint::new(
            "test_graph".to_string(),
            "node_a".to_string(),
            &state,
            vec!["start".to_string()],
            1,
            InterruptType::AfterNode,
        );

        let id = cp.id.clone();

        // Save
        store.save(&cp).await.unwrap();

        // Load
        let loaded = store.load(&id).await.unwrap();
        assert!(loaded.is_some());
        let loaded = loaded.unwrap();
        assert_eq!(loaded.graph_name, "test_graph");
        assert_eq!(loaded.current_node, "node_a");

        // List
        let list = store.list().await.unwrap();
        assert_eq!(list.len(), 1);

        // Clear
        store.clear().await.unwrap();
        let list = store.list().await.unwrap();
        assert!(list.is_empty());

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_path);
    }
}
