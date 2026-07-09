//! Checkpoint persistent storage
//!
//! Supports saving and restoring Graph execution state, implementing LangGraph-style interrupt/checkpoint mechanisms.
//!
//! ## Design
//!
//! - `CheckpointStore` trait: abstract storage interface
//! - `MemoryCheckpointStore`: in-memory storage (default, non-persistent)
//! - `FileCheckpointStore`: file storage (supports persistence)
//!
//! ## Use Cases
//!
//! - Long-running workflow pause/resume
//! - Node interrupts requiring manual approval
//! - Failure recovery and retry

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::sync::RwLock;

use super::state::SharedState;
use echo_core::error::Result;

// ── Checkpoint ────────────────────────────────────────────────────────────────

/// Checkpoint - snapshot of graph execution state
///
/// Contains all information needed to resume execution:
/// - Execution path and step count
/// - Shared state snapshot
/// - Pending node information
///
/// Supports time-travel debugging via `parent_checkpoint_id` lineage,
/// human-readable `label`/`tags`, and `branch` forking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    /// Unique identifier
    pub id: String,
    /// Graph name
    pub graph_name: String,
    /// Current node
    pub current_node: String,
    /// State snapshot (JSON serialized)
    pub state_snapshot: serde_json::Value,
    /// Execution path
    pub path: Vec<String>,
    /// Step count
    pub step_count: usize,
    /// Creation time
    #[serde(with = "echo_core::utils::time::local_rfc3339")]
    pub created_at: DateTime<Utc>,
    /// Pending tool calls (if any)
    pub pending_action: Option<serde_json::Value>,
    /// Interrupt type
    pub interrupt_type: InterruptType,
    /// Parent checkpoint ID for lineage tracking.
    /// `None` for the first checkpoint in a session.
    #[serde(default)]
    pub parent_checkpoint_id: Option<String>,
    /// Human-readable label (e.g., "before risky operation", "branch:feature-x")
    #[serde(default)]
    pub label: Option<String>,
    /// Tags for filtering and discovery
    #[serde(default)]
    pub tags: Vec<String>,
    /// Branch name if this checkpoint is a fork
    #[serde(default)]
    pub branch: Option<String>,
}

impl Checkpoint {
    /// Create a new Checkpoint
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
            parent_checkpoint_id: None,
            label: None,
            tags: Vec::new(),
            branch: None,
        }
    }

    /// Create a new Checkpoint with explicit parent for lineage tracking.
    #[allow(clippy::too_many_arguments)]
    pub fn with_parent(
        parent_id: impl Into<String>,
        graph_name: String,
        current_node: String,
        state: &SharedState,
        path: Vec<String>,
        step_count: usize,
        interrupt_type: InterruptType,
    ) -> Self {
        let mut cp = Self::new(
            graph_name,
            current_node,
            state,
            path,
            step_count,
            interrupt_type,
        );
        cp.parent_checkpoint_id = Some(parent_id.into());
        cp
    }

    /// Create a new branch checkpoint (fork off an existing checkpoint).
    #[allow(clippy::too_many_arguments)]
    pub fn branch(
        parent_id: impl Into<String>,
        branch_name: impl Into<String>,
        graph_name: String,
        current_node: String,
        state: &SharedState,
        path: Vec<String>,
        step_count: usize,
        interrupt_type: InterruptType,
    ) -> Self {
        let mut cp = Self::with_parent(
            parent_id,
            graph_name,
            current_node,
            state,
            path,
            step_count,
            interrupt_type,
        );
        cp.branch = Some(branch_name.into());
        cp
    }

    /// Set a human-readable label on this checkpoint.
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Add tags to this checkpoint.
    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }

    /// Restore SharedState from a Checkpoint
    pub fn restore_state(&self) -> Result<SharedState> {
        SharedState::from_json(&self.state_snapshot).map_err(|e| {
            echo_core::error::ReactError::Other(format!("Failed to restore state: {}", e))
        })
    }
}

/// Interrupt type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InterruptType {
    /// Pause before entering a node
    BeforeNode,
    /// Pause after node execution
    AfterNode,
    /// Pause for tool approval
    ToolApproval,
    /// Pause for user request
    UserRequest,
}

/// Checkpoint info summary (used for list display)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointInfo {
    pub id: String,
    pub graph_name: String,
    pub current_node: String,
    pub step_count: usize,
    #[serde(with = "echo_core::utils::time::local_rfc3339")]
    pub created_at: DateTime<Utc>,
    pub interrupt_type: InterruptType,
    pub parent_checkpoint_id: Option<String>,
    pub label: Option<String>,
    pub tags: Vec<String>,
    pub branch: Option<String>,
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
            parent_checkpoint_id: cp.parent_checkpoint_id.clone(),
            label: cp.label.clone(),
            tags: cp.tags.clone(),
            branch: cp.branch.clone(),
        }
    }
}

/// Filter parameters for listing checkpoints
#[derive(Debug, Clone, Default)]
pub struct CheckpointFilter {
    /// Filter by graph name
    pub graph_name: Option<String>,
    /// Filter by branch name
    pub branch: Option<String>,
    /// Filter by tag
    pub tag: Option<String>,
    /// Limit number of results
    pub limit: Option<usize>,
}

