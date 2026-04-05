//! 共享状态（SharedState）
//!
//! 图工作流中节点间通信的核心机制。每个节点通过读写 `SharedState` 传递数据，
//! 状态在每个节点执行后自动 checkpoint，支持 resume / replay。
//!
//! ## 设计原则
//!
//! - **类型安全**：通过 `get::<T>()` / `set()` 以 serde 序列化实现类型安全的 KV 存取
//! - **线程安全**：`Arc<RwLock>` 内部可变，多节点并发安全
//! - **可序列化**：整个 State 可 snapshot 到 JSON，支持持久化
//! - **结构化消息历史**：保留完整的 `Message` 结构（不丢失 tool call 元数据）

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::llm::types::Message;

/// 图工作流的共享状态
///
/// 节点通过 `get` / `set` 读写任意键值对，通过 `messages()` 访问结构化对话历史。
/// 内部使用 `Arc<RwLock>` 实现，可以安全地跨节点、跨线程共享。
///
/// # 示例
///
/// ```rust
/// use echo_agent::workflow::SharedState;
///
/// let state = SharedState::new();
/// state.set("count", 42);
/// state.set("name", "echo");
///
/// assert_eq!(state.get::<i64>("count"), Some(42));
/// assert_eq!(state.get::<String>("name"), Some("echo".to_string()));
/// ```
#[derive(Clone)]
pub struct SharedState {
    inner: Arc<RwLock<StateInner>>,
}

/// 内部状态（可序列化）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StateInner {
    /// 任意 KV 数据
    pub values: HashMap<String, Value>,
    /// 结构化消息历史
    pub messages: Vec<Message>,
    /// 当前所在节点（运行时信息）
    #[serde(default)]
    pub current_node: Option<String>,
}

impl SharedState {
    /// 创建空状态
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(StateInner::default())),
        }
    }

    /// 从已有数据创建
    pub fn from_values(values: HashMap<String, Value>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(StateInner {
                values,
                messages: Vec::new(),
                current_node: None,
            })),
        }
    }

    /// 从快照恢复
    pub fn from_snapshot(snapshot: &str) -> Result<Self, serde_json::Error> {
        let inner: StateInner = serde_json::from_str(snapshot)?;
        Ok(Self {
            inner: Arc::new(RwLock::new(inner)),
        })
    }

    // ── KV 操作 ─────────────────────────────────────────────────────────────

    /// 设置值（自动序列化为 JSON）
    pub fn set<T: Serialize>(&self, key: impl Into<String>, value: T) {
        let v = serde_json::to_value(value).expect("SharedState::set: serialize failed");
        self.inner.write().unwrap().values.insert(key.into(), v);
    }

    /// 获取值（自动反序列化）
    pub fn get<T: for<'de> Deserialize<'de>>(&self, key: &str) -> Option<T> {
        let inner = self.inner.read().unwrap();
        inner
            .values
            .get(key)
            .and_then(|v| serde_json::from_value(v.clone()).ok())
    }

    /// 获取原始 JSON 值
    pub fn get_raw(&self, key: &str) -> Option<Value> {
        self.inner.read().unwrap().values.get(key).cloned()
    }

    /// 检查 key 是否存在
    pub fn contains(&self, key: &str) -> bool {
        self.inner.read().unwrap().values.contains_key(key)
    }

    /// 删除 key
    pub fn remove(&self, key: &str) -> Option<Value> {
        self.inner.write().unwrap().values.remove(key)
    }

    /// 获取所有 key
    pub fn keys(&self) -> Vec<String> {
        self.inner.read().unwrap().values.keys().cloned().collect()
    }

    // ── 消息操作 ────────────────────────────────────────────────────────────

    /// 追加消息
    pub fn push_message(&self, msg: Message) {
        self.inner.write().unwrap().messages.push(msg);
    }

    /// 获取所有消息的克隆
    pub fn messages(&self) -> Vec<Message> {
        self.inner.read().unwrap().messages.clone()
    }

    /// 获取消息数量
    pub fn message_count(&self) -> usize {
        self.inner.read().unwrap().messages.len()
    }

    /// 清空消息
    pub fn clear_messages(&self) {
        self.inner.write().unwrap().messages.clear();
    }

    // ── 节点追踪 ────────────────────────────────────────────────────────────

    /// 设置当前节点
    pub(crate) fn set_current_node(&self, node: impl Into<String>) {
        self.inner.write().unwrap().current_node = Some(node.into());
    }

    /// 获取当前节点
    pub fn current_node(&self) -> Option<String> {
        self.inner.read().unwrap().current_node.clone()
    }

    // ── 序列化 ──────────────────────────────────────────────────────────────

    /// 导出为 JSON 快照
    pub fn snapshot(&self) -> String {
        let inner = self.inner.read().unwrap();
        serde_json::to_string_pretty(&*inner).expect("SharedState::snapshot: serialize failed")
    }

    /// 导出为 JSON Value
    pub fn to_json(&self) -> serde_json::Value {
        let inner = self.inner.read().unwrap();
        serde_json::to_value(&*inner).expect("SharedState::to_json: serialize failed")
    }

    /// 从 JSON Value 恢复
    pub fn from_json(json: &serde_json::Value) -> Result<Self, serde_json::Error> {
        let inner: StateInner = serde_json::from_value(json.clone())?;
        Ok(Self {
            inner: Arc::new(RwLock::new(inner)),
        })
    }

    /// 合并另一个 state 的 values（不覆盖已有 key）
    pub fn merge(&self, other: &SharedState) {
        let other_inner = other.inner.read().unwrap();
        let mut self_inner = self.inner.write().unwrap();
        for (k, v) in &other_inner.values {
            self_inner
                .values
                .entry(k.clone())
                .or_insert_with(|| v.clone());
        }
    }

    /// 合并另一个 state 的 values（覆盖已有 key）
    pub fn merge_overwrite(&self, other: &SharedState) {
        let other_inner = other.inner.read().unwrap();
        let mut self_inner = self.inner.write().unwrap();
        for (k, v) in &other_inner.values {
            self_inner.values.insert(k.clone(), v.clone());
        }
    }
}

