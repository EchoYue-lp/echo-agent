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

use echo_core::error::ReactError;
use echo_core::utils::blocking::{
    BlockingFileOperationKey, BlockingFileOperationScope, run_keyed_file_operation,
};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::{
    AgentCheckpoint, RuntimeStateClearReceipt, RuntimeStateScopeClearReceipt, RuntimeStateStore,
};

const SCOPE_INDEX_VERSION: u8 = 1;

#[derive(Debug, Serialize, Deserialize)]
struct RuntimeStateScopeIndex {
    version: u8,
    scope_id: String,
    runtime_state_ids: BTreeSet<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct RuntimeStateOwner {
    version: u8,
    runtime_state_id: String,
    scope_id: String,
}

impl RuntimeStateScopeIndex {
    fn empty(scope_id: &str) -> Self {
        Self {
            version: SCOPE_INDEX_VERSION,
            scope_id: scope_id.to_string(),
            runtime_state_ids: BTreeSet::new(),
        }
    }
}

/// File-backed runtime state store.
///
/// [`Self::new`] performs synchronous directory bootstrap. Construct the store
/// before entering latency-sensitive async work, or call it from a blocking
/// setup task. The [`RuntimeStateStore`] methods offload their file operations.
#[derive(Clone)]
pub struct FileRuntimeStateStore {
    base: PathBuf,
}

impl FileRuntimeStateStore {
    /// Create a file-backed state store rooted at `base/runtime_state/`.
    ///
    /// This synchronous bootstrap creates and canonicalizes the directory.
    pub fn new(base: impl AsRef<Path>) -> Result<Self, ReactError> {
        let base = base.as_ref().join("runtime_state");
        std::fs::create_dir_all(&base)
            .map_err(|e| ReactError::Other(format!("create runtime_state dir: {e}")))?;
        let base = std::fs::canonicalize(&base)
            .map_err(|e| ReactError::Other(format!("canonicalize runtime_state dir: {e}")))?;
        Ok(Self { base })
    }

    fn conv_dir(&self, conversation_id: &str) -> Result<PathBuf, ReactError> {
        let safe = safe_segment(conversation_id)?;
        Ok(self.base.join(safe))
    }

    fn checkpoint_path(&self, conversation_id: &str) -> Result<PathBuf, ReactError> {
        Ok(self.conv_dir(conversation_id)?.join("checkpoint.json"))
    }

    fn scope_index_path(&self, scope_id: &str) -> Result<PathBuf, ReactError> {
        let safe = safe_segment(scope_id)?;
        Ok(self.base.join("_scope_index").join(format!("{safe}.json")))
    }

    fn runtime_owner_path(&self, runtime_state_id: &str) -> Result<PathBuf, ReactError> {
        let safe = safe_segment(runtime_state_id)?;
        Ok(self
            .base
            .join("_runtime_owners")
            .join(format!("{safe}.json")))
    }

    fn to_react_err(e: impl std::fmt::Display) -> ReactError {
        ReactError::Other(format!("FileRuntimeStateStore: {e}"))
    }

