//! Agent 状态快照与回滚
//!
//! 在 ReAct 循环的每轮迭代后自动（或手动）捕获对话历史快照，
//! 异常时可回滚到上一个 known-good 状态。
//!
//! ## 快速上手
//!
//! ```rust,no_run
//! use echo_agent::prelude::*;
//! use echo_agent::memory::snapshot::SnapshotPolicy;
//!
//! # fn example() -> echo_agent::error::Result<()> {
//! let mut agent = ReactAgentBuilder::new()
//!     .model("qwen3-max")
//!     .system_prompt("你是一个助手")
//!     .snapshot_policy(SnapshotPolicy::EveryIteration)
//!     .max_snapshots(20)
//!     .build()?;
//!
//! // execute() 期间自动快照；出错后可回滚
//! // agent.rollback(1) 回到上一轮迭代状态
//! # Ok(())
//! # }
//! ```

use crate::llm::types::Message;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ── StateSnapshot ─────────────────────────────────────────────────────────────

/// 单次状态快照，记录某一时刻的完整对话历史
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateSnapshot {
    /// 快照唯一 ID（UUID v4）
    pub id: String,
    /// 产生快照的 ReAct 迭代轮次（从 0 开始）
    pub iteration: usize,
    /// 该时刻的完整消息历史
    pub messages: Vec<Message>,
    /// 用户自定义元数据（可选）
    pub metadata: HashMap<String, String>,
    /// 创建时间（Unix 秒）
    pub created_at: u64,
}

// ── SnapshotPolicy ────────────────────────────────────────────────────────────

/// 自动快照策略
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotPolicy {
    /// 每轮 ReAct 迭代后自动快照
    EveryIteration,
    /// 每 N 轮迭代后自动快照
    EveryN(usize),
    /// 仅手动调用 `snapshot()` 时创建
    Manual,
}

impl Default for SnapshotPolicy {
    fn default() -> Self {
        Self::EveryIteration
    }
}

// ── SnapshotManager ───────────────────────────────────────────────────────────

/// 管理状态快照的环形缓冲区
///
/// 超过 `max_snapshots` 时自动淘汰最早的快照。
pub struct SnapshotManager {
    snapshots: Vec<StateSnapshot>,
    policy: SnapshotPolicy,
    max_snapshots: usize,
}

impl SnapshotManager {
    /// 创建快照管理器
    ///
    /// - `policy`：自动快照策略
    /// - `max_snapshots`：最多保留的快照数量
    pub fn new(policy: SnapshotPolicy, max_snapshots: usize) -> Self {
        Self {
            snapshots: Vec::new(),
            policy,
            max_snapshots: max_snapshots.max(1),
        }
    }

    /// 获取当前快照策略
    pub fn policy(&self) -> &SnapshotPolicy {
        &self.policy
    }

    /// 根据策略判断当前迭代是否应该自动快照
    pub fn should_capture(&self, iteration: usize) -> bool {
        match &self.policy {
            SnapshotPolicy::EveryIteration => true,
            SnapshotPolicy::EveryN(n) => *n > 0 && (iteration + 1) % n == 0,
            SnapshotPolicy::Manual => false,
        }
    }

    /// 捕获一份快照，返回快照 ID
    pub fn capture(&mut self, iteration: usize, messages: &[Message]) -> String {
        self.capture_with_metadata(iteration, messages, HashMap::new())
    }

    /// 捕获一份快照（附带自定义元数据），返回快照 ID
    pub fn capture_with_metadata(
        &mut self,
        iteration: usize,
        messages: &[Message],
        metadata: HashMap<String, String>,
    ) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let snapshot = StateSnapshot {
            id: id.clone(),
            iteration,
            messages: messages.to_vec(),
            metadata,
            created_at: now_secs(),
        };

        self.snapshots.push(snapshot);

        if self.snapshots.len() > self.max_snapshots {
            let excess = self.snapshots.len() - self.max_snapshots;
            self.snapshots.drain(..excess);
        }

