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

use echo_core::error::ReactError;
use echo_core::llm::types::Message;

/// 递归合并两个 JSON object
fn deep_merge_values(
    target: &mut serde_json::Map<String, Value>,
    source: &serde_json::Map<String, Value>,
) {
    for (k, v) in source {
        if let Some(existing) = target.get_mut(k) {
            if let (Some(obj_a), Some(obj_b)) = (existing.as_object_mut(), v.as_object()) {
                deep_merge_values(obj_a, obj_b);
                continue;
            }
        }
        target.insert(k.clone(), v.clone());
    }
}

/// SharedState 操作返回类型
pub type StateResult<T> = std::result::Result<T, StateError>;

#[derive(Debug)]
pub enum StateError {
    Serialize(String),
    LockPoisoned(String),
}

impl std::fmt::Display for StateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StateError::Serialize(e) => write!(f, "序列化失败: {e}"),
            StateError::LockPoisoned(e) => write!(f, "锁中毒: {e}"),
        }
    }
}

impl std::error::Error for StateError {}

impl From<StateError> for ReactError {
    fn from(e: StateError) -> Self {
        ReactError::Other(e.to_string())
    }
}

/// 图工作流的共享状态
///
/// 节点通过 `get` / `set` 读写任意键值对，通过 `messages()` 访问结构化对话历史。
/// 内部使用 `Arc<RwLock>` 实现，可以安全地跨节点、跨线程共享。
///
/// # 示例
///
/// ```rust
/// use echo_orchestration::workflow::SharedState;
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
    pub fn from_snapshot(snapshot: &str) -> std::result::Result<Self, serde_json::Error> {
        let inner: StateInner = serde_json::from_str(snapshot)?;
        Ok(Self {
            inner: Arc::new(RwLock::new(inner)),
        })
    }

    // ── KV 操作 ─────────────────────────────────────────────────────────────

    /// 设置值（自动序列化为 JSON）。
    ///
    /// 返回 Result（`StateResult<()>`），序列化失败或锁中毒时不再 panic。
    /// 如需向后兼容的无返回值 API，使用 [`Self::set_unwrap`]。
    pub fn set<T: Serialize>(&self, key: impl Into<String>, value: T) -> StateResult<()> {
        let key = key.into();
        let v = serde_json::to_value(value).map_err(|e| StateError::Serialize(e.to_string()))?;
        let mut inner = self
            .inner
            .write()
            .map_err(|e| StateError::LockPoisoned(e.to_string()))?;
        inner.values.insert(key, v);
        drop(inner);
        Ok(())
    }

    /// 向后兼容：设置值，失败时返回 Option<()>（不 panic）
    pub fn set_best_effort<T: Serialize>(&self, key: impl Into<String>, value: T) -> Option<()> {
        self.set(key, value).ok()
    }

    /// 获取值（自动反序列）
    pub fn get<T: for<'de> Deserialize<'de>>(&self, key: &str) -> Option<T> {
        let Ok(inner) = self.inner.read() else {
            return None;
        };
        inner
            .values
            .get(key)
            .and_then(|v| serde_json::from_value(v.clone()).ok())
    }

    /// 获取原始 JSON 值
    pub fn get_raw(&self, key: &str) -> Option<Value> {
        self.inner.read().ok()?.values.get(key).cloned()
    }

    /// 检查 key 是否存在
    pub fn contains(&self, key: &str) -> bool {
        self.inner
            .read()
            .map(|inner| inner.values.contains_key(key))
            .unwrap_or(false)
    }

    /// 删除 key
    pub fn remove(&self, key: &str) -> Option<Value> {
        self.inner.write().ok()?.values.remove(key)
    }

    /// 获取所有 key
    pub fn keys(&self) -> Vec<String> {
        self.inner
            .read()
            .map(|inner| inner.values.keys().cloned().collect())
            .unwrap_or_default()
    }

    // ── 消息操作 ────────────────────────────────────────────────────────────

    /// 追加消息，返回 Result
    pub fn push_message(&self, msg: Message) -> StateResult<()> {
        let mut inner = self
            .inner
            .write()
            .map_err(|e| StateError::LockPoisoned(e.to_string()))?;
        inner.messages.push(msg);
        drop(inner);
        Ok(())
    }

    /// 获取所有消息的克隆
    pub fn messages(&self) -> Vec<Message> {
        self.inner
            .read()
            .map(|inner| inner.messages.clone())
            .unwrap_or_default()
    }

    /// 获取消息数量
    pub fn message_count(&self) -> usize {
        self.inner
            .read()
            .map(|inner| inner.messages.len())
            .unwrap_or(0)
    }

    /// 清空消息，返回 Result
    pub fn clear_messages(&self) -> StateResult<()> {
        let mut inner = self
            .inner
            .write()
            .map_err(|e| StateError::LockPoisoned(e.to_string()))?;
        inner.messages.clear();
        drop(inner);
        Ok(())
    }

    // ── 节点追踪 ────────────────────────────────────────────────────────────

    /// 设置当前节点
    pub(crate) fn set_current_node(&self, node: impl Into<String>) {
        if let Ok(mut inner) = self.inner.write() {
            inner.current_node = Some(node.into());
        }
    }

    /// 获取当前节点
    pub fn current_node(&self) -> Option<String> {
        self.inner
            .read()
            .ok()
            .and_then(|inner| inner.current_node.clone())
    }

    /// Create an isolated copy for branch execution.
    pub fn fork(&self) -> StateResult<Self> {
        let inner = self
            .inner
            .read()
            .map_err(|e| StateError::LockPoisoned(e.to_string()))?
            .clone();
        Ok(Self {
            inner: Arc::new(RwLock::new(inner)),
        })
    }

    // ── 序列化 ──────────────────────────────────────────────────────────────

    /// 出为 JSON 快照，返回 Result（不再 panic）
    pub fn snapshot(&self) -> StateResult<String> {
        let inner = self
            .inner
            .read()
            .map_err(|e| StateError::LockPoisoned(e.to_string()))?;
        serde_json::to_string_pretty(&*inner).map_err(|e| StateError::Serialize(e.to_string()))
    }

    /// 便捷方法：导出为 JSON 快照，失败时 unwrap
    pub fn snapshot_unwrap(&self) -> String {
        self.snapshot()
            .unwrap_or_else(|e| panic!("SharedState::snapshot_unwrap: {e}"))
    }

    /// 导出为 JSON Value，返回 Result
    pub fn to_json_value(&self) -> StateResult<serde_json::Value> {
        let inner = self
            .inner
            .read()
            .map_err(|e| StateError::LockPoisoned(e.to_string()))?;
        serde_json::to_value(&*inner).map_err(|e| StateError::Serialize(e.to_string()))
    }

    /// 便捷方法：导出为 JSON Value，失败时 unwrap
    pub fn to_json(&self) -> serde_json::Value {
        self.to_json_value()
            .unwrap_or_else(|e| panic!("SharedState::to_json: {e}"))
    }

    /// 从 JSON Value 恢复
    pub fn from_json(json: &serde_json::Value) -> std::result::Result<Self, serde_json::Error> {
        let inner: StateInner = serde_json::from_value(json.clone())?;
        Ok(Self {
            inner: Arc::new(RwLock::new(inner)),
        })
    }

    /// 合并另一个 state 的 values（不覆盖已有 key），返回 Result
    pub fn merge(&self, other: &SharedState) -> StateResult<()> {
        let other_inner = self
            .inner
            .read()
            .map_err(|e| StateError::LockPoisoned(e.to_string()))?;
        drop(other_inner);
        let self_inner = self
            .inner
            .read()
            .map_err(|e| StateError::LockPoisoned(e.to_string()))?;
        // SAFETY: need read both locks to prevent deadlock.
        // merge reads from others and writes to self, we re-lock sequentially
        drop(self_inner);

        let other_lock = other
            .inner
            .read()
            .map_err(|e| StateError::LockPoisoned(e.to_string()))?;
        let mut self_lock = self
            .inner
            .write()
            .map_err(|e| StateError::LockPoisoned(e.to_string()))?;
        for (k, v) in &other_lock.values {
            self_lock
                .values
                .entry(k.clone())
                .or_insert_with(|| v.clone());
        }
        drop(other_lock);
        drop(self_lock);
        Ok(())
    }

    /// 合并另一个 state 的 values（覆盖已有 key），返回 Result
    pub fn merge_overwrite(&self, other: &SharedState) -> StateResult<()> {
        let other_lock = other
            .inner
            .read()
            .map_err(|e| StateError::LockPoisoned(e.to_string()))?;
        let mut self_lock = self
            .inner
            .write()
            .map_err(|e| StateError::LockPoisoned(e.to_string()))?;
        for (k, v) in &other_lock.values {
            self_lock.values.insert(k.clone(), v.clone());
        }
        drop(other_lock);
        drop(self_lock);
        Ok(())
    }

    /// 深度合并另一个 state 的 values（递归合并嵌套结构），返回 Result
    ///
    /// 与 `merge_overwrite` 不同：当 key 对应值都是 JSON object 时，
    /// 此方法会递归合并两个 object 的字段，而非整体覆盖。
    /// 适用于并行分支修改同一 state 的嵌套结构时保持数据一致性。
    pub fn deep_merge(&self, other: &SharedState) -> StateResult<()> {
        let other_lock = other
            .inner
            .read()
            .map_err(|e| StateError::LockPoisoned(e.to_string()))?;
        let mut self_lock = self
            .inner
            .write()
            .map_err(|e| StateError::LockPoisoned(e.to_string()))?;
        for (k, other_val) in &other_lock.values {
            if let Some(self_val) = self_lock.values.get(k) {
                // 如果两者都是 object，递归合并
                if let (Some(self_obj), Some(other_obj)) =
                    (self_val.as_object(), other_val.as_object())
                {
                    let mut merged = self_obj.clone();
                    deep_merge_values(&mut merged, other_obj);
                    self_lock.values.insert(k.clone(), Value::Object(merged));
                    continue;
                }
            }
            // 否则直接覆盖
            self_lock.values.insert(k.clone(), other_val.clone());
        }
        drop(other_lock);
        drop(self_lock);
        Ok(())
    }
}