    fn save_checkpoint_sync(&self, checkpoint: &AgentCheckpoint) -> crate::error::Result<()> {
        let path = self.checkpoint_path(&checkpoint.conversation_id)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| ReactError::Other(format!("create dir: {error}")))?;
        }
        let json = serde_json::to_string_pretty(checkpoint)
            .map_err(|error| ReactError::Other(format!("serialize checkpoint: {error}")))?;
        echo_core::utils::fs::atomic_write(&path, json.as_bytes())
            .map_err(|error| ReactError::Other(format!("write checkpoint: {error}")))?;
        Ok(())
    }

    fn read_scope_index_sync(
        &self,
        scope_id: &str,
    ) -> crate::error::Result<RuntimeStateScopeIndex> {
        let path = self.scope_index_path(scope_id)?;
        let raw = match std::fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(RuntimeStateScopeIndex::empty(scope_id));
            }
            Err(error) => return Err(Self::to_react_err(error)),
        };
        let index: RuntimeStateScopeIndex = serde_json::from_str(&raw)
            .map_err(|error| ReactError::Other(format!("parse {}: {error}", path.display())))?;
        if index.version != SCOPE_INDEX_VERSION || index.scope_id != scope_id {
            return Err(ReactError::Other(format!(
                "runtime state scope index identity mismatch at {}",
                path.display()
            )));
        }
        for runtime_state_id in &index.runtime_state_ids {
            let _safe = safe_segment(runtime_state_id)?;
        }
        Ok(index)
    }

    fn write_scope_index_sync(&self, index: &RuntimeStateScopeIndex) -> crate::error::Result<()> {
        let path = self.scope_index_path(&index.scope_id)?;
        if index.runtime_state_ids.is_empty() {
            return match std::fs::remove_file(&path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(Self::to_react_err(error)),
            };
        }
        let parent = path.parent().ok_or_else(|| {
            ReactError::Other("runtime state scope index has no parent".to_string())
        })?;
        std::fs::create_dir_all(parent).map_err(Self::to_react_err)?;
        let raw = serde_json::to_vec_pretty(index)
            .map_err(|error| ReactError::Other(format!("serialize scope index: {error}")))?;
        echo_core::utils::fs::atomic_write(&path, &raw).map_err(Self::to_react_err)
    }

    fn read_runtime_owner_sync(
        &self,
        runtime_state_id: &str,
    ) -> crate::error::Result<Option<RuntimeStateOwner>> {
        let path = self.runtime_owner_path(runtime_state_id)?;
        let raw = match std::fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(Self::to_react_err(error)),
        };
        let owner: RuntimeStateOwner = serde_json::from_str(&raw)
            .map_err(|error| ReactError::Other(format!("parse {}: {error}", path.display())))?;
        if owner.version != SCOPE_INDEX_VERSION || owner.runtime_state_id != runtime_state_id {
            return Err(ReactError::Other(format!(
                "runtime state owner identity mismatch at {}",
                path.display()
            )));
        }
        let _safe = safe_segment(&owner.scope_id)?;
        Ok(Some(owner))
    }

    fn write_runtime_owner_sync(&self, owner: &RuntimeStateOwner) -> crate::error::Result<()> {
        let path = self.runtime_owner_path(&owner.runtime_state_id)?;
        let parent = path.parent().ok_or_else(|| {
            ReactError::Other("runtime state owner path has no parent".to_string())
        })?;
        std::fs::create_dir_all(parent).map_err(Self::to_react_err)?;
        let raw = serde_json::to_vec_pretty(owner)
            .map_err(|error| ReactError::Other(format!("serialize runtime owner: {error}")))?;
        echo_core::utils::fs::atomic_write(&path, &raw).map_err(Self::to_react_err)
    }

    fn remove_runtime_owner_sync(
        &self,
        scope_id: &str,
        runtime_state_id: &str,
    ) -> crate::error::Result<()> {
        let Some(owner) = self.read_runtime_owner_sync(runtime_state_id)? else {
            return Ok(());
        };
        if owner.scope_id != scope_id {
            return Err(ReactError::Other(format!(
                "runtime state {runtime_state_id} belongs to scope {}, not {scope_id}",
                owner.scope_id
            )));
        }
        let path = self.runtime_owner_path(runtime_state_id)?;
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(Self::to_react_err(error)),
        }
    }

    fn remove_checkpoint_sync(&self, runtime_state_id: &str) -> crate::error::Result<bool> {
        let dir = self.conv_dir(runtime_state_id)?;
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(Self::to_react_err(error)),
        }
    }

    fn checkpoint_exists_sync(&self, runtime_state_id: &str) -> crate::error::Result<bool> {
        let path = self.checkpoint_path(runtime_state_id)?;
        match std::fs::metadata(path) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(Self::to_react_err(error)),
        }
    }

    fn run_blocking<'a, T, F>(
        &'a self,
        conversation_id: String,
        operation: F,
    ) -> BoxFuture<'a, crate::error::Result<T>>
    where
        T: Send + 'static,
        F: FnOnce(Self, String) -> crate::error::Result<T> + Send + 'static,
    {
        let store = self.clone();
        Box::pin(async move {
            let safe = safe_segment(&conversation_id)?;
            let key = BlockingFileOperationKey::new(
                "runtime-state",
                store.base.clone(),
                BlockingFileOperationScope::Entity(safe),
            );
            run_keyed_file_operation(key, move || operation(store, conversation_id))
                .await
                .map_err(Self::to_react_err)?
        })
    }

    fn run_scope_blocking<'a, T, F>(
        &'a self,
        scope_id: String,
        operation: F,
    ) -> BoxFuture<'a, crate::error::Result<T>>
    where
        T: Send + 'static,
        F: FnOnce(Self, String) -> crate::error::Result<T> + Send + 'static,
    {
        let store = self.clone();
        Box::pin(async move {
            let safe = safe_segment(&scope_id)?;
            let key = BlockingFileOperationKey::new(
                "runtime-state",
                store.base.clone(),
                BlockingFileOperationScope::Entity(safe),
            );
            run_keyed_file_operation(key, move || operation(store, scope_id))
                .await
                .map_err(Self::to_react_err)?
        })
    }
}

