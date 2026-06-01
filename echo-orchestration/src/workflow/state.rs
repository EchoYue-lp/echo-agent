//! Shared state (SharedState)
//!
//! The core mechanism for inter-node communication in graph workflows. Each node passes data
//! by reading and writing `SharedState`. State is checkpointed automatically after each
//! node executes, supporting resume / replay.
//!
//! ## Design Principles
//!
//! - **Type safety**: type-safe KV access via `get::<T>()` / `set()` with serde serialization
//! - **Thread safety**: `Arc<RwLock>` interior mutability, concurrency-safe across nodes
//! - **Serializable**: the entire State can be snapshotted to JSON for persistence
//! - **Structured message history**: preserves full `Message` structure (no loss of tool call metadata)

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use echo_core::error::ReactError;
use echo_core::llm::types::Message;

/// Recursively merge two JSON objects
fn deep_merge_values(
    target: &mut serde_json::Map<String, Value>,
    source: &serde_json::Map<String, Value>,
) {
    for (k, v) in source {
        if let Some(existing) = target.get_mut(k)
            && let (Some(obj_a), Some(obj_b)) = (existing.as_object_mut(), v.as_object())
        {
            deep_merge_values(obj_a, obj_b);
            continue;
        }
        target.insert(k.clone(), v.clone());
    }
}

/// SharedState operation return type
pub type StateResult<T> = std::result::Result<T, StateError>;

#[derive(Debug)]
pub enum StateError {
    /// Serialization or deserialization failure.
    Serialize(String),
    /// A lock was poisoned (another thread panicked while holding it).
    LockPoisoned(String),
    /// A type mismatch occurred when deserializing a stored value.
    TypeMismatch {
        key: String,
        expected: String,
        found: String,
    },
    /// A required key is missing from the state.
    MissingKey(String),
    /// An invalid operation was attempted (e.g., fork on empty state).
    InvalidOperation(String),
}

impl std::fmt::Display for StateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StateError::Serialize(e) => write!(f, "Serialization failed: {e}"),
            StateError::LockPoisoned(e) => write!(f, "Lock poisoned: {e}"),
            StateError::TypeMismatch {
                key,
                expected,
                found,
            } => write!(
                f,
                "Type mismatch for key '{key}': expected {expected}, found {found}"
            ),
            StateError::MissingKey(key) => write!(f, "Missing key: '{key}'"),
            StateError::InvalidOperation(msg) => write!(f, "Invalid operation: {msg}"),
        }
    }
}

impl std::error::Error for StateError {}

impl From<StateError> for ReactError {
    fn from(e: StateError) -> Self {
        match e {
            StateError::Serialize(msg) => {
                ReactError::Other(format!("State serialization error: {msg}"))
            }
            StateError::LockPoisoned(msg) => {
                ReactError::Other(format!("State lock poisoned: {msg}"))
            }
            StateError::TypeMismatch {
                key,
                expected,
                found,
            } => ReactError::Other(format!(
                "State type mismatch for '{key}': expected {expected}, found {found}"
            )),
            StateError::MissingKey(key) => ReactError::Other(format!("State missing key: '{key}'")),
            StateError::InvalidOperation(msg) => {
                ReactError::Other(format!("State invalid operation: {msg}"))
            }
        }
    }
}

/// Shared state for graph workflows
///
/// Nodes read/write arbitrary key-value pairs via `get` / `set`, and access structured
/// conversation history via `messages()`. Internally uses `Arc<RwLock>`, safe to share
/// across nodes and threads.
///
/// # Example
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

/// Internal state (serializable)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StateInner {
    /// Arbitrary KV data
    pub values: HashMap<String, Value>,
    /// Structured message history
    pub messages: Vec<Message>,
    /// Current node (runtime info)
    #[serde(default)]
    pub current_node: Option<String>,
}