impl Default for SharedState {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for SharedState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.inner.read() {
            Ok(inner) => f
                .debug_struct("SharedState")
                .field("keys", &inner.values.keys().collect::<Vec<_>>())
                .field("messages", &inner.messages.len())
                .field("current_node", &inner.current_node)
                .finish(),
            Err(_) => f
                .debug_struct("SharedState")
                .field("error", &"lock poisoned")
                .finish(),
        }
    }
}

// ── 单元测试 ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_set_get_typed() {
        let state = SharedState::new();
        state.set("count", 42i64).unwrap();
        state.set("name", "echo").unwrap();
        state.set("tags", vec!["a", "b"]).unwrap();

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
        state.set("x", 1).unwrap();
        assert!(state.contains("x"));
        assert!(!state.contains("y"));

        state.remove("x");
        assert!(!state.contains("x"));
    }

    #[test]
    fn test_messages() {
        let state = SharedState::new();
        state
            .push_message(Message::user("hello".to_string()))
            .unwrap();
        state
            .push_message(Message::assistant("hi".to_string()))
            .unwrap();

        assert_eq!(state.message_count(), 2);
        let msgs = state.messages();
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[1].role, "assistant");
    }

    #[test]
    fn test_snapshot_restore() {
        let state = SharedState::new();
        state.set("key", "value").unwrap();
        state
            .push_message(Message::user("hello".to_string()))
            .unwrap();

        let snap = state.snapshot().unwrap();
        let restored = SharedState::from_snapshot(&snap).unwrap();

        assert_eq!(restored.get::<String>("key"), Some("value".to_string()));
        assert_eq!(restored.message_count(), 1);
    }

    #[test]
    fn test_merge() {
        let a = SharedState::new();
        a.set("x", 1).unwrap();
        a.set("shared", "from_a").unwrap();

        let b = SharedState::new();
        b.set("y", 2).unwrap();
        b.set("shared", "from_b").unwrap();

        a.merge(&b).unwrap();
        assert_eq!(a.get::<i64>("x"), Some(1));
        assert_eq!(a.get::<i64>("y"), Some(2));
        assert_eq!(a.get::<String>("shared"), Some("from_a".to_string())); // 不覆盖
    }

    #[test]
    fn test_merge_overwrite() {
        let a = SharedState::new();
        a.set("shared", "from_a").unwrap();

        let b = SharedState::new();
        b.set("shared", "from_b").unwrap();

        a.merge_overwrite(&b).unwrap();
        assert_eq!(a.get::<String>("shared"), Some("from_b".to_string()));
    }

    #[test]
    fn test_clone_shares_state() {
        let state = SharedState::new();
        let cloned = state.clone();

        state.set("x", 42).unwrap();
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