impl RuntimeStateStore for FileRuntimeStateStore {
    fn get_checkpoint<'a>(
        &'a self,
        conversation_id: &'a str,
    ) -> BoxFuture<'a, crate::error::Result<Option<AgentCheckpoint>>> {
        self.run_blocking(
            conversation_id.to_string(),
            move |store, conversation_id| {
                let path = store.checkpoint_path(&conversation_id)?;
                match std::fs::read_to_string(&path) {
                    Ok(s) => {
                        let cp: AgentCheckpoint = serde_json::from_str(&s).map_err(|e| {
                            ReactError::Other(format!("parse {}: {e}", path.display()))
                        })?;
                        Ok(Some(cp))
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                    Err(e) => Err(Self::to_react_err(e)),
                }
            },
        )
    }

    fn save_checkpoint<'a>(
        &'a self,
        checkpoint: &'a AgentCheckpoint,
    ) -> BoxFuture<'a, crate::error::Result<()>> {
        self.save_checkpoint_for_scope(&checkpoint.conversation_id, checkpoint)
    }

    fn save_checkpoint_for_scope<'a>(
        &'a self,
        scope_id: &'a str,
        checkpoint: &'a AgentCheckpoint,
    ) -> BoxFuture<'a, crate::error::Result<()>> {
        let checkpoint = checkpoint.clone();
        self.run_scope_blocking(scope_id.to_string(), move |store, scope_id| {
            let owner = store.read_runtime_owner_sync(&checkpoint.conversation_id)?;
            if let Some(owner) = owner.as_ref()
                && owner.scope_id != scope_id
            {
                return Err(ReactError::Other(format!(
                    "runtime state {} already belongs to scope {}",
                    checkpoint.conversation_id, owner.scope_id
                )));
            }
            let mut index = store.read_scope_index_sync(&scope_id)?;
            index
                .runtime_state_ids
                .insert(checkpoint.conversation_id.clone());
            store.write_scope_index_sync(&index)?;
            if owner.is_none() {
                store.write_runtime_owner_sync(&RuntimeStateOwner {
                    version: SCOPE_INDEX_VERSION,
                    runtime_state_id: checkpoint.conversation_id.clone(),
                    scope_id,
                })?;
            }
            store.save_checkpoint_sync(&checkpoint)
        })
    }

    fn runtime_state_ids<'a>(
        &'a self,
        scope_id: &'a str,
    ) -> BoxFuture<'a, crate::error::Result<Vec<String>>> {
        self.run_scope_blocking(scope_id.to_string(), move |store, scope_id| {
            Ok(store
                .read_scope_index_sync(&scope_id)?
                .runtime_state_ids
                .into_iter()
                .collect())
        })
    }

    fn clear_runtime_state<'a>(
        &'a self,
        scope_id: &'a str,
        runtime_state_id: &'a str,
    ) -> BoxFuture<'a, crate::error::Result<RuntimeStateClearReceipt>> {
        let runtime_state_id = runtime_state_id.to_string();
        self.run_scope_blocking(scope_id.to_string(), move |store, scope_id| {
            let mut index = store.read_scope_index_sync(&scope_id)?;
            let indexed = index.runtime_state_ids.contains(&runtime_state_id);
            let checkpoint_removed = if indexed || scope_id == runtime_state_id {
                if let Some(owner) = store.read_runtime_owner_sync(&runtime_state_id)?
                    && owner.scope_id != scope_id
                {
                    return Err(ReactError::Other(format!(
                        "runtime state {runtime_state_id} belongs to scope {}",
                        owner.scope_id
                    )));
                }
                store.remove_checkpoint_sync(&runtime_state_id)?
            } else {
                false
            };
            if indexed {
                store.remove_runtime_owner_sync(&scope_id, &runtime_state_id)?;
                index.runtime_state_ids.remove(&runtime_state_id);
                store.write_scope_index_sync(&index)?;
            }
            Ok(RuntimeStateClearReceipt {
                scope_id,
                runtime_state_id,
                checkpoint_removed,
            })
        })
    }

    fn clear_runtime_state_scope<'a>(
        &'a self,
        scope_id: &'a str,
    ) -> BoxFuture<'a, crate::error::Result<RuntimeStateScopeClearReceipt>> {
        self.run_scope_blocking(scope_id.to_string(), move |store, scope_id| {
            let index = store.read_scope_index_sync(&scope_id)?;
            let mut runtime_state_ids = index.runtime_state_ids.into_iter().collect::<Vec<_>>();
            if store.checkpoint_exists_sync(&scope_id)?
                && !runtime_state_ids
                    .iter()
                    .any(|runtime_id| runtime_id == &scope_id)
            {
                runtime_state_ids.push(scope_id.clone());
                runtime_state_ids.sort();
            }
            for runtime_state_id in &runtime_state_ids {
                if let Some(owner) = store.read_runtime_owner_sync(runtime_state_id)?
                    && owner.scope_id != scope_id
                {
                    return Err(ReactError::Other(format!(
                        "runtime state {runtime_state_id} belongs to scope {}",
                        owner.scope_id
                    )));
                }
                let _removed = store.remove_checkpoint_sync(runtime_state_id)?;
                store.remove_runtime_owner_sync(&scope_id, runtime_state_id)?;
            }
            store.write_scope_index_sync(&RuntimeStateScopeIndex::empty(&scope_id))?;
            Ok(RuntimeStateScopeClearReceipt {
                scope_id,
                runtime_state_ids,
            })
        })
    }

    fn clear_conversation<'a>(
        &'a self,
        conversation_id: &'a str,
    ) -> BoxFuture<'a, crate::error::Result<()>> {
        Box::pin(async move {
            self.clear_runtime_state(conversation_id, conversation_id)
                .await
                .map(|_receipt| ())
        })
    }
}