impl SharedState {
    /// Create an empty state
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(StateInner::default())),
        }
    }

    /// Create from existing data
    pub fn from_values(values: HashMap<String, Value>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(StateInner {
                values,
                messages: Vec::new(),
                current_node: None,
            })),
        }
    }

    /// Restore from a snapshot
    pub fn from_snapshot(snapshot: &str) -> std::result::Result<Self, serde_json::Error> {
        let inner: StateInner = serde_json::from_str(snapshot)?;
        Ok(Self {
            inner: Arc::new(RwLock::new(inner)),
        })
    }

    // ── KV Operations ─────────────────────────────────────────────────────────────

    /// Set a value (auto-serialized to JSON).
    ///
    /// Returns Result (`StateResult<()>`) -- no longer panics on serialization failure or lock poison.
    /// For a backward-compatible void-returning API, use [`Self::set_unwrap`].
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

    /// Backward-compatible: set value, returns Option<()> on failure (no panic)
    pub fn set_best_effort<T: Serialize>(&self, key: impl Into<String>, value: T) -> Option<()> {
        self.set(key, value).ok()
    }

    /// Get a value (auto-deserialized)
    pub fn get<T: for<'de> Deserialize<'de>>(&self, key: &str) -> Option<T> {
        let Ok(inner) = self.inner.read() else {
            return None;
        };
        inner
            .values
            .get(key)
            .and_then(|v| serde_json::from_value(v.clone()).ok())
    }

    /// Get raw JSON value
    pub fn get_raw(&self, key: &str) -> Option<Value> {
        self.inner.read().ok()?.values.get(key).cloned()
    }

    /// Check if key exists
    pub fn contains(&self, key: &str) -> bool {
        self.inner
            .read()
            .map(|inner| inner.values.contains_key(key))
            .unwrap_or(false)
    }

    /// Remove a key
    pub fn remove(&self, key: &str) -> Option<Value> {
        self.inner.write().ok()?.values.remove(key)
    }

    /// Get all keys
    pub fn keys(&self) -> Vec<String> {
        self.inner
            .read()
            .map(|inner| inner.values.keys().cloned().collect())
            .unwrap_or_default()
    }

    // ── Message Operations ────────────────────────────────────────────────────────────

    /// Push a message, returns Result
    pub fn push_message(&self, msg: Message) -> StateResult<()> {
        let mut inner = self
            .inner
            .write()
            .map_err(|e| StateError::LockPoisoned(e.to_string()))?;
        inner.messages.push(msg);
        drop(inner);
        Ok(())
    }

    /// Get a clone of all messages
    pub fn messages(&self) -> Vec<Message> {
        self.inner
            .read()
            .map(|inner| inner.messages.clone())
            .unwrap_or_default()
    }

    /// Get message count
    pub fn message_count(&self) -> usize {
        self.inner
            .read()
            .map(|inner| inner.messages.len())
            .unwrap_or(0)
    }

    /// Clear messages, returns Result
    pub fn clear_messages(&self) -> StateResult<()> {
        let mut inner = self
            .inner
            .write()
            .map_err(|e| StateError::LockPoisoned(e.to_string()))?;
        inner.messages.clear();
        drop(inner);
        Ok(())
    }

    // ── Node Tracking ────────────────────────────────────────────────────────────

    /// Set the current node
    pub(crate) fn set_current_node(&self, node: impl Into<String>) {
        if let Ok(mut inner) = self.inner.write() {
            inner.current_node = Some(node.into());
        }
    }

    /// Get the current node
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

    // ── Serialization ──────────────────────────────────────────────────────────────

    /// Export as JSON snapshot, returns Result (no longer panics)
    pub fn snapshot(&self) -> StateResult<String> {
        let inner = self
            .inner
            .read()
            .map_err(|e| StateError::LockPoisoned(e.to_string()))?;
        serde_json::to_string_pretty(&*inner).map_err(|e| StateError::Serialize(e.to_string()))
    }

    /// Convenience method: export as JSON snapshot, unwraps on failure
    pub fn snapshot_unwrap(&self) -> String {
        self.snapshot()
            .unwrap_or_else(|e| panic!("SharedState::snapshot_unwrap: {e}"))
    }

    /// Export as JSON Value, returns Result
    pub fn to_json_value(&self) -> StateResult<serde_json::Value> {
        let inner = self
            .inner
            .read()
            .map_err(|e| StateError::LockPoisoned(e.to_string()))?;
        serde_json::to_value(&*inner).map_err(|e| StateError::Serialize(e.to_string()))
    }

    /// Convenience method: export as JSON Value, unwraps on failure
    pub fn to_json(&self) -> serde_json::Value {
        self.to_json_value()
            .unwrap_or_else(|e| panic!("SharedState::to_json: {e}"))
    }

    /// Restore from JSON Value
    pub fn from_json(json: &serde_json::Value) -> std::result::Result<Self, serde_json::Error> {
        let inner: StateInner = serde_json::from_value(json.clone())?;
        Ok(Self {
            inner: Arc::new(RwLock::new(inner)),
        })
    }

    /// Merge values from another state (does not overwrite existing keys), returns Result
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

    /// Merge values from another state (overwrites existing keys), returns Result
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

    /// Deep merge values from another state (recursively merge nested structures), returns Result
    ///
    /// Unlike `merge_overwrite`: when a key's value is a JSON object in both states,
    /// this method recursively merges the two object's fields rather than wholesale overwrite.
    /// Useful for maintaining data consistency when parallel branches modify nested structures
    /// in the same state.
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
                // If both are objects, recursively merge
                if let (Some(self_obj), Some(other_obj)) =
                    (self_val.as_object(), other_val.as_object())
                {
                    let mut merged = self_obj.clone();
                    deep_merge_values(&mut merged, other_obj);
                    self_lock.values.insert(k.clone(), Value::Object(merged));
                    continue;
                }
            }
            // Otherwise overwrite directly
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

// ── Unit Tests ────────────────────────────────────────────────────────────────

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
        assert_eq!(msgs[0].role, "user".into());
        assert_eq!(msgs[1].role, "assistant".into());
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
        assert_eq!(a.get::<String>("shared"), Some("from_a".to_string())); // not overwritten
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
        assert_eq!(cloned.get::<i64>("x"), Some(42)); // Arc shared
    }

    #[test]
    fn test_from_values() {
        let mut vals = HashMap::new();
        vals.insert("key".to_string(), serde_json::json!("value"));

        let state = SharedState::from_values(vals);
        assert_eq!(state.get::<String>("key"), Some("value".to_string()));
    }
}
