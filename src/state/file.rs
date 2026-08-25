//! File-backed [`RuntimeStateStore`] — a no-dependency JSON-file backend.
//!
//! One record per runtime identity under
//! `<base>/runtime_state/_runtime_owners/<safe_id>.json`. The record owns the
//! stable scope binding, lifecycle phase, and checkpoint atomically. The
//! `_scope_index` files are rebuildable projections, never deletion authority.
//!
//! This is the no-SQLite alternative to [`SqliteRuntimeStateStore`](crate::state::sqlite::SqliteRuntimeStateStore)
//! (`sqlite` feature). Suitable for a single-process local agent (typical
//! echo-agent consumer). For multi-process concurrency, use the SQLite backend.
//!
//! ## Robustness
//!
//! - **Path-safe ids.** Conversation ids are sanitized before joining into the
//!   path (rejecting `/`, `\`, `..`, empty) to prevent directory escapes.
//! - **Corrupt JSON is an error.** A malformed owner record surfaces as
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
use echo_core::utils::fs::{
    ExclusiveFileLease, create_dir_all_durable, remove_file_durable, try_exclusive_file_lease,
};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use super::{
    AgentCheckpoint, RuntimeStateClearReceipt, RuntimeStateScopeClearReceipt, RuntimeStateStore,
};

const SCOPE_INDEX_VERSION: u8 = 1;
const RUNTIME_STATE_RECORD_VERSION: u8 = 1;
const RUNTIME_STATE_SHARDS: usize = 64;

#[derive(Debug, Serialize, Deserialize)]
struct RuntimeStateScopeIndex {
    version: u8,
    scope_id: String,
    runtime_state_ids: BTreeSet<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RuntimeStatePhase {
    Active,
    Deleting,
}

#[derive(Debug, Serialize, Deserialize)]
struct RuntimeStateOwner {
    version: u8,
    runtime_state_id: String,
    scope_id: String,
    phase: RuntimeStatePhase,
    checkpoint: Option<AgentCheckpoint>,
}

/// File-backed runtime state store.
///
/// [`Self::new`] performs synchronous directory bootstrap. Construct the store
/// before entering latency-sensitive async work, or call it from a blocking
/// setup task. The [`RuntimeStateStore`] methods offload their file operations.
struct FileRuntimeStateAuthority {
    shards: [Mutex<()>; RUNTIME_STATE_SHARDS],
    _lease: ExclusiveFileLease,
}

impl FileRuntimeStateAuthority {
    fn new(lease: ExclusiveFileLease) -> Self {
        Self {
            shards: std::array::from_fn(|_| Mutex::new(())),
            _lease: lease,
        }
    }
}

fn runtime_state_authorities() -> &'static Mutex<HashMap<PathBuf, Weak<FileRuntimeStateAuthority>>>
{
    static REGISTRY: OnceLock<Mutex<HashMap<PathBuf, Weak<FileRuntimeStateAuthority>>>> =
        OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Clone)]
pub struct FileRuntimeStateStore {
    base: PathBuf,
    authority: Arc<FileRuntimeStateAuthority>,
}

impl FileRuntimeStateStore {
    /// Create a file-backed state store rooted at `base/runtime_state/`.
    ///
    /// This synchronous bootstrap creates and canonicalizes the directory.
    pub fn new(base: impl AsRef<Path>) -> Result<Self, ReactError> {
        let base = base.as_ref().join("runtime_state");
        create_dir_all_durable(&base)
            .map_err(|e| ReactError::Other(format!("create runtime_state dir: {e}")))?;
        create_dir_all_durable(&base.join("_runtime_owners"))
            .map_err(|e| ReactError::Other(format!("create runtime owners dir: {e}")))?;
        create_dir_all_durable(&base.join("_scope_index"))
            .map_err(|e| ReactError::Other(format!("create runtime scope dir: {e}")))?;
        let base = std::fs::canonicalize(&base)
            .map_err(|e| ReactError::Other(format!("canonicalize runtime_state dir: {e}")))?;
        let mut registry = runtime_state_authorities()
            .lock()
            .map_err(|error| ReactError::Other(format!("runtime authority poisoned: {error}")))?;
        let authority = match registry.get(&base).and_then(Weak::upgrade) {
            Some(authority) => authority,
            None => {
                let lease = try_exclusive_file_lease(&base).map_err(Self::to_react_err)?;
                let authority = Arc::new(FileRuntimeStateAuthority::new(lease));
                registry.insert(base.clone(), Arc::downgrade(&authority));
                authority
            }
        };
        Ok(Self { base, authority })
    }