impl Default for SharedState {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for SharedState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let inner = self.inner.read().unwrap();
        f.debug_struct("SharedState")
            .field("keys", &inner.values.keys().collect::<Vec<_>>())
            .field("messages", &inner.messages.len())
            .field("current_node", &inner.current_node)
            .finish()
    }
}

// ── 单元测试 ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_set_get_typed() {
        let state = SharedState::new();
        state.set("count", 42i64);
        state.set("name", "echo");
        state.set("tags", vec!["a", "b"]);

        assert_eq!(state.get::<i64>("count"), Some(42));
        assert_eq!(state.get::<String>("name"), Some("echo".to_string()));
        assert_eq!(
            state.get::<Vec<String>>("tags"),
            Some(vec!["a".to_string(), "b".to_string()])
        );
        assert_eq!(state.get::<i64>("missing"), None);
    }

    #[test]
    fn test_contains_remove() {
        let state = SharedState::new();
        state.set("x", 1);
        assert!(state.contains("x"));
        assert!(!state.contains("y"));

        state.remove("x");
        assert!(!state.contains("x"));
    }

    #[test]
    fn test_messages() {
        let state = SharedState::new();
        state.push_message(Message::user("hello".to_string()));
        state.push_message(Message::assistant("hi".to_string()));

        assert_eq!(state.message_count(), 2);
        let msgs = state.messages();
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[1].role, "assistant");
    }

    #[test]
    fn test_snapshot_restore() {
        let state = SharedState::new();
        state.set("key", "value");
        state.push_message(Message::user("hello".to_string()));

        let snap = state.snapshot();
        let restored = SharedState::from_snapshot(&snap).unwrap();

        assert_eq!(restored.get::<String>("key"), Some("value".to_string()));
        assert_eq!(restored.message_count(), 1);
    }

    #[test]
    fn test_merge() {
        let a = SharedState::new();
        a.set("x", 1);
        a.set("shared", "from_a");

        let b = SharedState::new();
        b.set("y", 2);
        b.set("shared", "from_b");

        a.merge(&b);
        assert_eq!(a.get::<i64>("x"), Some(1));
        assert_eq!(a.get::<i64>("y"), Some(2));
        assert_eq!(a.get::<String>("shared"), Some("from_a".to_string())); // 不覆盖
    }

    #[test]
    fn test_merge_overwrite() {
        let a = SharedState::new();
        a.set("shared", "from_a");

        let b = SharedState::new();
        b.set("shared", "from_b");

        a.merge_overwrite(&b);
        assert_eq!(a.get::<String>("shared"), Some("from_b".to_string()));
    }

    #[test]
    fn test_clone_shares_state() {
        let state = SharedState::new();
        let cloned = state.clone();

        state.set("x", 42);
        assert_eq!(cloned.get::<i64>("x"), Some(42)); // Arc 共享
    }

    #[test]
    fn test_from_values() {
        let mut vals = HashMap::new();
        vals.insert("key".to_string(), serde_json::json!("value"));

        let state = SharedState::from_values(vals);
        assert_eq!(state.get::<String>("key"), Some("value".to_string()));
    }
}