impl CheckpointFilter {
    /// Create a filter that matches a specific graph.
    pub fn by_graph(graph_name: impl Into<String>) -> Self {
        Self {
            graph_name: Some(graph_name.into()),
            ..Default::default()
        }
    }
}

// ── CheckpointStore Trait ──────────────────────────────────────────────────────

/// Checkpoint storage interface
#[async_trait]
pub trait CheckpointStore: Send + Sync {
    /// Save a Checkpoint
    async fn save(&self, checkpoint: &Checkpoint) -> Result<()>;

    /// Load a Checkpoint
    async fn load(&self, id: &str) -> Result<Option<Checkpoint>>;

    /// List all Checkpoint info
    async fn list(&self) -> Result<Vec<CheckpointInfo>>;

    /// List checkpoints for a specific graph
    async fn list_by_graph(&self, graph_name: &str) -> Result<Vec<CheckpointInfo>> {
        let all = self.list().await?;
        Ok(all
            .into_iter()
            .filter(|info| info.graph_name == graph_name)
            .collect())
    }

    /// List checkpoints matching a filter
    async fn list_filtered(&self, filter: &CheckpointFilter) -> Result<Vec<CheckpointInfo>> {
        let mut results = if let Some(ref graph_name) = filter.graph_name {
            self.list_by_graph(graph_name).await?
        } else {
            self.list().await?
        };

        if let Some(ref branch) = filter.branch {
            results.retain(|info| info.branch.as_deref() == Some(branch.as_str()));
        }
        if let Some(ref tag) = filter.tag {
            results.retain(|info| info.tags.contains(tag));
        }
        if let Some(limit) = filter.limit {
            results.truncate(limit);
        }

        Ok(results)
    }

    /// Delete a Checkpoint
    async fn delete(&self, id: &str) -> Result<()>;

    /// Clear all Checkpoints
    async fn clear(&self) -> Result<()>;
}

// ── Memory Checkpoint Store ────────────────────────────────────────────────────

/// In-memory storage implementation (default, non-persistent)
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

/// File storage implementation (supports persistence)
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
                echo_core::error::ReactError::Other(format!(
                    "Failed to create checkpoint dir: {}",
                    e
                ))
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
            echo_core::error::ReactError::Other(format!("Failed to serialize checkpoint: {}", e))
        })?;
        // Atomic write: write to temp file first, then rename to avoid corruption on crash
        let tmp_path = path.with_extension("json.tmp");
        tokio::fs::write(&tmp_path, &json).await.map_err(|e| {
            echo_core::error::ReactError::Other(format!("Failed to write temp checkpoint: {}", e))
        })?;
        tokio::fs::rename(&tmp_path, &path).await.map_err(|e| {
            echo_core::error::ReactError::Other(format!("Failed to rename checkpoint file: {}", e))
        })?;
        Ok(())
    }

    async fn load(&self, id: &str) -> Result<Option<Checkpoint>> {
        let path = self.checkpoint_path(id);
        if !path.exists() {
            return Ok(None);
        }
        let json = tokio::fs::read_to_string(path).await.map_err(|e| {
            echo_core::error::ReactError::Other(format!("Failed to read checkpoint: {}", e))
        })?;
        let checkpoint: Checkpoint = serde_json::from_str(&json).map_err(|e| {
            echo_core::error::ReactError::Other(format!("Failed to parse checkpoint: {}", e))
        })?;
        Ok(Some(checkpoint))
    }

    async fn list(&self) -> Result<Vec<CheckpointInfo>> {
        self.ensure_dir_exists()?;
        let mut entries = tokio::fs::read_dir(&self.base_path).await.map_err(|e| {
            echo_core::error::ReactError::Other(format!("Failed to read checkpoint dir: {}", e))
        })?;

        let mut infos = Vec::new();
        while let Some(entry) = entries.next_entry().await.map_err(|e| {
            echo_core::error::ReactError::Other(format!("Failed to read entry: {}", e))
        })? {
            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false)
                && let Ok(json) = tokio::fs::read_to_string(&path).await
                && let Ok(cp) = serde_json::from_str::<Checkpoint>(&json)
            {
                infos.push(CheckpointInfo::from(&cp));
            }
        }

        // Sort by creation time (newest first)
        infos.sort_by_key(|info| std::cmp::Reverse(info.created_at));
        Ok(infos)
    }

    async fn delete(&self, id: &str) -> Result<()> {
        let path = self.checkpoint_path(id);
        if path.exists() {
            tokio::fs::remove_file(path).await.map_err(|e| {
                echo_core::error::ReactError::Other(format!("Failed to delete checkpoint: {}", e))
            })?;
        }
        Ok(())
    }

    async fn clear(&self) -> Result<()> {
        if !self.base_path.exists() {
            return Ok(());
        }
        let mut entries = tokio::fs::read_dir(&self.base_path).await.map_err(|e| {
            echo_core::error::ReactError::Other(format!("Failed to read checkpoint dir: {}", e))
        })?;

        while let Some(entry) = entries.next_entry().await.map_err(|e| {
            echo_core::error::ReactError::Other(format!("Failed to read entry: {}", e))
        })? {
            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                let _ = tokio::fs::remove_file(path).await;
            }
        }
        Ok(())
    }
}

// ── Unit Tests ────────────────────────────────────────────────────────────────────

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
        // Use a temp path under the current directory
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