        id
    }

    /// 回滚到 N 步之前的快照，返回该快照的消息历史
    ///
    /// `steps_back = 1` 表示回到最近一次快照，`2` 表示回到倒数第二次，依此类推。
    /// 回滚后，该快照之后的所有快照都会被丢弃。
    pub fn rollback(&mut self, steps_back: usize) -> Option<StateSnapshot> {
        if steps_back == 0 || steps_back > self.snapshots.len() {
            return None;
        }
        let target_idx = self.snapshots.len() - steps_back;
        let snapshot = self.snapshots[target_idx].clone();
        self.snapshots.truncate(target_idx + 1);
        Some(snapshot)
    }

    /// 回滚到指定 ID 的快照
    pub fn rollback_to(&mut self, snapshot_id: &str) -> Option<StateSnapshot> {
        let idx = self
            .snapshots
            .iter()
            .position(|s| s.id == snapshot_id)?;
        let snapshot = self.snapshots[idx].clone();
        self.snapshots.truncate(idx + 1);
        Some(snapshot)
    }

    /// 获取最新快照
    pub fn latest(&self) -> Option<&StateSnapshot> {
        self.snapshots.last()
    }

    /// 获取所有快照（时间正序）
    pub fn list(&self) -> &[StateSnapshot] {
        &self.snapshots
    }

    /// 快照数量
    pub fn len(&self) -> usize {
        self.snapshots.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.snapshots.is_empty()
    }

    /// 清空所有快照
    pub fn clear(&mut self) {
        self.snapshots.clear();
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// ── 单元测试 ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_messages(n: usize) -> Vec<Message> {
        (0..n)
            .map(|i| Message::user(format!("msg-{i}")))
            .collect()
    }

    #[test]
    fn test_capture_and_list() {
        let mut mgr = SnapshotManager::new(SnapshotPolicy::EveryIteration, 10);
        assert!(mgr.is_empty());

        let id1 = mgr.capture(0, &sample_messages(2));
        let id2 = mgr.capture(1, &sample_messages(3));

        assert_eq!(mgr.len(), 2);
        assert_eq!(mgr.list()[0].id, id1);
        assert_eq!(mgr.list()[1].id, id2);
        assert_eq!(mgr.latest().unwrap().messages.len(), 3);
    }

    #[test]
    fn test_max_snapshots_eviction() {
        let mut mgr = SnapshotManager::new(SnapshotPolicy::EveryIteration, 3);

        for i in 0..5 {
            mgr.capture(i, &sample_messages(1));
        }

        assert_eq!(mgr.len(), 3);
        assert_eq!(mgr.list()[0].iteration, 2);
        assert_eq!(mgr.list()[2].iteration, 4);
    }

    #[test]
    fn test_rollback_steps() {
        let mut mgr = SnapshotManager::new(SnapshotPolicy::EveryIteration, 10);
        mgr.capture(0, &sample_messages(1));
        mgr.capture(1, &sample_messages(2));
        mgr.capture(2, &sample_messages(3));

        let snapshot = mgr.rollback(2).unwrap();
        assert_eq!(snapshot.iteration, 1);
        assert_eq!(snapshot.messages.len(), 2);
        assert_eq!(mgr.len(), 2);
    }

    #[test]
    fn test_rollback_out_of_range() {
        let mut mgr = SnapshotManager::new(SnapshotPolicy::EveryIteration, 10);
        mgr.capture(0, &sample_messages(1));

        assert!(mgr.rollback(0).is_none());
        assert!(mgr.rollback(5).is_none());
    }

    #[test]
    fn test_rollback_to_id() {
        let mut mgr = SnapshotManager::new(SnapshotPolicy::EveryIteration, 10);
        let _id1 = mgr.capture(0, &sample_messages(1));
        let id2 = mgr.capture(1, &sample_messages(2));
        let _id3 = mgr.capture(2, &sample_messages(3));

        let snapshot = mgr.rollback_to(&id2).unwrap();
        assert_eq!(snapshot.iteration, 1);
        assert_eq!(mgr.len(), 2);
    }

    #[test]
    fn test_rollback_to_unknown_id() {
        let mut mgr = SnapshotManager::new(SnapshotPolicy::EveryIteration, 10);
        mgr.capture(0, &sample_messages(1));

        assert!(mgr.rollback_to("nonexistent").is_none());
    }

    #[test]
    fn test_should_capture_every_iteration() {
        let mgr = SnapshotManager::new(SnapshotPolicy::EveryIteration, 10);
        assert!(mgr.should_capture(0));
        assert!(mgr.should_capture(1));
        assert!(mgr.should_capture(99));
    }

    #[test]
    fn test_should_capture_every_n() {
        let mgr = SnapshotManager::new(SnapshotPolicy::EveryN(3), 10);
        assert!(!mgr.should_capture(0));
        assert!(!mgr.should_capture(1));
        assert!(mgr.should_capture(2));
        assert!(!mgr.should_capture(3));
        assert!(!mgr.should_capture(4));
        assert!(mgr.should_capture(5));
    }

    #[test]
    fn test_should_capture_manual() {
        let mgr = SnapshotManager::new(SnapshotPolicy::Manual, 10);
        assert!(!mgr.should_capture(0));
        assert!(!mgr.should_capture(99));
    }

    #[test]
    fn test_clear() {
        let mut mgr = SnapshotManager::new(SnapshotPolicy::EveryIteration, 10);
        mgr.capture(0, &sample_messages(1));
        mgr.capture(1, &sample_messages(2));
        assert_eq!(mgr.len(), 2);

        mgr.clear();
        assert!(mgr.is_empty());
    }

    #[test]
    fn test_capture_with_metadata() {
        let mut mgr = SnapshotManager::new(SnapshotPolicy::EveryIteration, 10);
        let mut meta = HashMap::new();
        meta.insert("reason".to_string(), "before_risky_tool".to_string());

        mgr.capture_with_metadata(0, &sample_messages(1), meta);

        let snapshot = mgr.latest().unwrap();
        assert_eq!(
            snapshot.metadata.get("reason").unwrap(),
            "before_risky_tool"
        );
    }
}