    #[cfg(test)]
    fn checkpoint_path(&self, conversation_id: &str) -> Result<PathBuf, ReactError> {
        self.runtime_owner_path(conversation_id)
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

    fn runtime_shard(runtime_state_id: &str) -> usize {
        let hash = runtime_state_id
            .as_bytes()
            .iter()
            .fold(0_u64, |hash, byte| {
                hash.wrapping_mul(1099511628211)
                    .wrapping_add(u64::from(*byte))
            });
        usize::try_from(hash % u64::try_from(RUNTIME_STATE_SHARDS).unwrap_or(1)).unwrap_or(0)
    }

    fn lock_runtime(
        &self,
        runtime_state_id: &str,
    ) -> crate::error::Result<std::sync::MutexGuard<'_, ()>> {
        self.authority
            .shards
            .get(Self::runtime_shard(runtime_state_id))
            .ok_or_else(|| Self::to_react_err("runtime shard index is out of bounds"))?
            .lock()
            .map_err(|error| Self::to_react_err(format!("runtime shard poisoned: {error}")))
    }

    fn lock_all_runtime_shards(&self) -> crate::error::Result<Vec<std::sync::MutexGuard<'_, ()>>> {
        let mut guards = Vec::with_capacity(RUNTIME_STATE_SHARDS);
        for shard in &self.authority.shards {
            guards.push(
                shard.lock().map_err(|error| {
                    Self::to_react_err(format!("runtime shard poisoned: {error}"))
                })?,
            );
        }
        Ok(guards)
    }

    fn to_react_err(e: impl std::fmt::Display) -> ReactError {
        ReactError::Other(format!("FileRuntimeStateStore: {e}"))
    }

    #[cfg(test)]
    fn save_checkpoint_sync(&self, checkpoint: &AgentCheckpoint) -> crate::error::Result<()> {
        self.write_runtime_owner_sync(&RuntimeStateOwner {
            version: RUNTIME_STATE_RECORD_VERSION,
            runtime_state_id: checkpoint.conversation_id.clone(),
            scope_id: checkpoint.conversation_id.clone(),
            phase: RuntimeStatePhase::Active,
            checkpoint: Some(checkpoint.clone()),
        })
    }

    fn write_scope_index_sync(&self, index: &RuntimeStateScopeIndex) -> crate::error::Result<()> {
        let path = self.scope_index_path(&index.scope_id)?;
        if index.runtime_state_ids.is_empty() {
            return remove_file_durable(&path)
                .map(|_removed| ())
                .map_err(Self::to_react_err);
        }
        let parent = path.parent().ok_or_else(|| {
            ReactError::Other("runtime state scope index has no parent".to_string())
        })?;
        create_dir_all_durable(parent).map_err(Self::to_react_err)?;
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
        if owner.version != RUNTIME_STATE_RECORD_VERSION
            || owner.runtime_state_id != runtime_state_id
        {
            return Err(ReactError::Other(format!(
                "runtime state owner identity mismatch at {}",
                path.display()
            )));
        }
        let _safe = safe_segment(&owner.scope_id)?;
        match owner.phase {
            RuntimeStatePhase::Active => {
                if owner
                    .checkpoint
                    .as_ref()
                    .is_none_or(|checkpoint| checkpoint.conversation_id != runtime_state_id)
                {
                    return Err(ReactError::Other(format!(
                        "active runtime state record is missing checkpoint at {}",
                        path.display()
                    )));
                }
            }
            RuntimeStatePhase::Deleting => {
                if owner.checkpoint.is_some() {
                    return Err(ReactError::Other(format!(
                        "deleting runtime state record retained checkpoint at {}",
                        path.display()
                    )));
                }
            }
        }
        Ok(Some(owner))
    }

    fn write_runtime_owner_sync(&self, owner: &RuntimeStateOwner) -> crate::error::Result<()> {
        let _safe_scope = safe_segment(&owner.scope_id)?;
        let path = self.runtime_owner_path(&owner.runtime_state_id)?;
        let parent = path.parent().ok_or_else(|| {
            ReactError::Other("runtime state owner path has no parent".to_string())
        })?;
        create_dir_all_durable(parent).map_err(Self::to_react_err)?;
        let raw = serde_json::to_vec_pretty(owner)
            .map_err(|error| ReactError::Other(format!("serialize runtime owner: {error}")))?;
        echo_core::utils::fs::atomic_write(&path, &raw).map_err(Self::to_react_err)
    }

    fn mark_runtime_deleting_sync(
        &self,
        scope_id: &str,
        runtime_state_id: &str,
    ) -> crate::error::Result<bool> {
        let Some(owner) = self.read_runtime_owner_sync(runtime_state_id)? else {
            return Ok(false);
        };
        if owner.scope_id != scope_id {
            return Err(ReactError::Other(format!(
                "runtime state {runtime_state_id} belongs to scope {}, not {scope_id}",
                owner.scope_id
            )));
        }
        if owner.phase == RuntimeStatePhase::Active {
            self.write_runtime_owner_sync(&RuntimeStateOwner {
                version: RUNTIME_STATE_RECORD_VERSION,
                runtime_state_id: runtime_state_id.to_string(),
                scope_id: scope_id.to_string(),
                phase: RuntimeStatePhase::Deleting,
                checkpoint: None,
            })?;
        }
        Ok(true)
    }

    fn remove_runtime_record_sync(&self, runtime_state_id: &str) -> crate::error::Result<bool> {
        remove_file_durable(&self.runtime_owner_path(runtime_state_id)?).map_err(Self::to_react_err)
    }

    fn runtime_records_sync(&self) -> crate::error::Result<Vec<RuntimeStateOwner>> {
        let root = self.base.join("_runtime_owners");
        let mut records = Vec::new();
        for entry in std::fs::read_dir(&root).map_err(Self::to_react_err)? {
            let entry = entry.map_err(Self::to_react_err)?;
            let path = entry.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
                continue;
            }
            let raw = std::fs::read_to_string(&path).map_err(Self::to_react_err)?;
            let record: RuntimeStateOwner = serde_json::from_str(&raw)
                .map_err(|error| ReactError::Other(format!("parse {}: {error}", path.display())))?;
            if self.runtime_owner_path(&record.runtime_state_id)? != path {
                return Err(ReactError::Other(format!(
                    "runtime state record filename does not match identity: {}",
                    path.display()
                )));
            }
            let validated = self
                .read_runtime_owner_sync(&record.runtime_state_id)?
                .ok_or_else(|| {
                    ReactError::Other(format!(
                        "runtime state record disappeared: {}",
                        path.display()
                    ))
                })?;
            records.push(validated);
        }
        records.sort_by(|left, right| left.runtime_state_id.cmp(&right.runtime_state_id));
        Ok(records)
    }

    fn scope_runtime_state_ids(scope_id: &str, records: &[RuntimeStateOwner]) -> Vec<String> {
        records
            .iter()
            .filter(|record| record.scope_id == scope_id)
            .map(|record| record.runtime_state_id.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    fn reconcile_scope_index_sync(
        &self,
        scope_id: &str,
        records: &[RuntimeStateOwner],
    ) -> crate::error::Result<()> {
        let runtime_state_ids = Self::scope_runtime_state_ids(scope_id, records);
        self.write_scope_index_sync(&RuntimeStateScopeIndex {
            version: SCOPE_INDEX_VERSION,
            scope_id: scope_id.to_string(),
            runtime_state_ids: runtime_state_ids.into_iter().collect(),
        })
    }

    fn repair_scope_index_best_effort(&self, scope_id: &str) {
        let repaired = self
            .runtime_records_sync()
            .and_then(|records| self.reconcile_scope_index_sync(scope_id, &records));
        if let Err(error) = repaired {
            tracing::warn!(
                %scope_id,
                %error,
                "runtime scope projection repair failed; owner records remain authoritative"
            );
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
                BlockingFileOperationScope::Collection(safe),
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
                let _runtime = store.lock_runtime(&conversation_id)?;
                let Some(record) = store.read_runtime_owner_sync(&conversation_id)? else {
                    return Ok(None);
                };
                match record.phase {
                    RuntimeStatePhase::Active => Ok(record.checkpoint),
                    RuntimeStatePhase::Deleting => {
                        let _removed = store.remove_runtime_record_sync(&conversation_id)?;
                        Ok(None)
                    }
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
        let runtime_state_id = checkpoint.conversation_id.clone();
        let scope_id = scope_id.to_string();
        self.run_blocking(runtime_state_id, move |store, runtime_state_id| {
            let _safe_scope = safe_segment(&scope_id)?;
            let _runtime = store.lock_runtime(&runtime_state_id)?;
            if let Some(record) = store.read_runtime_owner_sync(&runtime_state_id)? {
                match record.phase {
                    RuntimeStatePhase::Active if record.scope_id != scope_id => {
                        return Err(ReactError::Other(format!(
                            "runtime state {runtime_state_id} already belongs to scope {}",
                            record.scope_id
                        )));
                    }
                    RuntimeStatePhase::Deleting => {
                        let _removed = store.remove_runtime_record_sync(&runtime_state_id)?;
                    }
                    RuntimeStatePhase::Active => {}
                }
            }
            store.write_runtime_owner_sync(&RuntimeStateOwner {
                version: RUNTIME_STATE_RECORD_VERSION,
                runtime_state_id: runtime_state_id.clone(),
                scope_id: scope_id.clone(),
                phase: RuntimeStatePhase::Active,
                checkpoint: Some(checkpoint),
            })?;
            store.repair_scope_index_best_effort(&scope_id);
            Ok(())
        })
    }

    fn runtime_state_ids<'a>(
        &'a self,
        scope_id: &'a str,
    ) -> BoxFuture<'a, crate::error::Result<Vec<String>>> {
        self.run_scope_blocking(scope_id.to_string(), move |store, scope_id| {
            let _runtime_shards = store.lock_all_runtime_shards()?;
            let records = store.runtime_records_sync()?;
            let runtime_state_ids = Self::scope_runtime_state_ids(&scope_id, &records);
            if let Err(error) = store.reconcile_scope_index_sync(&scope_id, &records) {
                tracing::warn!(
                    %scope_id,
                    %error,
                    "runtime scope projection repair failed during authoritative enumeration"
                );
            }
            Ok(runtime_state_ids)
        })
    }

    fn clear_runtime_state<'a>(
        &'a self,
        scope_id: &'a str,
        runtime_state_id: &'a str,
    ) -> BoxFuture<'a, crate::error::Result<RuntimeStateClearReceipt>> {
        let runtime_state_id = runtime_state_id.to_string();
        let scope_id = scope_id.to_string();
        self.run_blocking(runtime_state_id, move |store, runtime_state_id| {
            let _safe_scope = safe_segment(&scope_id)?;
            let _runtime = store.lock_runtime(&runtime_state_id)?;
            let checkpoint_removed =
                store.mark_runtime_deleting_sync(&scope_id, &runtime_state_id)?;
            let _removed = store.remove_runtime_record_sync(&runtime_state_id)?;
            store.repair_scope_index_best_effort(&scope_id);
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
            let _runtime_shards = store.lock_all_runtime_shards()?;
            let records = store.runtime_records_sync()?;
            let runtime_state_ids = records
                .iter()
                .filter(|record| record.scope_id == scope_id)
                .map(|record| record.runtime_state_id.clone())
                .collect::<Vec<_>>();
            for runtime_state_id in &runtime_state_ids {
                let _marked = store.mark_runtime_deleting_sync(&scope_id, runtime_state_id)?;
            }
            for runtime_state_id in &runtime_state_ids {
                let _removed = store.remove_runtime_record_sync(runtime_state_id)?;
            }
            store.repair_scope_index_best_effort(&scope_id);
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
        assert!(
            restarted
                .clear_runtime_state("alice", "bob-1")
                .await
                .is_err()
        );
        assert!(
            restarted
                .clear_runtime_state("bob-1", "bob-1")
                .await
                .is_err()
        );
        assert!(
            restarted
                .clear_runtime_state_scope("bob-1")
                .await?
                .runtime_state_ids
                .is_empty()
        );
        assert!(restarted.clear_conversation("bob-1").await.is_err());
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

        restarted
            .save_checkpoint_for_scope("scope-a", &checkpoint("scope-a-1", "owned by a"))
            .await?;
        restarted
            .save_checkpoint_for_scope("scope-b", &checkpoint("scope-a", "owned by b"))
            .await?;
        let same_name = restarted.clear_runtime_state_scope("scope-a").await?;
        assert_eq!(same_name.runtime_state_ids, vec!["scope-a-1".to_string()]);
        assert!(restarted.get_checkpoint("scope-a-1").await?.is_none());
        assert!(restarted.get_checkpoint("scope-a").await?.is_some());
        assert_eq!(
            restarted.runtime_state_ids("scope-b").await?,
            vec!["scope-a".to_string()]
        );
        let _ = std::fs::remove_dir_all(&tmp);
        Ok(())
    }

    #[tokio::test]
    async fn concurrent_scopes_cannot_claim_one_runtime_identity() -> crate::error::Result<()> {
        let tmp = tmp_base();
        let store = FileRuntimeStateStore::new(&tmp)?;
        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(3));
        let first_store = store.clone();
        let first_barrier = std::sync::Arc::clone(&barrier);
        let first = tokio::spawn(async move {
            first_barrier.wait().await;
            first_store
                .save_checkpoint_for_scope("scope-a", &checkpoint("shared-runtime", "a"))
                .await
        });
        // A separately constructed handle for the same canonical root must
        // share the fixed process authority, not create per-ID lock files.
        let second_store = FileRuntimeStateStore::new(&tmp)?;
        let second_barrier = std::sync::Arc::clone(&barrier);
        let second = tokio::spawn(async move {
            second_barrier.wait().await;
            second_store
                .save_checkpoint_for_scope("scope-b", &checkpoint("shared-runtime", "b"))
                .await
        });
        barrier.wait().await;
        let first = first.await.map_err(FileRuntimeStateStore::to_react_err)?;
        let second = second.await.map_err(FileRuntimeStateStore::to_react_err)?;
        assert_ne!(first.is_ok(), second.is_ok());

        let scope_a = store.runtime_state_ids("scope-a").await?;
        let scope_b = store.runtime_state_ids("scope-b").await?;
        assert_eq!(scope_a.len().saturating_add(scope_b.len()), 1);
        assert!(
            scope_a.first().map(String::as_str) == Some("shared-runtime")
                || scope_b.first().map(String::as_str) == Some("shared-runtime")
        );
        assert!(store.get_checkpoint("shared-runtime").await?.is_some());
        assert!(!tmp.join("runtime_state").join("_locks").exists());
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
    async fn every_phase_cut_recovers_and_preserves_exact_owner() -> crate::error::Result<()> {
        let tmp = tmp_base();
        let store = FileRuntimeStateStore::new(&tmp)?;
        let active = checkpoint("active-cut", "active");
        store.write_runtime_owner_sync(&RuntimeStateOwner {
            version: RUNTIME_STATE_RECORD_VERSION,
            runtime_state_id: "active-cut".to_string(),
            scope_id: "scope-a".to_string(),
            phase: RuntimeStatePhase::Active,
            checkpoint: Some(active),
        })?;
        drop(store);

        let restarted = FileRuntimeStateStore::new(&tmp)?;
        assert_eq!(
            restarted.runtime_state_ids("scope-a").await?,
            vec!["active-cut".to_string()]
        );
        assert!(
            restarted
                .save_checkpoint_for_scope("scope-b", &checkpoint("active-cut", "wrong"))
                .await
                .is_err()
        );

        restarted
            .save_checkpoint_for_scope("scope-a", &checkpoint("deleting-cut", "delete"))
            .await?;
        restarted.mark_runtime_deleting_sync("scope-a", "deleting-cut")?;
        drop(restarted);
        let after_delete_mark = FileRuntimeStateStore::new(&tmp)?;
        assert!(
            after_delete_mark
                .get_checkpoint("deleting-cut")
                .await?
                .is_none()
        );
        assert_eq!(
            after_delete_mark.runtime_state_ids("scope-a").await?,
            vec!["active-cut".to_string()]
        );
        after_delete_mark
            .save_checkpoint_for_scope("scope-b", &checkpoint("deleting-cut", "reclaimed"))
            .await?;

        after_delete_mark
            .save_checkpoint_for_scope("scope-a", &checkpoint("deleting-claim", "delete"))
            .await?;
        after_delete_mark.mark_runtime_deleting_sync("scope-a", "deleting-claim")?;
        drop(after_delete_mark);
        let after_deleting_claim = FileRuntimeStateStore::new(&tmp)?;
        after_deleting_claim
            .save_checkpoint_for_scope("scope-b", &checkpoint("deleting-claim", "direct reclaim"))
            .await?;

        after_deleting_claim
            .save_checkpoint_for_scope("scope-a", &checkpoint("unlinked-cut", "unlink"))
            .await?;
        assert!(after_deleting_claim.remove_runtime_record_sync("unlinked-cut")?);
        drop(after_deleting_claim);
        let after_unlink = FileRuntimeStateStore::new(&tmp)?;
        assert_eq!(
            after_unlink.runtime_state_ids("scope-a").await?,
            vec!["active-cut".to_string()]
        );
        after_unlink
            .save_checkpoint_for_scope("scope-b", &checkpoint("unlinked-cut", "new owner"))
            .await?;
        assert!(after_unlink.get_checkpoint("active-cut").await?.is_some());
        assert!(after_unlink.get_checkpoint("deleting-cut").await?.is_some());
        assert!(
            after_unlink
                .get_checkpoint("deleting-claim")
                .await?
                .is_some()
        );
        assert!(after_unlink.get_checkpoint("unlinked-cut").await?.is_some());
        let _ = std::fs::remove_dir_all(&tmp);
        Ok(())
    }

    #[tokio::test]
    async fn corrupt_scope_projection_never_overrides_owner_authority() -> crate::error::Result<()>
    {
        let tmp = tmp_base();
        let store = FileRuntimeStateStore::new(&tmp)?;
        store
            .save_checkpoint_for_scope("scope-a", &checkpoint("runtime-a", "a"))
            .await?;
        let projection = store.scope_index_path("scope-a")?;
        echo_core::utils::fs::atomic_write(&projection, b"{ corrupt projection")
            .map_err(FileRuntimeStateStore::to_react_err)?;

        store
            .save_checkpoint_for_scope("scope-a", &checkpoint("runtime-b", "b"))
            .await?;
        assert_eq!(
            store.runtime_state_ids("scope-a").await?,
            vec!["runtime-a".to_string(), "runtime-b".to_string()]
        );

        echo_core::utils::fs::atomic_write(&projection, b"{ corrupt projection")
            .map_err(FileRuntimeStateStore::to_react_err)?;
        assert!(
            store
                .clear_runtime_state("scope-a", "runtime-a")
                .await?
                .checkpoint_removed
        );
        assert_eq!(
            store.runtime_state_ids("scope-a").await?,
            vec!["runtime-b".to_string()]
        );
        let _ = std::fs::remove_dir_all(&tmp);
        Ok(())
    }

    #[tokio::test]
    async fn unwritable_scope_projection_cannot_block_enumeration_or_delete()
    -> crate::error::Result<()> {
        let tmp = tmp_base();
        let store = FileRuntimeStateStore::new(&tmp)?;
        store
            .save_checkpoint_for_scope("scope-a", &checkpoint("runtime-a", "a"))
            .await?;
        let projection = store.scope_index_path("scope-a")?;
        let _removed =
            remove_file_durable(&projection).map_err(FileRuntimeStateStore::to_react_err)?;
        std::fs::create_dir(&projection).map_err(FileRuntimeStateStore::to_react_err)?;

        assert_eq!(
            store.runtime_state_ids("scope-a").await?,
            vec!["runtime-a".to_string()]
        );
        assert!(
            store
                .clear_runtime_state("scope-a", "runtime-a")
                .await?
                .checkpoint_removed
        );
        assert!(store.runtime_state_ids("scope-a").await?.is_empty());
        assert!(store.get_checkpoint("runtime-a").await?.is_none());
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
        assert!(
            store
                .save_checkpoint_for_scope("../scope", &checkpoint("safe-runtime", "state"))
                .await
                .is_err()
        );
        assert!(
            store
                .clear_runtime_state("../scope", "safe-runtime")
                .await
                .is_err()
        );
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
        let clear_error = clear
            .await
            .map_err(FileRuntimeStateStore::to_react_err)?
            .err()
            .ok_or_else(|| ReactError::Other("corrupt clear was accepted".to_string()))?;
        assert!(clear_error.to_string().contains("parse"));
        assert!(store.get_checkpoint("corrupt").await.is_err());
        let _removed = remove_file_durable(&path).map_err(FileRuntimeStateStore::to_react_err)?;
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