/// Validate an id and encode its exact UTF-8 bytes as one safe path segment.
fn safe_segment(id: &str) -> Result<String, ReactError> {
    echo_core::utils::fs::encode_path_segment_identity(id)
        .map_err(|error| ReactError::Other(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::time::Duration;

    fn tmp_base() -> PathBuf {
        std::env::temp_dir().join(format!(
            "echo-file-runtime-state-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ))
    }

    fn checkpoint(runtime_state_id: &str, marker: &str) -> AgentCheckpoint {
        AgentCheckpoint {
            conversation_id: runtime_state_id.to_string(),
            messages_json: format!(r#"["{marker}"]"#),
            current_plan: None,
            active_skills: Vec::new(),
            blocked_reason: None,
            working_dir: None,
            timestamp: Utc::now(),
        }
    }

    #[tokio::test]
    async fn file_runtime_state_lifecycle() -> crate::error::Result<()> {
        let tmp = tmp_base();
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
    async fn scoped_incarnations_survive_restart_and_reset_is_sender_local()
    -> crate::error::Result<()> {
        let tmp = tmp_base();
        let store = FileRuntimeStateStore::new(&tmp)?;
        store
            .save_checkpoint_for_scope("alice", &checkpoint("alice-1", "alice one"))
            .await?;
        store
            .save_checkpoint_for_scope("alice", &checkpoint("alice-2", "alice two"))
            .await?;
        store
            .save_checkpoint_for_scope("bob", &checkpoint("bob-1", "bob one"))
            .await?;
        assert!(
            store
                .save_checkpoint_for_scope("bob", &checkpoint("alice-2", "wrong owner"))
                .await
                .is_err()
        );
        drop(store);

        let restarted = FileRuntimeStateStore::new(&tmp)?;
        assert_eq!(
            restarted.runtime_state_ids("alice").await?,
            vec!["alice-1".to_string(), "alice-2".to_string()]
        );
        let cross_sender = restarted.clear_runtime_state("alice", "bob-1").await?;
        assert!(!cross_sender.checkpoint_removed);
        assert!(restarted.get_checkpoint("bob-1").await?.is_some());
        let reset = restarted.clear_runtime_state("alice", "alice-1").await?;
        assert!(reset.checkpoint_removed);
        assert_eq!(
            restarted.runtime_state_ids("alice").await?,
            vec!["alice-2".to_string()]
        );
        assert!(restarted.get_checkpoint("alice-1").await?.is_none());
        assert!(restarted.get_checkpoint("alice-2").await?.is_some());
        assert_eq!(
            restarted
                .get_checkpoint("alice-2")
                .await?
                .map(|checkpoint| checkpoint.messages_json),
            Some(r#"["alice two"]"#.to_string())
        );
        assert!(restarted.get_checkpoint("bob-1").await?.is_some());
        assert_eq!(
            restarted.runtime_state_ids("bob").await?,
            vec!["bob-1".to_string()]
        );
        let _ = std::fs::remove_dir_all(&tmp);
        Ok(())
    }

    #[tokio::test]
    async fn reset_keeps_transcript_and_product_delete_clears_all_incarnations()
    -> crate::error::Result<()> {
        use crate::memory::{ConversationStore, FileConversationStore, NewConversation};

        let tmp = tmp_base();
        let runtime = FileRuntimeStateStore::new(&tmp)?;
        let conversations = FileConversationStore::new(&tmp)?;
        for conversation_id in ["alice", "bob"] {
            conversations
                .ensure_conversation(NewConversation {
                    conversation_id: conversation_id.to_string(),
                    user_id: "default".to_string(),
                    agent_type: None,
                    title: None,
                })
                .await?;
            let messages = crate::memory::project_messages(
                conversation_id,
                &[crate::llm::types::Message::user(format!(
                    "{conversation_id} transcript"
                ))],
            )?;
            conversations
                .save_messages(conversation_id, &messages)
                .await?;
        }
        runtime
            .save_checkpoint_for_scope("alice", &checkpoint("alice-1", "one"))
            .await?;
        runtime
            .save_checkpoint_for_scope("alice", &checkpoint("alice-2", "two"))
            .await?;
        runtime
            .save_checkpoint_for_scope("bob", &checkpoint("bob-1", "bob"))
            .await?;

        for runtime_state_id in ["alice-1", "bob-1"] {
            conversations
                .ensure_conversation(NewConversation {
                    conversation_id: runtime_state_id.to_string(),
                    user_id: "default".to_string(),
                    agent_type: None,
                    title: None,
                })
                .await?;
            let incarnation_messages = crate::memory::project_messages(
                runtime_state_id,
                &[crate::llm::types::Message::user(format!(
                    "{runtime_state_id} incarnation-only transcript"
                ))],
            )?;
            conversations
                .save_messages(runtime_state_id, &incarnation_messages)
                .await?;
        }

        assert!(
            super::super::clear_persisted_runtime_incarnation(
                &conversations,
                &runtime,
                "alice",
                "bob-1",
            )
            .await
            .is_err()
        );
        assert!(conversations.get_conversation("bob-1").await?.is_some());

        super::super::clear_persisted_runtime_incarnation(
            &conversations,
            &runtime,
            "alice",
            "alice-1",
        )
        .await?;
        assert_eq!(conversations.count_messages("alice").await?, 1);
        assert!(conversations.get_conversation("alice-1").await?.is_none());

        let deleted =
            super::super::delete_persisted_conversation(&conversations, &runtime, "alice").await?;
        assert_eq!(deleted.runtime_state_ids, vec!["alice-2".to_string()]);
        assert!(conversations.get_conversation("alice").await?.is_none());
        assert!(runtime.get_checkpoint("alice-2").await?.is_none());
        assert!(runtime.runtime_state_ids("alice").await?.is_empty());
        assert!(conversations.get_conversation("bob").await?.is_some());
        assert!(conversations.get_conversation("bob-1").await?.is_some());
        assert!(runtime.get_checkpoint("bob-1").await?.is_some());

        drop(conversations);
        let _ = std::fs::remove_dir_all(&tmp);
        Ok(())
    }

    #[tokio::test]
    async fn crash_tombstone_is_recoverable_after_restart() -> crate::error::Result<()> {
        let tmp = tmp_base();
        let store = FileRuntimeStateStore::new(&tmp)?;
        let mut index = RuntimeStateScopeIndex::empty("crash-scope");
        index
            .runtime_state_ids
            .insert("missing-after-crash".to_string());
        store.write_scope_index_sync(&index)?;
        drop(store);

        let restarted = FileRuntimeStateStore::new(&tmp)?;
        assert_eq!(
            restarted.runtime_state_ids("crash-scope").await?,
            vec!["missing-after-crash".to_string()]
        );
        let receipt = restarted.clear_runtime_state_scope("crash-scope").await?;
        assert_eq!(
            receipt.runtime_state_ids,
            vec!["missing-after-crash".to_string()]
        );
        assert!(restarted.runtime_state_ids("crash-scope").await?.is_empty());
        let _ = std::fs::remove_dir_all(&tmp);
        Ok(())
    }

    #[tokio::test]
    async fn corrupt_checkpoint_surfaces_as_error() -> crate::error::Result<()> {
        let tmp = tmp_base();
        let store = FileRuntimeStateStore::new(&tmp)?;
        let path = store.checkpoint_path("c1")?;
        let parent = path.parent().ok_or_else(|| {
            ReactError::Other("checkpoint path has no parent directory".to_string())
        })?;
        std::fs::create_dir_all(parent).map_err(|error| ReactError::Other(error.to_string()))?;
        std::fs::write(path, b"{ not valid json")
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
        let tmp = tmp_base();
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

    #[tokio::test]
    async fn exact_utf8_ids_do_not_alias_on_case_folding_filesystems() -> crate::error::Result<()> {
        let tmp = tmp_base();
        let store = FileRuntimeStateStore::new(&tmp)?;
        let upper = AgentCheckpoint {
            conversation_id: "A".to_string(),
            messages_json: "[\"upper\"]".to_string(),
            current_plan: None,
            active_skills: Vec::new(),
            blocked_reason: None,
            working_dir: None,
            timestamp: Utc::now(),
        };
        let lower = AgentCheckpoint {
            conversation_id: "a".to_string(),
            messages_json: "[\"lower\"]".to_string(),
            ..upper.clone()
        };
        let composed = AgentCheckpoint {
            conversation_id: "é".to_string(),
            messages_json: "[\"composed\"]".to_string(),
            ..upper.clone()
        };
        let decomposed = AgentCheckpoint {
            conversation_id: "e\u{301}".to_string(),
            messages_json: "[\"decomposed\"]".to_string(),
            ..upper.clone()
        };
        let paths = [
            store.checkpoint_path(&upper.conversation_id)?,
            store.checkpoint_path(&lower.conversation_id)?,
            store.checkpoint_path(&composed.conversation_id)?,
            store.checkpoint_path(&decomposed.conversation_id)?,
        ];
        let unique = paths.iter().collect::<std::collections::HashSet<_>>();
        assert_eq!(unique.len(), paths.len());

        tokio::try_join!(
            store.save_checkpoint(&upper),
            store.save_checkpoint(&lower),
            store.save_checkpoint(&composed),
            store.save_checkpoint(&decomposed),
        )?;
        for expected in [&upper, &lower, &composed, &decomposed] {
            let loaded = store
                .get_checkpoint(&expected.conversation_id)
                .await?
                .ok_or_else(|| ReactError::Other("aliased checkpoint is missing".to_string()))?;
            assert_eq!(loaded.conversation_id, expected.conversation_id);
            assert_eq!(loaded.messages_json, expected.messages_json);
        }
        let _ = std::fs::remove_dir_all(&tmp);
        Ok(())
    }

    #[tokio::test]
    async fn aborted_caller_does_not_cancel_accepted_checkpoint_write() -> crate::error::Result<()>
    {
        let tmp = tmp_base();
        let store = FileRuntimeStateStore::new(&tmp)?;
        let caller_store = store.clone();
        let checkpoint = AgentCheckpoint {
            conversation_id: "abort".to_string(),
            messages_json: "[\"committed\"]".to_string(),
            current_plan: None,
            active_skills: Vec::new(),
            blocked_reason: None,
            working_dir: None,
            timestamp: Utc::now(),
        };
        let checkpoint_for_write = checkpoint.clone();
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let caller = tokio::spawn(async move {
            caller_store
                .run_blocking("abort".to_string(), move |store, _| {
                    let _ignored = entered_tx.send(());
                    release_rx
                        .recv_timeout(Duration::from_secs(2))
                        .map_err(|error| {
                            ReactError::Other(format!("release checkpoint write: {error}"))
                        })?;
                    store.save_checkpoint_sync(&checkpoint_for_write)
                })
                .await
        });
        entered_rx
            .await
            .map_err(FileRuntimeStateStore::to_react_err)?;
        caller.abort();
        release_tx
            .send(())
            .map_err(FileRuntimeStateStore::to_react_err)?;

        let loaded = tokio::time::timeout(
            Duration::from_secs(2),
            store.get_checkpoint(&checkpoint.conversation_id),
        )
        .await
        .map_err(|_| ReactError::Other("accepted checkpoint write did not settle".to_string()))??
        .ok_or_else(|| ReactError::Other("accepted checkpoint write disappeared".to_string()))?;
        assert_eq!(loaded.messages_json, checkpoint.messages_json);
        let _ = std::fs::remove_dir_all(&tmp);
        Ok(())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn save_clear_save_preserves_exact_fifo_after_caller_abort() -> crate::error::Result<()> {
        let tmp = tmp_base();
        let store = FileRuntimeStateStore::new(&tmp)?;
        let first_checkpoint = AgentCheckpoint {
            conversation_id: "aba".to_string(),
            messages_json: "[\"first\"]".to_string(),
            current_plan: None,
            active_skills: Vec::new(),
            blocked_reason: None,
            working_dir: None,
            timestamp: Utc::now(),
        };
        let final_checkpoint = AgentCheckpoint {
            messages_json: "[\"final\"]".to_string(),
            timestamp: Utc::now(),
            ..first_checkpoint.clone()
        };
        let first_store = store.clone();
        let first_for_operation = first_checkpoint.clone();
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let first = tokio::spawn(async move {
            first_store
                .run_blocking("aba".to_string(), move |store, _| {
                    let _ignored = entered_tx.send(());
                    release_rx
                        .recv_timeout(Duration::from_secs(2))
                        .map_err(FileRuntimeStateStore::to_react_err)?;
                    store.save_checkpoint_sync(&first_for_operation)
                })
                .await
        });
        entered_rx
            .await
            .map_err(FileRuntimeStateStore::to_react_err)?;
        first.abort();

        let clear_store = store.clone();
        let clear = tokio::spawn(async move { clear_store.clear_conversation("aba").await });
        tokio::task::yield_now().await;
        assert!(!clear.is_finished());
        let final_store = store.clone();
        let final_for_operation = final_checkpoint.clone();
        let final_save =
            tokio::spawn(async move { final_store.save_checkpoint(&final_for_operation).await });
        tokio::task::yield_now().await;
        assert!(!clear.is_finished());
        assert!(!final_save.is_finished());

        release_tx
            .send(())
            .map_err(FileRuntimeStateStore::to_react_err)?;
        clear.await.map_err(FileRuntimeStateStore::to_react_err)??;
        final_save
            .await
            .map_err(FileRuntimeStateStore::to_react_err)??;
        let loaded = store
            .get_checkpoint("aba")
            .await?
            .ok_or_else(|| ReactError::Other("final checkpoint is missing".to_string()))?;
        assert_eq!(loaded.messages_json, final_checkpoint.messages_json);
        for _ in 0..16 {
            tokio::task::yield_now().await;
        }
        let stable = store
            .get_checkpoint("aba")
            .await?
            .ok_or_else(|| ReactError::Other("stable checkpoint is missing".to_string()))?;
        assert_eq!(stable.messages_json, final_checkpoint.messages_json);
        let _ = std::fs::remove_dir_all(&tmp);
        Ok(())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn corrupt_read_and_clear_are_ordered_for_one_conversation() -> crate::error::Result<()> {
        let tmp = tmp_base();
        let store = FileRuntimeStateStore::new(&tmp)?;
        let path = store.checkpoint_path("corrupt")?;
        let parent = path.parent().ok_or_else(|| {
            ReactError::Other("checkpoint path has no parent directory".to_string())
        })?;
        std::fs::create_dir_all(parent).map_err(FileRuntimeStateStore::to_react_err)?;
        std::fs::write(&path, b"{ invalid json").map_err(FileRuntimeStateStore::to_react_err)?;

        let blocker_store = store.clone();
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let blocker = tokio::spawn(async move {
            blocker_store
                .run_blocking("corrupt".to_string(), move |_, _| {
                    let _ignored = entered_tx.send(());
                    release_rx
                        .recv_timeout(Duration::from_secs(2))
                        .map_err(FileRuntimeStateStore::to_react_err)
                })
                .await
        });
        entered_rx
            .await
            .map_err(FileRuntimeStateStore::to_react_err)?;
        let read_store = store.clone();
        let read = tokio::spawn(async move { read_store.get_checkpoint("corrupt").await });
        tokio::task::yield_now().await;
        let clear_store = store.clone();
        let clear = tokio::spawn(async move { clear_store.clear_conversation("corrupt").await });
        tokio::task::yield_now().await;
        release_tx
            .send(())
            .map_err(FileRuntimeStateStore::to_react_err)?;

        blocker
            .await
            .map_err(FileRuntimeStateStore::to_react_err)??;
        let read_error = read
            .await
            .map_err(FileRuntimeStateStore::to_react_err)?
            .err()
            .ok_or_else(|| ReactError::Other("corrupt checkpoint was accepted".to_string()))?;
        assert!(read_error.to_string().contains("parse"));
        clear.await.map_err(FileRuntimeStateStore::to_react_err)??;
        assert!(store.get_checkpoint("corrupt").await?.is_none());
        let _ = std::fs::remove_dir_all(&tmp);
        Ok(())
    }

    #[tokio::test]
    async fn different_conversations_can_use_distinct_blocking_slots() -> crate::error::Result<()> {
        let tmp = tmp_base();
        let store = FileRuntimeStateStore::new(&tmp)?;
        let first_store = store.clone();
        let second_store = store.clone();
        let (first_entered_tx, first_entered_rx) = tokio::sync::oneshot::channel();
        let (second_entered_tx, second_entered_rx) = tokio::sync::oneshot::channel();
        let (first_release_tx, first_release_rx) = std::sync::mpsc::channel();
        let (second_release_tx, second_release_rx) = std::sync::mpsc::channel();
        let first = tokio::spawn(async move {
            first_store
                .run_blocking("first".to_string(), move |_, _| {
                    let _ignored = first_entered_tx.send(());
                    first_release_rx
                        .recv_timeout(Duration::from_secs(2))
                        .map_err(FileRuntimeStateStore::to_react_err)
                })
                .await
        });
        let second = tokio::spawn(async move {
            second_store
                .run_blocking("second".to_string(), move |_, _| {
                    let _ignored = second_entered_tx.send(());
                    second_release_rx
                        .recv_timeout(Duration::from_secs(2))
                        .map_err(FileRuntimeStateStore::to_react_err)
                })
                .await
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            first_entered_rx
                .await
                .map_err(FileRuntimeStateStore::to_react_err)?;
            second_entered_rx
                .await
                .map_err(FileRuntimeStateStore::to_react_err)
        })
        .await
        .map_err(|_| ReactError::Other("distinct conversations were serialized".to_string()))??;
        first_release_tx
            .send(())
            .map_err(FileRuntimeStateStore::to_react_err)?;
        second_release_tx
            .send(())
            .map_err(FileRuntimeStateStore::to_react_err)?;
        first.await.map_err(FileRuntimeStateStore::to_react_err)??;
        second
            .await
            .map_err(FileRuntimeStateStore::to_react_err)??;
        let _ = std::fs::remove_dir_all(&tmp);
        Ok(())
    }
}
