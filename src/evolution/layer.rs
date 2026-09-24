//! Memory layer manager — two-tier memory organization (hot/warm).
//!
//! (stage4) The cold layer was removed: staleness is now a recall-decay
//! weight, not a separate namespace. Archived memories (`MemoryStatus::Archived`)
//! stay in the warm/unified namespace and remain recallable (with decay).
//!
//! # Layers
//!
//! | Layer | Storage | Namespace | Purpose |
//! |-------|---------|-----------|---------|
//! | **Hot** | `.echo-agent/MEMORY.md` (YAML frontmatter + markdown body) | File | Approved entries loaded into context, max ~2000 tokens |
//! | **Warm** | Store KV | `["agent", "memories"]` | Available on-demand via search; Archived entries stay here (stage4: cold removed) |
//!
//! # Hot layer MEMORY.md format
//!
//! ```markdown
//! ---
//! entries:
//!   - key: build_java8
//!     memory_type: debugging_lesson
//!     confidence: 0.90
//!     stability: 0.80
//!     source: error_resolution
//!     topic: build
//!     risk: low
//!     last_promoted: "2026-06-15T10:30:00Z"
//! ---
//!
//! - **[build/java8]** Maven compile requires JAVA_HOME pointing to JDK 8.
//! - **[style/concise]** User prefers concise code comments.
//! ```
//!
//! # Promotion/Demotion
//!
//! Memories are promoted from warm→hot when they meet eligibility criteria
//! (`MemoryMeta::is_hot_eligible()`) and the hot layer has capacity.
//! When the hot layer exceeds its token budget, the lowest-priority entries
//! are demoted back to warm based on a demotion score.

use echo_core::memory::store::{Store, StoreItem};
use echo_core::memory::types::{
    MemoryApproval, MemoryMeta, MemoryProvenance, MemoryRisk, MemorySource, MemoryStatus,
    MemoryTrust, MemoryType, TypedMemoryValue,
};
use echo_state::memory::typed_store::{MemoryFilter, TypedMemoryEntry, TypedMemoryStore};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Mutex;

use super::audit::{ChangeEntryBuilder, ChangeFilter, ChangeLog, ChangeType, EntityType};
use super::mutation::{
    HotValue, MemoryOperation, MemoryOperationBatch, MemoryOperationHistory,
    MemoryOperationJournal, MemoryOperationOrigin,
};
use super::review::{
    AppliedMemoryMerge, ConflictDetector, ConflictGroup, MemoryConflictProposal,
    MemoryMergeSnapshot, MemoryMerger, MergeResult, ordered_conflict_entries,
};
use super::security::{
    EvolutionSecurityGuard, InputTrustLevel, PromptInjectionDetector, SecretScanner,
};
use echo_core::error::{ConfigError, ReactError};

/// Alias for layer operation results.
type Result<T> = std::result::Result<T, ReactError>;

fn stricter_memory_risk(left: MemoryRisk, right: MemoryRisk) -> MemoryRisk {
    match (left, right) {
        (MemoryRisk::High, _) | (_, MemoryRisk::High) => MemoryRisk::High,
        (MemoryRisk::Medium, _) | (_, MemoryRisk::Medium) => MemoryRisk::Medium,
        _ => MemoryRisk::Low,
    }
}

fn evidence_is_safe(provenance: &MemoryProvenance) -> bool {
    let scanner = SecretScanner::new();
    let injector = PromptInjectionDetector;
    provenance
        .evidence
        .iter()
        .all(|item| !scanner.scan(&item.quote).has_secrets && !injector.detect(&item.quote))
}

fn merge_plan_error(message: impl Into<String>) -> ReactError {
    ReactError::Config(Box::new(ConfigError::ConfigFileError(message.into())))
}

fn stale_merge_plan_error(message: impl Into<String>) -> ReactError {
    ReactError::Memory(Box::new(echo_core::error::MemoryError::StaleProposal(
        message.into(),
    )))
}

/// Return whether an error means a reviewed memory proposal is no longer current.
pub fn is_stale_memory_proposal_error(error: &ReactError) -> bool {
    matches!(
        error,
        ReactError::Memory(memory_error)
            if matches!(
                memory_error.as_ref(),
                echo_core::error::MemoryError::StaleProposal(_)
            )
    )
}

// ── Constants ──────────────────────────────────────────────────────────

/// Namespace for the unified typed-memory store (stage4: warm+cold+L3 all
/// merged here; Archived status replaces the former cold namespace).
pub const WARM_NAMESPACE: &[&str] = &["agent", "memories"];

/// Optional namespace for a separate cold/archival tier (retained as pub API
/// for consumers aligned with Letta/MemGPT archival memory — recall-on-demand,
/// not proactively loaded into context). The default product path uses
/// `WARM_NAMESPACE` + `MemoryStatus::Archived` instead; this constant exists
/// so consumers who need a distinct cold store can opt in.
pub const COLD_NAMESPACE: &[&str] = &["agent", "cold_memories"];

/// Maximum token budget for the hot layer (MEMORY.md body).
const HOT_TOKEN_BUDGET: usize = 2000;

// ── MemoryLayer ────────────────────────────────────────────────────────

/// A memory layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryLayer {
    /// Always loaded into context (MEMORY.md). Max ~2000 tokens.
    Hot,
    /// Available on-demand via Store KV search. Archived memories
    /// (`MemoryStatus::Archived`) also live here in the unified namespace —
    /// stage4 removed the separate Cold layer; staleness is a recall-decay
    /// weight, not a layer move.
    Warm,
    /// Optional third tier for long-term archival (rarely loaded, low
    /// confidence/stale). stage4 collapsed cold into `Warm`+`Archived`, but
    /// this variant + [`COLD_NAMESPACE`] are retained as pub API so consumers
    /// who need a separate cold tier (aligned with Letta/MemGPT archival) can
    /// opt in. The default product path does not use it.
    Cold,
}

impl std::fmt::Display for MemoryLayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Hot => write!(f, "hot"),
            Self::Warm => write!(f, "warm"),
            Self::Cold => write!(f, "cold"),
        }
    }
}

// ── HotEntryMeta ───────────────────────────────────────────────────────

/// Metadata for a single entry within the MEMORY.md hot layer.
///
/// Stored in YAML frontmatter alongside the markdown body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HotEntryMeta {
    /// Key identifier for this memory.
    pub key: String,
    /// Memory type classification.
    pub memory_type: MemoryType,
    /// Confidence score (0.0-1.0).
    pub confidence: f32,
    /// Stability score (0.0-1.0).
    pub stability: f32,
    /// Recall importance used by warm-layer ranking.
    #[serde(default = "default_hot_recall_weight")]
    pub recall_weight: f32,
    /// Source of this memory.
    pub source: MemorySource,
    /// Exact origin evidence and approval retained across hot/warm moves.
    #[serde(default)]
    pub provenance: MemoryProvenance,
    /// Topic category.
    pub topic: String,
    /// Risk level.
    #[serde(default)]
    pub risk: MemoryRisk,
    /// Number of semantic revisions accumulated before promotion.
    #[serde(default)]
    pub revision_count: u32,
    /// Number of successful recalls accumulated before promotion.
    #[serde(default)]
    pub recall_count: u32,
    /// Last successful recall timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_recalled_at: Option<u64>,
    /// Nontrivial content is stored as a JSON string in its body bullet so
    /// whitespace and newlines survive a MEMORY.md roundtrip. Legacy bullets
    /// without this flag remain plain text.
    #[serde(default, skip_serializing_if = "is_false")]
    pub content_json: bool,
    /// When this entry was promoted to hot (ISO 8601).
    pub last_promoted: String,
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl HotEntryMeta {
    /// Convert to a MemoryMeta for reconstruction.
    fn to_memory_meta(&self) -> MemoryMeta {
        let mut meta = MemoryMeta::new(self.memory_type, self.source, &self.topic)
            .with_confidence(self.confidence)
            .with_stability(self.stability)
            .with_recall_weight(self.recall_weight)
            .with_risk(self.risk)
            .with_provenance(self.provenance.clone());
        meta.revision_count = self.revision_count;
        meta.recall_count = self.recall_count;
        meta.last_recalled_at = self.last_recalled_at;
        meta
    }
}

fn default_hot_recall_weight() -> f32 {
    0.5
}

// ── MemoryFile ─────────────────────────────────────────────────────────

/// Parsed structure of MEMORY.md.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MemoryFile {
    /// Entries from YAML frontmatter.
    pub entries: Vec<HotEntryMeta>,
    /// The markdown body (human-readable, loaded into context).
    pub body: String,
}

// ── LayerChangeResult ──────────────────────────────────────────────────

/// Result of a promotion or demotion operation.
#[derive(Debug, Clone)]
pub struct LayerChangeResult {
    /// The key that was moved.
    pub key: String,
    /// Direction of the move (from).
    pub from_layer: MemoryLayer,
    /// Direction of the move (to).
    pub to_layer: MemoryLayer,
    /// Reason for the change.
    pub reason: String,
}

/// Exact Draft snapshot presented to an external reviewer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryActivationProposal {
    /// Memory key in the canonical layered-memory authority.
    pub key: String,
    /// Exact content reviewed by the caller.
    pub content: String,
    /// Exact Draft metadata reviewed by the caller.
    pub meta: MemoryMeta,
    /// Latest durable journal generation for this key when it was reviewed.
    pub generation: u64,
}

/// Durable activation receipt returned after Draft-to-Active settlement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryActivationReceipt {
    /// Activated memory key.
    pub key: String,
    /// Stable approval identity persisted in the memory provenance.
    pub approval_id: String,
    /// Journal generation of the Draft that was approved.
    pub draft_generation: u64,
}

/// Result of applying a caller-owned approval to one exact Draft snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemoryActivationOutcome {
    /// The Draft-to-Active transition was committed.
    Activated(MemoryActivationReceipt),
    /// The same approval had already reached Active state.
    AlreadyActivated(MemoryActivationReceipt),
}

/// A typed, stable target for a later memory rollback.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemoryRollbackTarget {
    /// Roll back the durable batch containing this ChangeLog entry.
    ChangeId(String),
    /// Roll back a complete durable journal batch.
    BatchId(String),
}

impl MemoryRollbackTarget {
    pub fn change_id(value: impl Into<String>) -> Self {
        Self::ChangeId(value.into())
    }

    pub fn batch_id(value: impl Into<String>) -> Self {
        Self::BatchId(value.into())
    }
}

/// A stable explanation for a rollback target that cannot safely be applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryRollbackConflict {
    pub target_batch_id: Option<String>,
    pub affected_keys: Vec<String>,
    pub reason: String,
}

/// A stable explanation for unavailable or expired journal history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryRollbackHistoryUnavailable {
    pub target: MemoryRollbackTarget,
    pub reason: String,
}

/// Preview of a later rollback before it creates an inverse batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryRollbackPreview {
    pub target_batch_id: String,
    pub target_generation: u64,
    pub affected_keys: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemoryRollbackPreviewOutcome {
    Ready(MemoryRollbackPreview),
    Conflict(MemoryRollbackConflict),
    HistoryUnavailable(MemoryRollbackHistoryUnavailable),
}

/// Durable receipt for a committed inverse batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryRollbackReceipt {
    pub request_id: String,
    pub target_batch_id: String,
    pub inverse_batch_id: String,
    pub target_generation: u64,
    pub affected_keys: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemoryRollbackOutcome {
    Applied(MemoryRollbackReceipt),
    AlreadyApplied(MemoryRollbackReceipt),
    Conflict(MemoryRollbackConflict),
    HistoryUnavailable(MemoryRollbackHistoryUnavailable),
}

// ── MemoryLayerManager ─────────────────────────────────────────────────

/// Manages the two-tier memory layer system (stage4: cold removed).
///
/// - **Hot layer**: `.echo-agent/MEMORY.md` (YAML frontmatter + markdown body).
///   Only approved entries are loaded into context. Max ~2000 tokens.
/// - **Warm layer**: Store KV under [`WARM_NAMESPACE`] = `["agent", "memories"]`.
///   Available on-demand via search; Archived entries stay here (recallable
///   with decay). See module-level docs for why cold is gone.
pub struct MemoryLayerManager {
    /// Path to the MEMORY.md file.
    hot_path: PathBuf,
    /// In-process lock serializing all MEMORY.md read-modify-write so two
    /// calls on the same `MemoryLayerManager` cannot interleave and lose
    /// updates (P0-3). Cross-process safety is provided by the fs2 advisory
    /// lock in `with_locked_hot_file`.
    hot_lock: Mutex<()>,
    /// Typed store for warm/cold layers.
    typed_store: TypedMemoryStore,
    /// Change log for audit trail.
    change_log: Box<dyn ChangeLog>,
    /// A failed journal open remains a hard mutation error, even though this
    /// legacy constructor cannot return Result.
    operation_journal: std::result::Result<MemoryOperationJournal, String>,
    operation_lock: Arc<Mutex<()>>,
    reconciled: AtomicBool,
    /// Security guard for write-time checks (secret scan, injection, rate limit).
    security_guard: EvolutionSecurityGuard,
    /// Optional observer called after persisted evolution changes.
    evolution_observer: Option<Arc<dyn EvolutionObserver>>,
}

/// Observer for durable memory and skill-evolution events.
pub trait EvolutionObserver: Send + Sync {
    /// Called after a memory has reached the layered store.
    fn on_memory_write<'a>(&'a self, _key: &'a str, _source: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }

    /// Called after a memory layer transition is fully persisted.
    fn on_memory_layer_change<'a>(
        &'a self,
        _key: &'a str,
        _from_layer: &'a str,
        _to_layer: &'a str,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }

    /// Called after a new skill candidate is persisted.
    fn on_skill_candidate_detected<'a>(&'a self, _skill_name: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }

    /// Called after a skill health report is computed.
    fn on_skill_health_check<'a>(&'a self, _skill_name: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }
}

/// Guard holding an exclusive lock on the MEMORY.md hot layer plus the
/// freshly-loaded `MemoryFile`.
///
/// Created by [`MemoryLayerManager::lock_hot_file`]. Hold it across async
/// work, then call [`commit`](Self::commit) to persist or drop to discard. The
/// in-process Mutex and the cross-process flock are released on drop (the
/// flock also auto-releases if the process dies, so a kill -9 cannot wedge a
/// peer).
struct HotFileGuard<'a> {
    #[cfg(test)]
    manager: &'a MemoryLayerManager,
    file: MemoryFile,
    /// In-process Mutex guard (`tokio::sync::MutexGuard`, which is `Send`, so
    /// the guard can be held across `.await`s). Keeps other tasks on the same
    /// manager out of the critical section until this guard drops.
    _inproc: tokio::sync::MutexGuard<'a, ()>,
    /// Holds the cross-process advisory lock; dropped to release. The flock is
    /// also released by the OS on fd close (kill-safe).
    _lock_file: std::fs::File,
}

impl<'a> HotFileGuard<'a> {
    /// Persist the (possibly mutated) `MemoryFile` back to disk, still under
    /// the held locks.
    #[cfg(test)]
    fn commit(self) -> std::io::Result<()> {
        self.manager.write_memory_file(&self.file)
        // self drops here: inproc guard + lock file fd dropped → both released.
    }
}

impl MemoryLayerManager {
    /// Create a new layer manager.
    ///
    /// # Arguments
    /// * `echo_agent_dir` — Path to the `.echo-agent/` directory (hot layer MEMORY.md will be inside).
    /// * `store` — The underlying Store implementation for warm/cold layers.
    /// * `change_log` — Audit log for recording all mutations.
    pub fn new(
        echo_agent_dir: PathBuf,
        store: Arc<dyn Store>,
        change_log: Box<dyn ChangeLog>,
    ) -> Self {
        let hot_path = echo_agent_dir.join("MEMORY.md");
        let operation_journal =
            MemoryOperationJournal::open(&echo_agent_dir).map_err(|error| error.to_string());
        let operation_lock = operation_journal
            .as_ref()
            .map(MemoryOperationJournal::serial)
            .unwrap_or_else(|_| Arc::new(Mutex::new(())));
        Self {
            hot_path,
            hot_lock: Mutex::new(()),
            typed_store: TypedMemoryStore::new(store),
            change_log,
            operation_journal,
            operation_lock,
            reconciled: AtomicBool::new(false),
            security_guard: EvolutionSecurityGuard::default_config(),
            evolution_observer: None,
        }
    }

    /// Construct a manager that rejects a missing or corrupt recovery journal.
    /// Call [`Self::reconcile_pending`] before handing its Store to readers.
    pub fn try_new(
        echo_agent_dir: PathBuf,
        store: Arc<dyn Store>,
        change_log: Box<dyn ChangeLog>,
    ) -> Result<Self> {
        let manager = Self::new(echo_agent_dir, store, change_log);
        manager.operation_journal()?;
        Ok(manager)
    }

    /// Configure an observer invoked after successful durable evolution events.
    pub fn with_evolution_observer(mut self, observer: Arc<dyn EvolutionObserver>) -> Self {
        self.evolution_observer = Some(observer);
        self
    }

    fn operation_journal(&self) -> Result<&MemoryOperationJournal> {
        self.operation_journal.as_ref().map_err(|error| {
            ReactError::Other(format!("memory operation journal unavailable: {error}"))
        })
    }

    fn typed_value(content: &str, meta: MemoryMeta) -> Result<serde_json::Value> {
        TypedMemoryValue::new(content, meta)
            .to_value()
            .map_err(|error| {
                ReactError::Other(format!("failed to serialize memory projection: {error}"))
            })
    }

    fn hot_value(file: &MemoryFile, key: &str) -> Option<HotValue> {
        file.entries
            .iter()
            .find(|entry| entry.key == key)
            .map(|meta| HotValue {
                meta: meta.clone(),
                content: extract_hot_value_content(&file.body, meta),
            })
    }

    fn set_hot_value(file: &mut MemoryFile, key: &str, value: Option<&HotValue>) -> Result<()> {
        file.entries.retain(|entry| entry.key != key);
        let pattern = format!("- **[{key}]**");
        file.body = file
            .body
            .lines()
            .filter(|line| !line.starts_with(&pattern))
            .collect::<Vec<_>>()
            .join("\n");
        if !file.body.is_empty() && !file.body.ends_with('\n') {
            file.body.push('\n');
        }
        if let Some(value) = value {
            let content = if value.meta.content_json {
                serde_json::to_string(&value.content).map_err(|error| {
                    ReactError::Other(format!("failed to encode hot memory content: {error}"))
                })?
            } else {
                value.content.clone()
            };
            file.entries.push(value.meta.clone());
            file.body.push_str(&format!("- **[{key}]** {content}\n"));
        }
        Ok(())
    }

    fn semantic_warm(value: &serde_json::Value) -> serde_json::Value {
        let mut normalized = value.clone();
        if let Some(meta) = normalized
            .get_mut("meta")
            .and_then(serde_json::Value::as_object_mut)
        {
            meta.remove("recall_count");
            meta.remove("last_recalled_at");
        }
        normalized
    }

    fn warm_matches(
        current: Option<&serde_json::Value>,
        expected: Option<&serde_json::Value>,
    ) -> bool {
        match (current, expected) {
            (Some(current), Some(expected)) => {
                Self::semantic_warm(current) == Self::semantic_warm(expected)
            }
            (None, None) => true,
            _ => false,
        }
    }

    fn preserve_recall_telemetry(
        current: Option<&serde_json::Value>,
        desired: &serde_json::Value,
    ) -> serde_json::Value {
        let mut result = desired.clone();
        // Recall counts belong to a reviewed fact, not just its storage key.
        // Replacing the fact or its origin starts a fresh telemetry lifetime.
        let same_fact = current
            .and_then(|value| TypedMemoryValue::from_value(value).ok())
            .zip(TypedMemoryValue::from_value(desired).ok())
            .is_some_and(|(before, after)| {
                before.content == after.content
                    && before.meta.provenance.trust == after.meta.provenance.trust
                    && before.meta.provenance.evidence == after.meta.provenance.evidence
            });
        if !same_fact {
            return result;
        }
        if let (Some(current_meta), Some(target_meta)) = (
            current
                .and_then(|value| value.get("meta"))
                .and_then(serde_json::Value::as_object),
            result
                .get_mut("meta")
                .and_then(serde_json::Value::as_object_mut),
        ) {
            for field in ["recall_count", "last_recalled_at"] {
                let old = current_meta.get(field).and_then(serde_json::Value::as_u64);
                let new = target_meta.get(field).and_then(serde_json::Value::as_u64);
                if let Some(value) = old.into_iter().chain(new).max() {
                    target_meta.insert(field.to_owned(), serde_json::Value::from(value));
                }
            }
        }
        result
    }

    async fn apply_operation(&self, operation: &MemoryOperation) -> Result<()> {
        self.apply_operation_with_history(operation, std::slice::from_ref(operation))
            .await
    }

    async fn apply_operation_with_history(
        &self,
        operation: &MemoryOperation,
        history: &[MemoryOperation],
    ) -> Result<()> {
        let mut hot = self.lock_hot_file().await.map_err(ReactError::from)?;
        let current_hot = Self::hot_value(&hot.file, &operation.key);
        let current_warm = self
            .typed_store
            .inner()
            .get(WARM_NAMESPACE, &operation.key)
            .await?
            .map(|entry| entry.value);
        if !history
            .iter()
            .any(|prior| current_hot == prior.hot_before || current_hot == prior.hot_after)
        {
            return Err(ReactError::Other(format!(
                "memory operation {} conflicts with hot entry {}",
                operation.id, operation.key
            )));
        }
        if !history.iter().any(|prior| {
            Self::warm_matches(current_warm.as_ref(), prior.warm_before.as_ref())
                || Self::warm_matches(current_warm.as_ref(), prior.warm_after.as_ref())
        }) {
            return Err(ReactError::Other(format!(
                "memory operation {} conflicts with warm entry {}",
                operation.id, operation.key
            )));
        }

        // Destination-first on layer moves. Every intermediate state is
        // repairable from the durable intent, including a cancelled future.
        if operation.hot_after.is_some() && current_hot != operation.hot_after {
            Self::set_hot_value(&mut hot.file, &operation.key, operation.hot_after.as_ref())?;
            self.write_memory_file(&hot.file)
                .map_err(ReactError::from)?;
        }
        if let Some(after) = &operation.warm_after
            && !Self::warm_matches(current_warm.as_ref(), Some(after))
        {
            self.typed_store
                .inner()
                .put(
                    WARM_NAMESPACE,
                    &operation.key,
                    Self::preserve_recall_telemetry(current_warm.as_ref(), after),
                )
                .await?;
        }
        if operation.hot_after.is_none() && current_hot.is_some() {
            Self::set_hot_value(&mut hot.file, &operation.key, None)?;
            self.write_memory_file(&hot.file)
                .map_err(ReactError::from)?;
        }
        if operation.warm_after.is_none() && current_warm.is_some() {
            self.typed_store
                .inner()
                .delete(WARM_NAMESPACE, &operation.key)
                .await?;
        }
        drop(hot);
        Ok(())
    }

    async fn settle_batch(&self, batch: &MemoryOperationBatch) -> Result<()> {
        for operation in &batch.operations {
            self.apply_operation(operation).await?;
            self.change_log.record_idempotent(operation.audit.clone())?;
        }
        self.operation_journal()?.settle(&batch.id)
    }

    async fn reconcile_pending_locked(&self) -> Result<()> {
        let history = self.operation_journal()?.history()?;
        let mut by_key = BTreeMap::<String, Vec<MemoryOperation>>::new();
        for (batch, _) in &history {
            for operation in &batch.operations {
                by_key
                    .entry(operation.key.clone())
                    .or_default()
                    .push(operation.clone());
            }
        }
        for operations in by_key.values() {
            let latest = operations.last().ok_or_else(|| {
                ReactError::Other("memory operation history has an empty key series".into())
            })?;
            self.apply_operation_with_history(latest, operations)
                .await?;
        }
        for (batch, settled) in history {
            if !settled {
                for operation in &batch.operations {
                    self.change_log.record_idempotent(operation.audit.clone())?;
                }
                self.operation_journal()?.settle(&batch.id)?;
            }
        }
        self.reconciled.store(true, Ordering::Release);
        Ok(())
    }

    /// Finish prepared memory operations after restart or an uncertain failure.
    /// Repeated calls replay the same identity and never append duplicate audit.
    pub async fn reconcile_pending(&self) -> Result<()> {
        let _serial = self.operation_lock.lock().await;
        self.reconcile_pending_locked().await
    }

    fn rollback_target_batch_id(
        &self,
        target: &MemoryRollbackTarget,
        history: &[MemoryOperationHistory],
    ) -> Result<Option<String>> {
        match target {
            MemoryRollbackTarget::BatchId(batch_id) => Ok(history
                .iter()
                .find(|item| item.batch.id == *batch_id)
                .map(|item| item.batch.id.clone())),
            MemoryRollbackTarget::ChangeId(change_id) => {
                let known = self
                    .change_log
                    .query(&ChangeFilter::new())?
                    .into_iter()
                    .any(|entry| entry.change_id == *change_id);
                if !known {
                    return Ok(None);
                }
                Ok(history
                    .iter()
                    .find(|item| {
                        item.batch
                            .operations
                            .iter()
                            .any(|operation| operation.audit.change_id == *change_id)
                    })
                    .map(|item| item.batch.id.clone()))
            }
        }
    }

    fn rollback_resolution<'a>(
        &self,
        target: &MemoryRollbackTarget,
        history: &'a [MemoryOperationHistory],
    ) -> Result<std::result::Result<&'a MemoryOperationHistory, MemoryRollbackPreviewOutcome>> {
        let Some(batch_id) = self.rollback_target_batch_id(target, history)? else {
            return Ok(Err(MemoryRollbackPreviewOutcome::HistoryUnavailable(
                MemoryRollbackHistoryUnavailable {
                    target: target.clone(),
                    reason: "target is absent from the retained ChangeLog or operation journal"
                        .to_string(),
                },
            )));
        };
        let Some(item) = history.iter().find(|item| item.batch.id == batch_id) else {
            return Ok(Err(MemoryRollbackPreviewOutcome::HistoryUnavailable(
                MemoryRollbackHistoryUnavailable {
                    target: target.clone(),
                    reason: "target journal batch is unavailable".to_string(),
                },
            )));
        };
        let keys = item
            .batch
            .operations
            .iter()
            .map(|operation| operation.key.clone())
            .collect::<Vec<_>>();
        if !item.settled {
            return Ok(Err(MemoryRollbackPreviewOutcome::HistoryUnavailable(
                MemoryRollbackHistoryUnavailable {
                    target: target.clone(),
                    reason: "target batch is not settled".to_string(),
                },
            )));
        }
        for key in &keys {
            let latest = history
                .iter()
                .filter(|candidate| {
                    candidate
                        .batch
                        .operations
                        .iter()
                        .any(|operation| operation.key == *key)
                })
                .max_by_key(|candidate| candidate.generation);
            if latest.map(|candidate| candidate.batch.id.as_str()) != Some(batch_id.as_str()) {
                return Ok(Err(MemoryRollbackPreviewOutcome::Conflict(
                    MemoryRollbackConflict {
                        target_batch_id: Some(batch_id),
                        affected_keys: keys.clone(),
                        reason: format!(
                            "target is not the latest journal generation for memory key {key}"
                        ),
                    },
                )));
            }
        }
        Ok(Ok(item))
    }

    /// Preview a later rollback without creating an inverse batch.
    pub async fn preview_rollback(
        &self,
        target: &MemoryRollbackTarget,
    ) -> Result<MemoryRollbackPreviewOutcome> {
        let _serial = self.operation_lock.lock().await;
        self.reconcile_pending_locked().await?;
        let history = self.operation_journal()?.history_with_generations()?;
        match self.rollback_resolution(target, &history)? {
            Ok(item) => Ok(MemoryRollbackPreviewOutcome::Ready(MemoryRollbackPreview {
                target_batch_id: item.batch.id.clone(),
                target_generation: item.generation,
                affected_keys: item
                    .batch
                    .operations
                    .iter()
                    .map(|operation| operation.key.clone())
                    .collect(),
            })),
            Err(outcome) => Ok(outcome),
        }
    }

    /// Alias for callers that use the shorter rollback vocabulary.
    pub async fn preview(
        &self,
        target: &MemoryRollbackTarget,
    ) -> Result<MemoryRollbackPreviewOutcome> {
        self.preview_rollback(target).await
    }

    fn rollback_receipt(
        request_id: &str,
        target_batch_id: &str,
        target_generation: u64,
        inverse: &MemoryOperationHistory,
    ) -> MemoryRollbackReceipt {
        MemoryRollbackReceipt {
            request_id: request_id.to_owned(),
            target_batch_id: target_batch_id.to_owned(),
            inverse_batch_id: inverse.batch.id.clone(),
            target_generation,
            affected_keys: inverse
                .batch
                .operations
                .iter()
                .map(|operation| operation.key.clone())
                .collect(),
        }
    }

    fn inverse_change_type(change_type: ChangeType) -> ChangeType {
        match change_type {
            ChangeType::Create => ChangeType::Delete,
            ChangeType::Delete => ChangeType::Create,
            ChangeType::Promote => ChangeType::Demote,
            ChangeType::Demote => ChangeType::Promote,
            ChangeType::Update | ChangeType::Merge => ChangeType::Update,
        }
    }

    fn rollback_projection_summary(
        warm: &Option<serde_json::Value>,
        hot: &Option<HotValue>,
        original_change_id: &str,
        target_batch_id: &str,
        target_generation: u64,
    ) -> serde_json::Value {
        let layer = if hot.is_some() {
            "hot"
        } else if warm.is_some() {
            "warm"
        } else {
            "absent"
        };
        serde_json::json!({
            "layer": layer,
            "warm": warm,
            "hot": hot,
            "rollback_of": original_change_id,
            "target_batch_id": target_batch_id,
            "target_generation": target_generation,
        })
    }

    /// Create and settle one durable inverse batch for a settled memory batch.
    ///
    /// The request ID is persisted in the inverse batch lineage. Replaying the
    /// same request returns `AlreadyApplied`; reusing it for another target
    /// fails closed. Each inverse is a new batch, so rollback-of-rollback is
    /// ordinary later rollback with a fresh request ID.
    pub async fn rollback_memory(
        &self,
        request_id: &str,
        target: MemoryRollbackTarget,
    ) -> Result<MemoryRollbackOutcome> {
        if request_id.is_empty() {
            return Err(ReactError::Other(
                "rollback request ID must not be empty".into(),
            ));
        }
        let _serial = self.operation_lock.lock().await;
        self.reconcile_pending_locked().await?;
        let history = self.operation_journal()?.history_with_generations()?;

        if let Some(existing) = history.iter().find(|item| {
            item.batch
                .origin
                .as_ref()
                .is_some_and(|origin| origin.request_id == request_id)
        }) {
            let requested_batch = self.rollback_target_batch_id(&target, &history)?;
            let origin = existing.batch.origin.as_ref().ok_or_else(|| {
                ReactError::Other("rollback lineage disappeared from journal history".into())
            })?;
            if requested_batch.as_deref() != Some(origin.target_batch_id.as_str()) {
                return Ok(MemoryRollbackOutcome::Conflict(MemoryRollbackConflict {
                    target_batch_id: requested_batch,
                    affected_keys: existing
                        .batch
                        .operations
                        .iter()
                        .map(|operation| operation.key.clone())
                        .collect(),
                    reason: "request ID was already used for a different rollback target"
                        .to_string(),
                }));
            }
            let receipt = Self::rollback_receipt(
                request_id,
                &origin.target_batch_id,
                origin.target_generation,
                existing,
            );
            return if existing.settled {
                Ok(MemoryRollbackOutcome::AlreadyApplied(receipt))
            } else {
                Err(ReactError::Other(
                    "rollback request remained unsettled after reconciliation".into(),
                ))
            };
        }

        let item = match self.rollback_resolution(&target, &history)? {
            Ok(item) => item,
            Err(MemoryRollbackPreviewOutcome::Conflict(conflict)) => {
                return Ok(MemoryRollbackOutcome::Conflict(conflict));
            }
            Err(MemoryRollbackPreviewOutcome::HistoryUnavailable(unavailable)) => {
                return Ok(MemoryRollbackOutcome::HistoryUnavailable(unavailable));
            }
            Err(MemoryRollbackPreviewOutcome::Ready(_)) => {
                return Err(ReactError::Other(
                    "rollback resolution returned an invalid ready state".into(),
                ));
            }
        };
        let inverse_batch_id = uuid::Uuid::new_v4().to_string();
        let mut operations = Vec::with_capacity(item.batch.operations.len());
        for operation in &item.batch.operations {
            let operation_id = uuid::Uuid::new_v4().to_string();
            let audit = ChangeEntryBuilder::new(
                EntityType::Memory,
                &operation.key,
                Self::inverse_change_type(operation.audit.change_type),
            )
            .before(Self::rollback_projection_summary(
                &operation.warm_after,
                &operation.hot_after,
                &operation.audit.change_id,
                &item.batch.id,
                item.generation,
            ))
            .after(Self::rollback_projection_summary(
                &operation.warm_before,
                &operation.hot_before,
                &operation.audit.change_id,
                &item.batch.id,
                item.generation,
            ))
            .reason(format!(
                "rollback of {:?} memory change in batch {}",
                operation.audit.change_type, item.batch.id
            ))
            .trigger("memory_rollback")
            .build_with(operation_id.clone(), chrono::Utc::now());
            operations.push(MemoryOperation {
                id: operation_id,
                key: operation.key.clone(),
                warm_before: operation.warm_after.clone(),
                warm_after: operation.warm_before.clone(),
                hot_before: operation.hot_after.clone(),
                hot_after: operation.hot_before.clone(),
                audit,
            });
        }
        let inverse = MemoryOperationBatch {
            id: inverse_batch_id,
            operations,
            origin: Some(MemoryOperationOrigin {
                request_id: request_id.to_owned(),
                target_batch_id: item.batch.id.clone(),
                target_generation: item.generation,
            }),
        };
        self.operation_journal()?.prepare_batch(inverse.clone())?;
        self.settle_batch(&inverse).await?;
        let inverse_history = self
            .operation_journal()?
            .history_with_generations()?
            .into_iter()
            .find(|candidate| candidate.batch.id == inverse.id)
            .ok_or_else(|| ReactError::Other("rollback receipt batch disappeared".into()))?;
        Ok(MemoryRollbackOutcome::Applied(Self::rollback_receipt(
            request_id,
            &item.batch.id,
            item.generation,
            &inverse_history,
        )))
    }

    /// Alias for callers that use the shorter rollback vocabulary.
    pub async fn rollback(
        &self,
        request_id: &str,
        target: MemoryRollbackTarget,
    ) -> Result<MemoryRollbackOutcome> {
        self.rollback_memory(request_id, target).await
    }

    async fn transition(
        &self,
        key: &str,
        warm_after: Option<serde_json::Value>,
        hot_after: Option<HotValue>,
        builder: ChangeEntryBuilder,
        expected: Option<(MemoryLayer, &TypedMemoryEntry)>,
    ) -> Result<()> {
        self.transition_at_generation(key, warm_after, hot_after, builder, expected, None)
            .await
    }

    async fn transition_at_generation(
        &self,
        key: &str,
        warm_after: Option<serde_json::Value>,
        hot_after: Option<HotValue>,
        builder: ChangeEntryBuilder,
        expected: Option<(MemoryLayer, &TypedMemoryEntry)>,
        expected_generation: Option<u64>,
    ) -> Result<()> {
        let _serial = self.operation_lock.lock().await;
        self.reconcile_pending_locked().await?;
        if let Some(generation) = expected_generation
            && self.latest_key_generation(key)? != Some(generation)
        {
            return Err(stale_merge_plan_error(
                "memory journal generation changed; refresh the activation proposal",
            ));
        }
        let hot = self.lock_hot_file().await.map_err(ReactError::from)?;
        let hot_before = Self::hot_value(&hot.file, key);
        let warm_before = self
            .typed_store
            .inner()
            .get(WARM_NAMESPACE, key)
            .await?
            .map(|entry| entry.value);
        drop(hot);

        if let Some((layer, entry)) = expected {
            let matches = if entry.key != key {
                false
            } else {
                match layer {
                    MemoryLayer::Warm => {
                        hot_before.is_none()
                            && Self::warm_matches(warm_before.as_ref(), Some(&entry.raw.value))
                    }
                    MemoryLayer::Hot => match hot_before.as_ref() {
                        Some(current) if current.content == entry.content => {
                            let current_value =
                                Self::typed_value(&current.content, current.meta.to_memory_meta())?;
                            let expected_value =
                                Self::typed_value(&entry.content, entry.meta.clone())?;
                            Self::warm_matches(Some(&current_value), Some(&expected_value))
                        }
                        _ => false,
                    },
                    MemoryLayer::Cold => false,
                }
            };
            if !matches {
                return Err(ReactError::Other(format!(
                    "memory key {key} changed before transition; retry from a fresh read"
                )));
            }
        }

        let id = uuid::Uuid::new_v4().to_string();
        let mut audit = builder.build_with(id.clone(), chrono::Utc::now());
        if audit.trigger == "write_memory" {
            if let Some(candidate) = warm_after
                .as_ref()
                .and_then(|value| TypedMemoryValue::from_value(value).ok())
            {
                let same_approved_warm = warm_before
                    .as_ref()
                    .and_then(|value| TypedMemoryValue::from_value(value).ok())
                    .is_some_and(|current| {
                        current.content == candidate.content && current.meta.is_recallable()
                    });
                let same_approved_hot = hot_before.as_ref().is_some_and(|current| {
                    current.content == candidate.content
                        && current.meta.to_memory_meta().is_recallable()
                });
                if same_approved_warm || same_approved_hot {
                    return Ok(());
                }
            }
            audit.change_type = if warm_before.is_some() || hot_before.is_some() {
                ChangeType::Update
            } else {
                ChangeType::Create
            };
            audit.before = (warm_before.is_some() || hot_before.is_some())
                .then(|| serde_json::json!({ "layer": if hot_before.is_some() { "hot" } else { "warm" } }));
        }
        let operation = MemoryOperation {
            audit,
            id,
            key: key.to_owned(),
            warm_before,
            warm_after,
            hot_before,
            hot_after,
        };
        self.operation_journal()?.prepare(operation.clone())?;
        self.settle_batch(&MemoryOperationBatch {
            id: operation.id.clone(),
            operations: vec![operation],
            origin: None,
        })
        .await
    }

    fn latest_key_generation(&self, key: &str) -> Result<Option<u64>> {
        Ok(self
            .operation_journal()?
            .history_with_generations()?
            .into_iter()
            .filter(|item| item.batch.operations.iter().any(|op| op.key == key))
            .map(|item| item.generation)
            .max())
    }

    // ── Reading ─────────────────────────────────────────────────────

    /// Read the hot layer content (MEMORY.md body, frontmatter stripped).
    ///
    /// Returns an error while an operation awaits reconciliation.
    pub fn read_hot_content(&self) -> Result<String> {
        let _serial = self.clean_read_guard()?;
        let file = self.parse_memory_file();
        let mut body = String::new();
        for meta in &file.entries {
            if !meta.to_memory_meta().is_recallable() {
                continue;
            }
            let content = extract_hot_value_content(&file.body, meta);
            let rendered = if meta.content_json {
                serde_json::to_string(&content).map_err(|error| {
                    ReactError::Other(format!("failed to encode hot memory content: {error}"))
                })?
            } else {
                content
            };
            body.push_str(&format!("- **[{}]** {}\n", meta.key, rendered));
        }
        Ok(body)
    }

    /// Read the hot layer metadata (MEMORY.md YAML frontmatter entries).
    ///
    /// Returns an error while an operation awaits reconciliation.
    pub fn read_hot_meta(&self) -> Result<Vec<HotEntryMeta>> {
        let _serial = self.clean_read_guard()?;
        Ok(self.parse_memory_file().entries)
    }

    fn clean_read_guard(&self) -> Result<tokio::sync::MutexGuard<'_, ()>> {
        let guard = self.operation_lock.try_lock().map_err(|_| {
            ReactError::Other("memory mutation in progress; read after settlement".into())
        })?;
        let history = self.operation_journal()?.history()?;
        if history.iter().any(|(_, settled)| !settled)
            || (!history.is_empty() && !self.reconciled.load(Ordering::Acquire))
        {
            return Err(ReactError::Other(
                "memory mutation pending; call reconcile_pending before reading".into(),
            ));
        }
        Ok(guard)
    }

    /// Determine which layer a memory key currently resides in.
    ///
    /// Checks hot (MEMORY.md), then warm (Store). Stage4 removed the cold
    /// layer — Archived memories live on in the warm/unified namespace.
    /// Returns `None` if the key is not found in any layer.
    pub async fn locate(&self, key: &str) -> Result<Option<(MemoryLayer, TypedMemoryEntry)>> {
        let _serial = self.operation_lock.lock().await;
        self.reconcile_pending_locked().await?;
        // Check hot layer
        let file = self.parse_memory_file();
        if let Some(entry) = self.find_in_hot(&file, key) {
            return Ok(Some((MemoryLayer::Hot, entry)));
        }

        // Check warm layer (unified namespace; includes Archived entries)
        if let Some(entry) = self.typed_store.get_typed(WARM_NAMESPACE, key).await? {
            return Ok(Some((MemoryLayer::Warm, entry)));
        }

        Ok(None)
    }

    /// Get all hot entries as TypedMemoryEntries (reconstructed from MEMORY.md).
    pub fn list_hot(&self) -> Result<Vec<TypedMemoryEntry>> {
        let _serial = self.clean_read_guard()?;
        let file = self.parse_memory_file();
        Ok(file
            .entries
            .iter()
            .filter_map(|meta| self.hot_meta_to_entry(meta, &file.body))
            .collect())
    }

    /// Get all warm entries matching a filter.
    pub async fn list_warm(&self, filter: &MemoryFilter) -> Result<Vec<TypedMemoryEntry>> {
        let _serial = self.operation_lock.lock().await;
        self.reconcile_pending_locked().await?;
        self.typed_store.list_typed(WARM_NAMESPACE, filter).await
    }

    /// Recall warm memory only after the canonical mutation journal settles.
    pub async fn recall_warm(&self, query: &str, limit: usize) -> Result<Vec<StoreItem>> {
        let _serial = self.operation_lock.lock().await;
        self.reconcile_pending_locked().await?;
        crate::evolution::recall::MemoryRecaller::new(self.typed_store.inner().clone())
            .recall(query, limit)
            .await
    }

    pub(crate) fn store(&self) -> &Arc<dyn Store> {
        self.typed_store.inner()
    }

    /// Read one Draft as an exact activation proposal.
    pub async fn preview_activation(&self, key: &str) -> Result<Option<MemoryActivationProposal>> {
        let _serial = self.operation_lock.lock().await;
        self.reconcile_pending_locked().await?;
        if Self::hot_value(&self.parse_memory_file(), key).is_some() {
            return Ok(None);
        }
        let Some(entry) = self.typed_store.get_typed(WARM_NAMESPACE, key).await? else {
            return Ok(None);
        };
        if entry.meta.status != MemoryStatus::Draft
            || !entry.meta.provenance.is_well_formed()
            || !entry.meta.has_required_evidence()
            || !evidence_is_safe(&entry.meta.provenance)
            || entry.meta.provenance.approval.is_some()
        {
            return Ok(None);
        }
        let Some(generation) = self.latest_key_generation(key)? else {
            return Ok(None);
        };
        Ok(Some(MemoryActivationProposal {
            key: entry.key,
            content: entry.content,
            meta: entry.meta,
            generation,
        }))
    }

    fn activation_still_current(
        &self,
        proposal: &MemoryActivationProposal,
        approval: &MemoryApproval,
        current: &TypedMemoryEntry,
        layer: MemoryLayer,
    ) -> Result<bool> {
        if current.meta.status != MemoryStatus::Active || current.content != proposal.content {
            return Ok(false);
        }
        let mut expected_meta = proposal.meta.clone();
        expected_meta.status = MemoryStatus::Active;
        expected_meta.provenance.approval = Some(approval.clone());
        expected_meta.recall_count = current.meta.recall_count;
        expected_meta.last_recalled_at = current.meta.last_recalled_at;
        if current.meta != expected_meta {
            return Ok(false);
        }

        let history = self.operation_journal()?.history_with_generations()?;
        let key_history = history
            .iter()
            .filter_map(|item| {
                item.batch
                    .operations
                    .iter()
                    .find(|operation| operation.key == proposal.key)
                    .map(|operation| (item.generation, operation))
            })
            .collect::<Vec<_>>();
        let Some(draft_position) = key_history
            .iter()
            .position(|(generation, _)| *generation == proposal.generation)
        else {
            return Ok(false);
        };
        let Some((_, activation)) = key_history.get(draft_position.saturating_add(1)) else {
            return Ok(false);
        };
        let reviewed = Self::typed_value(&proposal.content, proposal.meta.clone())?;
        let activated = Self::typed_value(&proposal.content, expected_meta)?;
        if activation.audit.trigger != "activate_draft"
            || !Self::warm_matches(activation.warm_before.as_ref(), Some(&reviewed))
            || !Self::warm_matches(activation.warm_after.as_ref(), Some(&activated))
        {
            return Ok(false);
        }
        let remaining = key_history
            .get(draft_position.saturating_add(2)..)
            .unwrap_or(&[]);
        match layer {
            MemoryLayer::Warm => Ok(remaining.is_empty()),
            MemoryLayer::Hot => Ok(remaining.len() == 1
                && remaining.first().is_some_and(|(_, operation)| {
                    operation.audit.trigger == "promote"
                        && Self::warm_matches(
                            operation.warm_before.as_ref(),
                            activation.warm_after.as_ref(),
                        )
                        && operation.hot_after.as_ref().is_some_and(|hot| {
                            hot.content == current.content
                                && hot.meta.to_memory_meta().provenance == current.meta.provenance
                        })
                })),
            MemoryLayer::Cold => Ok(false),
        }
    }

    /// Activate the exact Draft snapshot approved by an external caller.
    ///
    /// The existing operation journal owns prepare, projection, audit,
    /// settlement, restart reconciliation, and stale fencing. Retrying the
    /// same approval after an uncertain response returns AlreadyActivated.
    pub async fn activate_draft(
        &self,
        proposal: &MemoryActivationProposal,
        approval: MemoryApproval,
    ) -> Result<MemoryActivationOutcome> {
        if !approval.is_valid() {
            return Err(ReactError::Config(Box::new(ConfigError::ConfigFileError(
                "memory approval requires non-empty approval and reviewer identities".into(),
            ))));
        }
        let (layer, current, already_activated) = {
            let _serial = self.operation_lock.lock().await;
            self.reconcile_pending_locked().await?;
            let hot = self.parse_memory_file();
            let state = if let Some(entry) = self.find_in_hot(&hot, &proposal.key) {
                Some((MemoryLayer::Hot, entry))
            } else {
                self.typed_store
                    .get_typed(WARM_NAMESPACE, &proposal.key)
                    .await?
                    .map(|entry| (MemoryLayer::Warm, entry))
            };
            let Some((layer, current)) = state else {
                return Err(stale_merge_plan_error(
                    "memory Draft disappeared; refresh the activation proposal",
                ));
            };
            let already_activated =
                self.activation_still_current(proposal, &approval, &current, layer)?;
            (layer, current, already_activated)
        };
        if already_activated {
            return Ok(MemoryActivationOutcome::AlreadyActivated(
                MemoryActivationReceipt {
                    key: current.key,
                    approval_id: approval.approval_id,
                    draft_generation: proposal.generation,
                },
            ));
        }
        if layer != MemoryLayer::Warm
            || current.meta.status != MemoryStatus::Draft
            || current.content != proposal.content
            || current.meta != proposal.meta
            || !current.meta.provenance.is_well_formed()
            || !current.meta.has_required_evidence()
            || !evidence_is_safe(&current.meta.provenance)
            || current.meta.provenance.approval.is_some()
        {
            return Err(stale_merge_plan_error(
                "memory Draft changed; refresh the activation proposal",
            ));
        }

        let mut active = current.meta.clone();
        active.status = MemoryStatus::Active;
        active.provenance.approval = Some(approval.clone());
        self.transition_at_generation(
            &current.key,
            Some(Self::typed_value(&current.content, active)?),
            None,
            Self::change_builder(
                &current.key,
                ChangeType::Promote,
                Some("draft"),
                Some("active"),
                "explicit memory approval",
                "activate_draft",
            ),
            Some((MemoryLayer::Warm, &current)),
            Some(proposal.generation),
        )
        .await?;
        let _ = self.consider_promotion(&current.key).await?;
        Ok(MemoryActivationOutcome::Activated(
            MemoryActivationReceipt {
                key: current.key,
                approval_id: approval.approval_id,
                draft_generation: proposal.generation,
            },
        ))
    }

    // ── Promotion / Demotion ────────────────────────────────────────
    // (stage4 B1) `promote` (the cold→warm dispatcher) was removed — zero
    // callers, and the cold layer no longer exists. Warm→hot promotion still
    // happens via `consider_promotion` → `promote_warm_to_hot`.

    /// Demote a memory: hot→warm, or warm→Archived (in place).
    /// Promote a memory up one tier: cold→warm, or warm→hot.
    ///
    /// This is the inverse of [`demote`](Self::demote). stage4 collapsed the
    /// separate cold tier into `Warm`+`Archived`, so:
    /// - `Warm → Hot`: delegates to [`consider_promotion`](Self::consider_promotion).
    /// - `Cold → Warm`: only meaningful for consumers maintaining a separate
    ///   cold store under [`COLD_NAMESPACE`]; default path returns `Ok(None)`.
    /// - `Hot`: already at top, returns `Ok(None)`.
    ///
    /// Retained as pub API (aligned with Letta/MemGPT promotion + OpenClaw
    /// Dreaming) so consumers with a three-tier layout can hook in.
    pub async fn promote(&self, key: &str) -> Result<Option<LayerChangeResult>> {
        let Some((layer, _entry)) = self.locate(key).await? else {
            return Ok(None);
        };
        match layer {
            MemoryLayer::Hot => Ok(None),
            MemoryLayer::Warm => self.consider_promotion(key).await,
            MemoryLayer::Cold => {
                tracing::debug!(
                    key = %key,
                    "promote() on cold-layer entry; default path has no cold store, no-op"
                );
                Ok(None)
            }
        }
    }

    ///
    /// (stage4 B1) Warm demotion no longer moves the entry to a cold namespace;
    /// it marks `status = Archived` in place so the memory stays recallable
    /// (with decay) in the unified namespace. Revival Archived→Active is
    /// handled by Dreaming (stage 2). `update_meta` takes a full `MemoryMeta`
    /// (not a closure), hence get-modify-put.
    pub async fn demote(&self, key: &str, reason: &str) -> Result<LayerChangeResult> {
        let Some((layer, entry)) = self.locate(key).await? else {
            return Err(ReactError::Config(Box::new(ConfigError::ConfigFileError(
                format!("Memory key '{key}' not found in any layer"),
            ))));
        };

        match layer {
            MemoryLayer::Hot => self.demote_hot_to_warm(key, entry, reason).await,
            MemoryLayer::Warm => {
                let mut meta = entry.meta.clone();
                meta.status = MemoryStatus::Archived;
                self.transition(
                    key,
                    Some(Self::typed_value(&entry.content, meta)?),
                    None,
                    Self::change_builder(
                        key,
                        ChangeType::Demote,
                        Some("warm"),
                        Some("archived"),
                        reason,
                        "demote",
                    ),
                    Some((MemoryLayer::Warm, &entry)),
                )
                .await?;
                self.notify_memory_layer_change(key, "warm", "archived")
                    .await;
                Ok(LayerChangeResult {
                    key: key.to_string(),
                    from_layer: MemoryLayer::Warm,
                    to_layer: MemoryLayer::Warm,
                    reason: reason.to_string(),
                })
            }
            MemoryLayer::Cold => Err(ReactError::Config(Box::new(ConfigError::ConfigFileError(
                format!(
                    "Memory key '{key}' is in the optional cold layer; the default product path has no cold store, cannot demote further"
                ),
            )))),
        }
    }

    /// Revive an Archived warm memory back to Active in place (stage4 F1 Dreaming, G2).
    ///
    /// `MemoryMeta::is_hot_eligible()` requires `status == Active`, so Archived
    /// memories must be revived before `consider_promotion` can promote them to
    /// hot. Get-modify-put (`update_meta` takes a full `MemoryMeta`, not a
    /// closure). Returns `true` only if the entry was Archived and is now Active.
    pub async fn revive_archived(&self, key: &str) -> Result<bool> {
        if let Some(entry) = self.typed_store.get_typed(WARM_NAMESPACE, key).await?
            && entry.meta.status == MemoryStatus::Archived
            && entry.meta.provenance.is_approved()
        {
            let mut meta = entry.meta.clone();
            meta.status = MemoryStatus::Active;
            self.transition(
                key,
                Some(Self::typed_value(&entry.content, meta)?),
                None,
                Self::change_builder(
                    key,
                    ChangeType::Promote,
                    Some("archived"),
                    Some("warm"),
                    "recent high-recall activity revived archived memory",
                    "dreaming",
                ),
                Some((MemoryLayer::Warm, &entry)),
            )
            .await?;
            self.notify_memory_layer_change(key, "archived", "warm")
                .await;
            return Ok(true);
        }
        Ok(false)
    }

    /// Permanently delete a memory from whichever layer holds it (C4 fix).
    ///
    /// Looks up `key` via [`locate`](Self::locate): hot entries are removed from
    /// MEMORY.md (frontmatter + body); warm entries are deleted from the typed
    /// store via `delete_typed`. Returns `true` if a memory was found and
    /// removed, `false` if the key does not exist in any layer.
    ///
    /// This closes the namespace gap where the product `forget` tool (legacy
    /// `ForgetTool`) deleted from the per-agent namespace `[agent_name,
    /// "memories"]` while `remember` writes the unified `["agent", "memories"]`
    /// — so `forget` could never delete what `remember` stored. The layered
    /// variant (`LayeredForgetTool`) routes here.
    pub async fn delete_memory(&self, key: &str) -> Result<bool> {
        let Some((layer, entry)) = self.locate(key).await? else {
            return Ok(false);
        };

        match layer {
            MemoryLayer::Hot | MemoryLayer::Warm => {}
            MemoryLayer::Cold => {
                return Err(ReactError::Other(format!(
                    "memory key {key} belongs to an unsupported cold store"
                )));
            }
        }

        let layer_name = match layer {
            MemoryLayer::Hot => "hot",
            MemoryLayer::Warm => "warm",
            MemoryLayer::Cold => "cold",
        };
        self.transition(
            key,
            None,
            None,
            Self::change_builder(
                key,
                ChangeType::Delete,
                Some(layer_name),
                None,
                "user forget",
                "delete_memory",
            ),
            Some((layer, &entry)),
        )
        .await?;
        if layer == MemoryLayer::Hot {
            self.notify_memory_layer_change(key, "hot", "deleted").await;
        }
        Ok(true)
    }

    /// List all warm-layer typed memories matching `filter` (stage4 F1 Dreaming).
    ///
    /// Dreaming scans the unified `["agent","memories"]` namespace to find
    /// high-recall memories worth promoting and stale low-recall ones to demote.
    pub async fn list_warm_memories(&self, filter: &MemoryFilter) -> Result<Vec<TypedMemoryEntry>> {
        self.list_warm(filter).await
    }

    /// Apply a previously reviewed semantic-conflict proposal.
    ///
    /// The current warm-layer conflict must still exactly match the proposal;
    /// stale proposals fail before any mutation. The returned snapshots are the
    /// authoritative undo payload for the caller's review log.
    pub async fn apply_merge_proposal(
        &self,
        proposal: &MemoryConflictProposal,
    ) -> Result<AppliedMemoryMerge> {
        if proposal.members.len() < 2 {
            return Err(merge_plan_error(
                "memory merge proposal requires at least two members",
            ));
        }

        let entries = self
            .typed_store
            .list_typed(WARM_NAMESPACE, &MemoryFilter::new())
            .await?;
        let expected_keys: std::collections::HashSet<&str> = proposal
            .members
            .iter()
            .map(|member| member.key.as_str())
            .collect();
        if expected_keys.len() != proposal.members.len() {
            return Err(merge_plan_error(
                "memory merge proposal contains duplicate member keys",
            ));
        }

        let group = ConflictDetector::new()
            .detect(&entries)
            .into_iter()
            .find(|group| {
                if group.topic != proposal.topic || group.memory_type != proposal.memory_type {
                    return false;
                }
                let current_keys: std::collections::HashSet<&str> = group
                    .entries
                    .iter()
                    .map(|entry| entry.key.as_str())
                    .collect();
                current_keys == expected_keys
            })
            .ok_or_else(|| {
                stale_merge_plan_error("memory conflict changed; refresh Review Inbox")
            })?;

        for expected in &proposal.members {
            let current = group
                .entries
                .iter()
                .find(|entry| entry.key == expected.key)
                .ok_or_else(|| merge_plan_error("memory conflict member disappeared"))?;
            if current.content != expected.content
                || current.meta.status != expected.status
                || (current.meta.confidence - expected.confidence).abs() > f32::EPSILON
            {
                return Err(stale_merge_plan_error(
                    "memory conflict content or metadata changed; refresh Review Inbox",
                ));
            }
        }

        let current_proposal = MemoryConflictProposal::from_group(&group)
            .ok_or_else(|| merge_plan_error("memory conflict has no primary candidate"))?;
        if current_proposal.recommended_primary_key != proposal.recommended_primary_key {
            return Err(stale_merge_plan_error(
                "memory conflict recommendation changed; refresh Review Inbox",
            ));
        }

        let before = group
            .entries
            .iter()
            .map(|entry| MemoryMergeSnapshot {
                key: entry.key.clone(),
                content: entry.content.clone(),
                meta: entry.meta.clone(),
            })
            .collect();
        let result = MemoryMerger::new(self).merge_group(&group).await?;
        Ok(AppliedMemoryMerge {
            primary_key: result.primary_key,
            superseded_keys: result.superseded_keys,
            before,
            batch_id: result.batch_id,
        })
    }

    pub(super) async fn commit_merge_group(&self, group: &ConflictGroup) -> Result<MergeResult> {
        let ordered = ordered_conflict_entries(group);
        let Some(primary) = ordered.first() else {
            return Ok(MergeResult {
                primary_key: String::new(),
                superseded_keys: Vec::new(),
                batch_id: None,
            });
        };
        if ordered.len() < 2 {
            return Ok(MergeResult {
                primary_key: primary.key.clone(),
                superseded_keys: Vec::new(),
                batch_id: None,
            });
        }
        let _serial = self.operation_lock.lock().await;
        self.reconcile_pending_locked().await?;
        let revision_count = ordered.iter().fold(0_u32, |sum, entry| {
            sum.saturating_add(entry.meta.revision_count)
        });
        let mut operations = Vec::with_capacity(ordered.len());
        for (index, entry) in ordered.iter().enumerate() {
            let current = self
                .typed_store
                .inner()
                .get(WARM_NAMESPACE, &entry.key)
                .await?
                .ok_or_else(|| stale_merge_plan_error("memory merge member disappeared"))?;
            if current.value != entry.raw.value
                || Self::hot_value(&self.parse_memory_file(), &entry.key).is_some()
            {
                return Err(stale_merge_plan_error(
                    "memory merge member changed; refresh Review Inbox",
                ));
            }
            let mut meta = entry.meta.clone();
            let superseded_by = if index == 0 {
                None
            } else {
                Some(primary.key.clone())
            };
            if index == 0 {
                meta.revision_count = revision_count.max(meta.revision_count);
            } else {
                meta.status = MemoryStatus::Superseded;
                meta.superseded_by = superseded_by.clone();
            }
            let id = uuid::Uuid::new_v4().to_string();
            let reason = if index == 0 {
                format!(
                    "primary survivor of {}-way merge on topic '{}'",
                    ordered.len(),
                    group.topic
                )
            } else {
                format!(
                    "superseded by '{}' during merge on topic '{}'",
                    primary.key, group.topic
                )
            };
            let audit = ChangeEntryBuilder::new(EntityType::Memory, &entry.key, ChangeType::Merge)
                .reason(reason).trigger("explicit_memory_merge")
                .after(serde_json::json!({ "superseded_by": superseded_by, "group_size": ordered.len() }))
                .build_with(id.clone(), chrono::Utc::now());
            operations.push(MemoryOperation {
                id,
                key: entry.key.clone(),
                warm_before: Some(current.value),
                warm_after: Some(Self::typed_value(&entry.content, meta)?),
                hot_before: None,
                hot_after: None,
                audit,
            });
        }
        let batch = MemoryOperationBatch {
            id: uuid::Uuid::new_v4().to_string(),
            operations,
            origin: None,
        };
        self.operation_journal()?.prepare_batch(batch.clone())?;
        self.settle_batch(&batch).await?;
        Ok(MergeResult {
            primary_key: primary.key.clone(),
            superseded_keys: ordered
                .iter()
                .skip(1)
                .map(|entry| entry.key.clone())
                .collect(),
            batch_id: Some(batch.id),
        })
    }

    /// Legacy merge undo entry point.
    ///
    /// Snapshot-only restoration cannot prove a journal tip and is therefore
    /// retired. Call [`Self::rollback_memory`] with the `batch_id` returned by
    /// [`Self::apply_merge_proposal`] so the complete merge group is reverted
    /// through the canonical journal authority.
    #[deprecated(note = "use rollback_memory with AppliedMemoryMerge::batch_id")]
    pub async fn restore_merge_snapshots(&self, _snapshots: &[MemoryMergeSnapshot]) -> Result<()> {
        Err(merge_plan_error(
            "snapshot-only merge restore is retired; use rollback_memory with the merge batch ID",
        ))
    }

    /// Consider promoting a warm entry to hot if eligible and space exists.
    ///
    /// Called after every new memory write.
    pub async fn consider_promotion(&self, key: &str) -> Result<Option<LayerChangeResult>> {
        let Some((layer, entry)) = self.locate(key).await? else {
            return Ok(None);
        };

        if layer != MemoryLayer::Warm {
            return Ok(None);
        }

        // Check eligibility
        if !entry.meta.is_hot_eligible() {
            return Ok(None);
        }

        // Check trust level — untrusted content cannot auto-promote to hot
        use super::security::InputTrustLevel;
        if !InputTrustLevel::from_provenance(&entry.meta.provenance).can_auto_promote() {
            return Ok(None);
        }

        self.promote_warm_to_hot(key, entry).await
    }

    /// Enforce the hot layer token budget by demoting lowest-priority entries.
    ///
    /// Called after every hot-layer write. Returns a list of demotion results.
    pub async fn enforce_hot_budget(&self) -> Result<Vec<LayerChangeResult>> {
        let mut results = Vec::new();
        loop {
            let file = self.parse_memory_file();
            if estimate_tokens(&file.body) <= HOT_TOKEN_BUDGET {
                break;
            }
            let candidate = file.entries.iter().max_by(|a, b| {
                Self::demotion_score(a)
                    .partial_cmp(&Self::demotion_score(b))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let Some(candidate) = candidate else { break };
            let key = candidate.key.clone();
            let entry = self
                .find_in_hot(&file, &key)
                .ok_or_else(|| ReactError::Other(format!("missing hot entry {key}")))?;
            results.push(
                self.demote_hot_to_warm(&key, entry, "hot layer budget enforcement")
                    .await?,
            );
        }

        Ok(results)
    }

    /// Compute a demotion priority score for a hot entry.
    ///
    /// Higher score = more likely to be demoted.
    fn demotion_score(meta: &HotEntryMeta) -> f32 {
        let confidence_factor = 1.0 - meta.confidence;
        let stability_factor = 1.0 - meta.stability;

        // Staleness from time since last promotion (simplified — no access tracking in MEMORY.md)
        // Use a fixed moderate staleness since we can't track access without the Store
        let staleness_factor = 0.3;

        // Recency — we don't have last_accessed, so use stability as proxy
        let recency_factor = 1.0 - meta.stability;

        confidence_factor * 0.35
            + stability_factor * 0.25
            + staleness_factor * 0.20
            + recency_factor * 0.20
    }

    // ── Write to warm layer ─────────────────────────────────────────

    /// Save a typed Draft to the warm layer. Activation is a separate,
    /// generation-fenced operation.
    ///
    /// Returns `Ok(Some(result))` if the memory was promoted to hot.
    pub async fn write_memory(
        &self,
        key: &str,
        content: &str,
        mut meta: MemoryMeta,
    ) -> Result<Option<LayerChangeResult>> {
        if (meta.provenance.trust == MemoryTrust::LegacyUnknown
            && !meta.provenance.evidence.is_empty())
            || (meta.provenance.trust != MemoryTrust::LegacyUnknown
                && !meta.provenance.is_well_formed())
        {
            return Err(ReactError::Config(Box::new(ConfigError::ConfigFileError(
                "memory provenance does not match its exact evidence roles".into(),
            ))));
        }
        if !evidence_is_safe(&meta.provenance) {
            return Err(ReactError::Config(Box::new(ConfigError::ConfigFileError(
                "memory evidence contains a secret or instruction-like content".into(),
            ))));
        }
        // A write only proposes content. The caller cannot smuggle an Active
        // status or approval receipt around activate_draft's journal fence.
        meta.status = MemoryStatus::Draft;
        meta.provenance.approval = None;
        // Security check: scan secrets, detect injection, rate limit, trust assignment.
        let trust = InputTrustLevel::from_provenance(&meta.provenance);
        let verdict = self.security_guard.check_memory_write(content, trust);

        if !verdict.allowed {
            return Err(ReactError::Config(Box::new(ConfigError::ConfigFileError(
                format!(
                    "Memory write blocked by security guard: {}",
                    verdict
                        .reason
                        .unwrap_or_else(|| "unknown reason".to_string())
                ),
            ))));
        }

        // Use sanitized content if secrets were redacted, otherwise original.
        let safe_content = verdict
            .sanitized_content
            .unwrap_or_else(|| content.to_string());
        meta.risk = stricter_memory_risk(meta.risk, verdict.risk_level);

        self.transition(
            key,
            Some(Self::typed_value(&safe_content, meta.clone())?),
            None,
            Self::change_builder(
                key,
                ChangeType::Create,
                None,
                Some("warm"),
                &format!("memory via {}", meta.source.as_str().unwrap_or("unknown")),
                "write_memory",
            ),
            None,
        )
        .await?;

        self.notify_memory_write(key, meta.source.as_str().unwrap_or("unknown"))
            .await;

        // Consider promotion
        self.consider_promotion(key).await
    }

    // ── Search ──────────────────────────────────────────────────────

    /// Search across hot and warm layers.
    ///
    /// Hot results come first (higher priority), then warm results.
    pub async fn search_layered(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<(MemoryLayer, TypedMemoryEntry)>> {
        self.search_settled(query, limit, false).await
    }

    /// Return every approved Hot entry plus relevant Warm entries for a turn.
    pub async fn recall_for_context(
        &self,
        query: &str,
        warm_limit: usize,
    ) -> Result<Vec<(MemoryLayer, TypedMemoryEntry)>> {
        self.search_settled(query, warm_limit, true).await
    }

    async fn search_settled(
        &self,
        query: &str,
        limit: usize,
        include_all_hot: bool,
    ) -> Result<Vec<(MemoryLayer, TypedMemoryEntry)>> {
        let _serial = self.operation_lock.lock().await;
        self.reconcile_pending_locked().await?;
        let mut results = Vec::new();

        // Search hot layer (keyword match on body)
        let file = self.parse_memory_file();
        let query_lower = query.to_lowercase();
        for meta in &file.entries {
            if !include_all_hot && results.len() >= limit {
                break;
            }
            // Hot context is resident; tool search filters it by key, topic, or body.
            if meta.to_memory_meta().is_recallable()
                && (include_all_hot
                    || meta.key.to_lowercase().contains(&query_lower)
                    || meta.topic.to_lowercase().contains(&query_lower)
                    || extract_hot_value_content(&file.body, meta)
                        .to_lowercase()
                        .contains(&query_lower))
                && let Some(entry) = self.hot_meta_to_entry(meta, &file.body)
            {
                results.push((MemoryLayer::Hot, entry));
            }
        }

        // (stage4 D1) Warm layer via the unified composite-score recall entry —
        // same ranking / Superseded filter / recall_count as the auto path.
        let remaining = if include_all_hot {
            limit
        } else {
            limit.saturating_sub(results.len())
        };
        if remaining > 0 {
            let reca =
                crate::evolution::recall::MemoryRecaller::new(self.typed_store.inner().clone());
            for item in reca.recall(query, remaining).await? {
                let entry = TypedMemoryEntry::from_store_item(item);
                results.push((MemoryLayer::Warm, entry));
            }
        }

        Ok(results)
    }

    // ── Hot layer file I/O ──────────────────────────────────────────

    /// Parse MEMORY.md into structured form.
    ///
    /// Returns a default (empty) MemoryFile if the file doesn't exist or can't
    /// be parsed.
    ///
    /// Note: this performs an unlocked read. For any read-modify-write that
    /// must be atomic, use [`with_locked_hot_file`] instead.
    fn parse_memory_file(&self) -> MemoryFile {
        let raw = match std::fs::read_to_string(&self.hot_path) {
            Ok(content) => content,
            Err(e) => {
                if e.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!(
                        path = ?self.hot_path,
                        error = %e,
                        "MEMORY.md could not be read; falling back to default"
                    );
                }
                return MemoryFile::default();
            }
        };

        parse_memory_md(&raw)
    }

    /// Write the MemoryFile back to disk.
    ///
    /// Atomic (temp + rename) and `fsync`-ed so a crash cannot leave a torn
    /// or empty MEMORY.md.
    fn write_memory_file(&self, file: &MemoryFile) -> std::io::Result<()> {
        // Ensure parent directory exists
        if let Some(parent) = self.hot_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let content = format_memory_md(file);
        // Atomic write: write to temp file, fsync, then rename.
        let tmp_path = self.hot_path.with_extension("md.tmp");
        {
            use std::io::Write;
            let mut tmp = std::fs::File::create(&tmp_path)?;
            tmp.write_all(content.as_bytes())?;
            // fsync before rename so a crash after rename cannot expose an
            // empty file (the prior version had no fsync).
            tmp.sync_all()?;
        }
        std::fs::rename(&tmp_path, &self.hot_path)?;
        if let Some(parent) = self.hot_path.parent() {
            std::fs::File::open(parent)?.sync_all()?;
        }

        Ok(())
    }

    /// Run a synchronous closure with exclusive access to the MEMORY.md hot
    /// layer. Holds both the in-process Mutex and a cross-process flock for
    /// the full load → mutate → save.
    ///
    /// For async callers that need to `.await` between mutate and save (e.g.
    /// `enforce_hot_budget`, which writes to the warm Store mid-critical
    /// section), use [`lock_hot_file`] to get a guard and hold it across the
    /// awaits instead.
    /// Acquire both the in-process Mutex and a cross-process flock on the
    /// MEMORY.md file, returning a guard holding the freshly-loaded
    /// `MemoryFile`.
    ///
    /// The guard can be held across `.await` points (it contains only
    /// `tokio::sync::MutexGuard` + a `File`, both `Send`), which lets
    /// `enforce_hot_budget`-style functions do async Store writes inside the
    /// critical section. Call [`HotFileGuard::commit`] to persist, or drop to
    /// discard.
    ///
    /// Note: the flock acquisition below blocks the calling thread. This is
    /// acceptable because curator/memory writes are infrequent and
    /// low-contention; the bounded retry loop prevents indefinite wedging.
    async fn lock_hot_file(&self) -> std::io::Result<HotFileGuard<'_>> {
        // In-process serialization first. tokio::sync::Mutex so the guard is
        // `Send` and can cross `.await`s.
        let inproc = self.hot_lock.lock().await;

        // Cross-process advisory lock on a sidecar file.
        let lock_path = self.hot_path.with_extension("md.lock");
        let lock_file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|e| {
                tracing::warn!(error = %e, "hot-file lock sidecar open failed; continuing lockless");
                e
            })?;
        // Bounded flock attempt. flock blocks per call; cap total attempts so
        // a wedged peer cannot hang us forever.
        use fs2::FileExt;
        const MAX_ATTEMPTS: u32 = 600;
        const BACKOFF: std::time::Duration = std::time::Duration::from_secs(1);
        let mut got_lock = false;
        for _ in 0..MAX_ATTEMPTS {
            match lock_file.try_lock_exclusive() {
                Ok(()) => {
                    got_lock = true;
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    tokio::time::sleep(BACKOFF).await;
                }
                Err(error) => return Err(error),
            }
        }
        if !got_lock {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                format!("MEMORY.md lock unavailable after {MAX_ATTEMPTS} attempts"),
            ));
        }

        let file = self.parse_memory_file();
        Ok(HotFileGuard {
            #[cfg(test)]
            manager: self,
            file,
            _inproc: inproc,
            _lock_file: lock_file,
        })
    }

    /// Add an entry to the MEMORY.md hot layer.
    #[cfg(test)]
    async fn add_to_hot(&self, entry: &TypedMemoryEntry) -> std::io::Result<()> {
        let mut guard = self.lock_hot_file().await?;
        let value = HotValue {
            meta: Self::hot_meta(entry),
            content: entry.content.clone(),
        };
        Self::set_hot_value(&mut guard.file, &entry.key, Some(&value))
            .map_err(std::io::Error::other)?;

        guard.commit()
    }

    /// Find a hot entry by key in the parsed file.
    fn find_in_hot(&self, file: &MemoryFile, key: &str) -> Option<TypedMemoryEntry> {
        let meta = file.entries.iter().find(|e| e.key == key)?;
        self.hot_meta_to_entry(meta, &file.body)
    }

    /// Reconstruct a TypedMemoryEntry from a HotEntryMeta and the file body.
    fn hot_meta_to_entry(&self, meta: &HotEntryMeta, body: &str) -> Option<TypedMemoryEntry> {
        let content = extract_hot_value_content(body, meta);
        Some(TypedMemoryEntry {
            key: meta.key.clone(),
            content,
            meta: meta.to_memory_meta(),
            raw: echo_core::memory::store::StoreItem::new(
                WARM_NAMESPACE.iter().map(|s| s.to_string()).collect(),
                meta.key.clone(),
                serde_json::Value::Null,
            ),
        })
    }

    // ── Internal promotion/demotion methods ─────────────────────────

    async fn promote_warm_to_hot(
        &self,
        key: &str,
        entry: TypedMemoryEntry,
    ) -> Result<Option<LayerChangeResult>> {
        let hot = HotValue {
            meta: Self::hot_meta(&entry),
            content: entry.content.clone(),
        };
        self.transition(
            key,
            None,
            Some(hot),
            Self::change_builder(
                key,
                ChangeType::Promote,
                Some("warm"),
                Some("hot"),
                "warm-to-hot promotion (eligible)",
                "promote",
            ),
            Some((MemoryLayer::Warm, &entry)),
        )
        .await?;

        self.notify_memory_layer_change(key, "warm", "hot").await;

        // Enforce budget after publishing the promotion. Any resulting
        // hot-to-warm events then retain their real chronological order.
        self.enforce_hot_budget().await?;

        Ok(Some(LayerChangeResult {
            key: key.to_string(),
            from_layer: MemoryLayer::Warm,
            to_layer: MemoryLayer::Hot,
            reason: "warm→hot promotion (eligible)".to_string(),
        }))
    }

    async fn demote_hot_to_warm(
        &self,
        key: &str,
        entry: TypedMemoryEntry,
        reason: &str,
    ) -> Result<LayerChangeResult> {
        self.transition(
            key,
            Some(Self::typed_value(&entry.content, entry.meta.clone())?),
            None,
            Self::change_builder(
                key,
                ChangeType::Demote,
                Some("hot"),
                Some("warm"),
                reason,
                "demote",
            ),
            Some((MemoryLayer::Hot, &entry)),
        )
        .await?;

        self.notify_memory_layer_change(key, "hot", "warm").await;

        Ok(LayerChangeResult {
            key: key.to_string(),
            from_layer: MemoryLayer::Hot,
            to_layer: MemoryLayer::Warm,
            reason: reason.to_string(),
        })
    }

    fn hot_meta(entry: &TypedMemoryEntry) -> HotEntryMeta {
        HotEntryMeta {
            key: entry.key.clone(),
            memory_type: entry.meta.memory_type,
            confidence: entry.meta.confidence,
            stability: entry.meta.stability,
            recall_weight: entry.meta.recall_weight,
            source: entry.meta.source,
            provenance: entry.meta.provenance.clone(),
            topic: entry.meta.topic.clone(),
            risk: entry.meta.risk,
            revision_count: entry.meta.revision_count,
            recall_count: entry.meta.recall_count,
            last_recalled_at: entry.meta.last_recalled_at,
            content_json: entry.content.trim() != entry.content
                || entry.content.contains('\n')
                || entry.content.contains('\r'),
            last_promoted: crate::utils::time::now_local().to_rfc3339(),
        }
    }

    fn change_builder(
        key: &str,
        change_type: ChangeType,
        from: Option<&str>,
        to: Option<&str>,
        reason: &str,
        trigger: &str,
    ) -> ChangeEntryBuilder {
        let mut builder = ChangeEntryBuilder::new(EntityType::Memory, key, change_type);

        if let Some(f) = from {
            builder = builder.before(serde_json::json!({ "layer": f }));
        }
        if let Some(t) = to {
            builder = builder.after(serde_json::json!({ "layer": t }));
        }
        builder = builder.reason(reason.to_string());
        builder = builder.trigger(trigger.to_string());

        builder.reason(reason).trigger(trigger)
    }

    async fn notify_memory_write(&self, key: &str, source: &str) {
        if let Some(observer) = &self.evolution_observer {
            observer.on_memory_write(key, source).await;
        }
    }

    async fn notify_memory_layer_change(&self, key: &str, from_layer: &str, to_layer: &str) {
        if let Some(observer) = &self.evolution_observer {
            observer
                .on_memory_layer_change(key, from_layer, to_layer)
                .await;
        }
    }
}

// ── Parsing functions ──────────────────────────────────────────────────

/// Parse a MEMORY.md file into structured form.
///
/// The file is expected to have YAML frontmatter between `---` markers,
/// followed by a markdown body with bullet entries.
fn parse_memory_md(raw: &str) -> MemoryFile {
    let trimmed = raw.trim_start();

    // Check for YAML frontmatter
    if !trimmed.starts_with("---") {
        // No frontmatter — treat entire content as body
        return MemoryFile {
            entries: Vec::new(),
            body: raw.to_string(),
        };
    }

    // Find the closing ---
    let rest = &trimmed[3..]; // skip opening ---
    let end_marker = rest.find("\n---").or_else(|| rest.find("\r\n---"));

    let (frontmatter_str, body_str) = match end_marker {
        Some(pos) => {
            let fm = &rest[..pos];
            let after_marker = &rest[pos + 4..]; // skip \n---
            let body = after_marker
                .trim_start_matches('\n')
                .trim_start_matches('\r');
            (fm, body)
        }
        None => {
            // No closing marker — treat as body only
            return MemoryFile {
                entries: Vec::new(),
                body: raw.to_string(),
            };
        }
    };

    let entries = match serde_yaml_ng::from_str::<MemoryFileFrontmatter>(frontmatter_str) {
        Ok(fm) => fm.entries.unwrap_or_default(),
        Err(_) => Vec::new(), // Malformed frontmatter — graceful fallback
    };

    MemoryFile {
        entries,
        body: body_str.to_string(),
    }
}

/// Format a MemoryFile back to the MEMORY.md format.
fn format_memory_md(file: &MemoryFile) -> String {
    let mut out = String::new();

    if !file.entries.is_empty() {
        out.push_str("---\n");
        let fm = MemoryFileFrontmatter {
            entries: Some(file.entries.clone()),
        };
        match serde_yaml_ng::to_string(&fm) {
            Ok(yaml) => {
                // serde_yaml adds "---\n" prefix, strip it
                let yaml = yaml.trim_start_matches("---\n");
                out.push_str(yaml);
            }
            Err(_) => {
                // Fallback: empty frontmatter
                out.push_str("entries: []\n");
            }
        }
        out.push_str("---\n\n");
    }

    out.push_str(&file.body);

    out
}

/// Helper struct for YAML frontmatter deserialization.
#[derive(Debug, Serialize, Deserialize)]
struct MemoryFileFrontmatter {
    #[serde(default)]
    entries: Option<Vec<HotEntryMeta>>,
}

/// Extract the content of a specific hot entry from the body by key.
fn extract_hot_entry_content(body: &str, key: &str) -> String {
    let pattern = format!("- **[{key}]** ");
    body.lines()
        .find_map(|line| line.strip_prefix(&pattern).map(str::to_owned))
        .unwrap_or_default()
}

fn extract_hot_value_content(body: &str, meta: &HotEntryMeta) -> String {
    let raw = extract_hot_entry_content(body, &meta.key);
    if meta.content_json {
        serde_json::from_str::<String>(&raw).unwrap_or(raw)
    } else {
        raw
    }
}

/// Estimate the token count of a string.
///
/// Uses a conservative heuristic: ~4 chars per token for Latin,
/// ~1.5 chars per token for CJK. We use 3 chars/token as a middle ground.
fn estimate_tokens(text: &str) -> usize {
    let latin_chars = text.chars().filter(|c| c.is_ascii()).count();
    let cjk_chars = text.chars().filter(|c| !c.is_ascii()).count();
    // Latin: ~4 chars/token, CJK: ~1.5 chars/token
    let latin_tokens = latin_chars / 4;
    let cjk_tokens = (cjk_chars as f32 / 1.5).ceil() as usize;
    latin_tokens + cjk_tokens
}

/// Extension for MemorySource to provide as_str.
trait MemorySourceExt {
    fn as_str(&self) -> Option<&'static str>;
}

impl MemorySourceExt for MemorySource {
    fn as_str(&self) -> Option<&'static str> {
        match self {
            MemorySource::UserCorrection => Some("user_correction"),
            MemorySource::ErrorResolution => Some("error_resolution"),
            MemorySource::RepeatedWorkflow => Some("repeated_workflow"),
            MemorySource::ExplicitSave => Some("explicit_save"),
            MemorySource::AutoExtracted => Some("auto_extracted"),
            MemorySource::L3Promotion => Some("l3_promotion"),
        }
    }
}

/// Extension for InputTrustLevel to derive from MemorySource.
mod security_ext {
    use super::super::security::InputTrustLevel;
    use echo_core::memory::types::{MemoryProvenance, MemoryTrust};

    impl InputTrustLevel {
        /// Derive the trust level from the memory source.
        pub fn from_provenance(provenance: &MemoryProvenance) -> Self {
            match provenance.trust {
                MemoryTrust::User => InputTrustLevel::Trusted,
                MemoryTrust::Assistant => InputTrustLevel::Assistant,
                MemoryTrust::Tool | MemoryTrust::Mixed | MemoryTrust::LegacyUnknown => {
                    InputTrustLevel::Untrusted
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::audit::{ChangeEntry, ChangeRecordOutcome, JsonlChangeLog};
    use super::*;
    use echo_core::memory::types::{MemoryMeta, MemorySource, MemoryType};
    use echo_state::memory::store::{FileStore, InMemoryStore};
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use tokio::sync::Notify;

    /// A no-op ChangeLog for testing.
    struct NullChangeLog;
    impl ChangeLog for NullChangeLog {
        fn record(&self, _entry: ChangeEntry) -> Result<()> {
            Ok(())
        }
        fn record_idempotent(
            &self,
            _entry: ChangeEntry,
        ) -> Result<super::super::audit::ChangeRecordOutcome> {
            Ok(super::super::audit::ChangeRecordOutcome::AlreadyRecorded)
        }
        fn query(&self, _filter: &super::super::audit::ChangeFilter) -> Result<Vec<ChangeEntry>> {
            Ok(Vec::new())
        }
        fn latest_for(
            &self,
            _entity_type: EntityType,
            _entity_key: &str,
        ) -> Result<Option<ChangeEntry>> {
            Ok(None)
        }
        fn len(&self) -> usize {
            0
        }
    }

    #[derive(Default)]
    struct RecordingObserver {
        changes: StdMutex<Vec<(String, String, String)>>,
    }

    impl RecordingObserver {
        fn changes(&self) -> Vec<(String, String, String)> {
            self.changes
                .lock()
                .map(|changes| changes.clone())
                .unwrap_or_default()
        }
    }

    impl EvolutionObserver for RecordingObserver {
        fn on_memory_layer_change<'a>(
            &'a self,
            key: &'a str,
            from_layer: &'a str,
            to_layer: &'a str,
        ) -> BoxFuture<'a, ()> {
            Box::pin(async move {
                if let Ok(mut changes) = self.changes.lock() {
                    changes.push((
                        key.to_string(),
                        from_layer.to_string(),
                        to_layer.to_string(),
                    ));
                }
            })
        }
    }

    struct FailingChangeLog;

    struct FailOnceChangeLog {
        inner: JsonlChangeLog,
        fail_next: AtomicBool,
    }

    struct FailSecondChangeLog {
        inner: JsonlChangeLog,
        calls: AtomicUsize,
    }

    struct BlockingStore {
        inner: InMemoryStore,
        block_next_put: AtomicBool,
        put_started: Notify,
        release_put: Notify,
    }

    impl BlockingStore {
        fn new() -> Self {
            Self {
                inner: InMemoryStore::new(),
                block_next_put: AtomicBool::new(false),
                put_started: Notify::new(),
                release_put: Notify::new(),
            }
        }

        fn block_next_put(&self) {
            self.block_next_put.store(true, Ordering::SeqCst);
        }
    }

    impl Store for BlockingStore {
        fn put<'a>(
            &'a self,
            namespace: &'a [&'a str],
            key: &'a str,
            value: serde_json::Value,
        ) -> futures::future::BoxFuture<'a, echo_core::error::Result<()>> {
            Box::pin(async move {
                if self.block_next_put.swap(false, Ordering::SeqCst) {
                    self.put_started.notify_one();
                    self.release_put.notified().await;
                }
                self.inner.put(namespace, key, value).await
            })
        }

        fn compare_and_put<'a>(
            &'a self,
            namespace: &'a [&'a str],
            key: &'a str,
            expected: Option<serde_json::Value>,
            value: serde_json::Value,
        ) -> futures::future::BoxFuture<
            'a,
            echo_core::error::Result<echo_core::memory::store::StoreCompareAndPutOutcome>,
        > {
            self.inner.compare_and_put(namespace, key, expected, value)
        }

        fn get<'a>(
            &'a self,
            namespace: &'a [&'a str],
            key: &'a str,
        ) -> futures::future::BoxFuture<
            'a,
            echo_core::error::Result<Option<echo_core::memory::store::StoreItem>>,
        > {
            self.inner.get(namespace, key)
        }

        fn search<'a>(
            &'a self,
            namespace: &'a [&'a str],
            query: &'a str,
            limit: usize,
        ) -> futures::future::BoxFuture<
            'a,
            echo_core::error::Result<Vec<echo_core::memory::store::StoreItem>>,
        > {
            self.inner.search(namespace, query, limit)
        }

        fn delete<'a>(
            &'a self,
            namespace: &'a [&'a str],
            key: &'a str,
        ) -> futures::future::BoxFuture<'a, echo_core::error::Result<bool>> {
            self.inner.delete(namespace, key)
        }

        fn list_namespaces<'a>(
            &'a self,
            prefix: Option<&'a [&'a str]>,
        ) -> futures::future::BoxFuture<'a, echo_core::error::Result<Vec<Vec<String>>>> {
            self.inner.list_namespaces(prefix)
        }

        fn list<'a>(
            &'a self,
            namespace: &'a [&'a str],
        ) -> futures::future::BoxFuture<
            'a,
            echo_core::error::Result<Vec<echo_core::memory::store::StoreItem>>,
        > {
            self.inner.list(namespace)
        }
    }

    impl ChangeLog for FailSecondChangeLog {
        fn record(&self, entry: ChangeEntry) -> Result<()> {
            self.record_idempotent(entry).map(|_| ())
        }

        fn record_idempotent(&self, entry: ChangeEntry) -> Result<ChangeRecordOutcome> {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 1 {
                return Err(merge_plan_error("injected second audit failure"));
            }
            self.inner.record_idempotent(entry)
        }

        fn query(&self, filter: &super::super::audit::ChangeFilter) -> Result<Vec<ChangeEntry>> {
            self.inner.query(filter)
        }

        fn latest_for(&self, entity_type: EntityType, key: &str) -> Result<Option<ChangeEntry>> {
            self.inner.latest_for(entity_type, key)
        }

        fn len(&self) -> usize {
            self.inner.len()
        }
    }

    impl ChangeLog for FailOnceChangeLog {
        fn record(&self, entry: ChangeEntry) -> Result<()> {
            self.record_idempotent(entry).map(|_| ())
        }

        fn record_idempotent(&self, entry: ChangeEntry) -> Result<ChangeRecordOutcome> {
            if self.fail_next.swap(false, Ordering::SeqCst) {
                return Err(merge_plan_error("injected change-log failure"));
            }
            self.inner.record_idempotent(entry)
        }

        fn query(&self, filter: &super::super::audit::ChangeFilter) -> Result<Vec<ChangeEntry>> {
            self.inner.query(filter)
        }

        fn latest_for(
            &self,
            entity_type: EntityType,
            entity_key: &str,
        ) -> Result<Option<ChangeEntry>> {
            self.inner.latest_for(entity_type, entity_key)
        }

        fn len(&self) -> usize {
            self.inner.len()
        }
    }

    impl ChangeLog for FailingChangeLog {
        fn record(&self, _entry: ChangeEntry) -> Result<()> {
            Err(merge_plan_error("injected change-log failure"))
        }

        fn record_idempotent(
            &self,
            _entry: ChangeEntry,
        ) -> Result<super::super::audit::ChangeRecordOutcome> {
            Err(merge_plan_error("injected change-log failure"))
        }

        fn query(&self, _filter: &super::super::audit::ChangeFilter) -> Result<Vec<ChangeEntry>> {
            Ok(Vec::new())
        }

        fn latest_for(
            &self,
            _entity_type: EntityType,
            _entity_key: &str,
        ) -> Result<Option<ChangeEntry>> {
            Ok(None)
        }

        fn len(&self) -> usize {
            0
        }
    }

    fn make_manager() -> MemoryLayerManager {
        let dir = tempfile::tempdir().expect("tempdir");
        let dir_path = dir.keep();
        let store = Arc::new(InMemoryStore::new());
        let change_log = Box::new(NullChangeLog);
        MemoryLayerManager::new(dir_path, store, change_log)
    }

    fn user_draft_meta(content: &str) -> MemoryMeta {
        MemoryMeta::new(
            MemoryType::ProjectFact,
            MemorySource::AutoExtracted,
            "project",
        )
        .with_confidence(0.9)
        .with_status(MemoryStatus::Draft)
        .with_provenance(MemoryProvenance::draft(
            MemoryTrust::User,
            vec![echo_core::memory::MemoryEvidence::new(
                echo_core::memory::MemoryEvidenceRole::User,
                content,
            )],
        ))
    }

    fn approved_test_meta(meta: MemoryMeta, content: &str) -> MemoryMeta {
        let mut provenance = MemoryProvenance::draft(
            MemoryTrust::User,
            vec![echo_core::memory::MemoryEvidence::new(
                echo_core::memory::MemoryEvidenceRole::User,
                content,
            )],
        );
        provenance.approval = Some(approval("approved-test-memory"));
        meta.with_provenance(provenance)
    }

    async fn write_approved_memory(
        manager: &MemoryLayerManager,
        key: &str,
        content: &str,
        meta: MemoryMeta,
    ) -> Result<()> {
        let draft = meta.with_provenance(MemoryProvenance::draft(
            MemoryTrust::User,
            vec![echo_core::memory::MemoryEvidence::new(
                echo_core::memory::MemoryEvidenceRole::User,
                content,
            )],
        ));
        manager.write_memory(key, content, draft).await?;
        let proposal = manager
            .preview_activation(key)
            .await?
            .ok_or_else(|| merge_plan_error(format!("missing Draft for {key}")))?;
        manager
            .activate_draft(&proposal, approval(&format!("approval-{key}")))
            .await?;
        Ok(())
    }

    fn approval(id: &str) -> MemoryApproval {
        MemoryApproval::new(id, "framework-test-reviewer", 1_750_000_000)
    }

    #[tokio::test]
    async fn assistant_claimed_user_preference_cannot_activate() -> Result<()> {
        let manager = make_manager();
        let content = "User prefers Rust";
        let meta = MemoryMeta::new(
            MemoryType::UserPreference,
            MemorySource::AutoExtracted,
            "style",
        )
        .with_provenance(MemoryProvenance::draft(
            MemoryTrust::Assistant,
            vec![echo_core::memory::MemoryEvidence::new(
                echo_core::memory::MemoryEvidenceRole::Assistant,
                content,
            )],
        ));
        manager.write_memory("claimed-pref", content, meta).await?;
        assert!(manager.preview_activation("claimed-pref").await?.is_none());
        let (_, entry) = manager
            .locate("claimed-pref")
            .await?
            .ok_or_else(|| merge_plan_error("Draft disappeared"))?;
        let generation = manager
            .latest_key_generation("claimed-pref")?
            .ok_or_else(|| merge_plan_error("Draft generation missing"))?;
        let proposal = MemoryActivationProposal {
            key: entry.key,
            content: entry.content,
            meta: entry.meta,
            generation,
        };
        assert!(
            manager
                .activate_draft(&proposal, approval("invalid-pref"))
                .await
                .is_err()
        );
        Ok(())
    }

    #[tokio::test]
    async fn draft_activation_is_exact_recallable_and_idempotent() -> Result<()> {
        let root = tempfile::tempdir().map_err(ReactError::from)?;
        let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let manager = MemoryLayerManager::new(
            root.path().to_path_buf(),
            store.clone(),
            Box::new(NullChangeLog),
        );
        let content = "The project uses Rust for durable services";
        manager
            .write_memory("activation", content, user_draft_meta(content))
            .await?;
        assert!(
            crate::evolution::MemoryRecaller::new(store.clone())
                .recall("durable services", 5)
                .await?
                .is_empty()
        );

        let proposal = manager
            .preview_activation("activation")
            .await?
            .ok_or_else(|| merge_plan_error("activation proposal missing"))?;
        let receipt = approval("approval-1");
        assert!(matches!(
            manager.activate_draft(&proposal, receipt.clone()).await?,
            MemoryActivationOutcome::Activated(_)
        ));
        assert_eq!(
            crate::evolution::MemoryRecaller::new(store)
                .recall("durable services", 5)
                .await?
                .len(),
            1
        );
        assert!(matches!(
            manager.activate_draft(&proposal, receipt).await?,
            MemoryActivationOutcome::AlreadyActivated(_)
        ));
        Ok(())
    }

    #[tokio::test]
    async fn stale_activation_proposal_cannot_overwrite_newer_draft() -> Result<()> {
        let manager = make_manager();
        let old = "The project uses Rust";
        manager
            .write_memory("stale-activation", old, user_draft_meta(old))
            .await?;
        let proposal = manager
            .preview_activation("stale-activation")
            .await?
            .ok_or_else(|| merge_plan_error("activation proposal missing"))?;

        let newer = "The project uses Rust and Tokio";
        manager
            .write_memory("stale-activation", newer, user_draft_meta(newer))
            .await?;
        let error = manager
            .activate_draft(&proposal, approval("stale-approval"))
            .await
            .err()
            .ok_or_else(|| merge_plan_error("stale activation unexpectedly succeeded"))?;
        assert!(is_stale_memory_proposal_error(&error));
        let (_, current) = manager
            .locate("stale-activation")
            .await?
            .ok_or_else(|| merge_plan_error("newer Draft missing"))?;
        assert_eq!(current.content, newer);
        assert_eq!(current.meta.status, MemoryStatus::Draft);
        Ok(())
    }

    #[tokio::test]
    async fn activation_fences_aba_even_when_draft_bytes_match_again() -> Result<()> {
        let manager = make_manager();
        let old = "The project uses Rust";
        let other = "The project uses Go";
        manager
            .write_memory("aba-activation", old, user_draft_meta(old))
            .await?;
        let proposal = manager
            .preview_activation("aba-activation")
            .await?
            .ok_or_else(|| merge_plan_error("activation proposal missing"))?;
        manager
            .write_memory("aba-activation", other, user_draft_meta(other))
            .await?;
        manager
            .write_memory("aba-activation", old, user_draft_meta(old))
            .await?;
        let error = manager
            .activate_draft(&proposal, approval("aba-approval"))
            .await
            .err()
            .ok_or_else(|| merge_plan_error("ABA activation unexpectedly succeeded"))?;
        assert!(is_stale_memory_proposal_error(&error));
        let refreshed = manager
            .preview_activation("aba-activation")
            .await?
            .ok_or_else(|| merge_plan_error("refreshed proposal missing"))?;
        assert!(refreshed.generation > proposal.generation);
        Ok(())
    }

    #[tokio::test]
    async fn direct_write_cannot_activate_or_replace_identical_approved_memory() -> Result<()> {
        let manager = make_manager();
        let content = "Use Rust for durable services";
        let mut forged = user_draft_meta(content);
        forged.status = MemoryStatus::Active;
        forged.provenance.approval = Some(approval("forged"));
        manager.write_memory("writer", content, forged).await?;
        let (_, draft) = manager
            .locate("writer")
            .await?
            .ok_or_else(|| merge_plan_error("Draft missing"))?;
        assert_eq!(draft.meta.status, MemoryStatus::Draft);
        assert!(draft.meta.provenance.approval.is_none());

        let proposal = manager
            .preview_activation("writer")
            .await?
            .ok_or_else(|| merge_plan_error("activation proposal missing"))?;
        manager
            .activate_draft(&proposal, approval("approved"))
            .await?;
        manager
            .write_memory("writer", content, user_draft_meta(content))
            .await?;
        let (_, active) = manager
            .locate("writer")
            .await?
            .ok_or_else(|| merge_plan_error("approved memory missing"))?;
        assert_eq!(active.meta.status, MemoryStatus::Active);
        assert_eq!(
            active
                .meta
                .provenance
                .approval
                .as_ref()
                .map(|item| item.approval_id.as_str()),
            Some("approved")
        );
        assert!(matches!(
            manager
                .activate_draft(&proposal, approval("approved"))
                .await?,
            MemoryActivationOutcome::AlreadyActivated(_)
        ));
        Ok(())
    }

    #[tokio::test]
    async fn provenance_evidence_with_a_secret_is_rejected_before_persistence() -> Result<()> {
        let manager = make_manager();
        let mut meta = user_draft_meta("ordinary fact");
        meta.provenance.evidence = vec![echo_core::memory::MemoryEvidence::new(
            echo_core::memory::MemoryEvidenceRole::User,
            format!("ghp_{}", "A".repeat(36)),
        )];
        assert!(
            manager
                .write_memory("secret-evidence", "ordinary fact", meta)
                .await
                .is_err()
        );
        assert!(manager.locate("secret-evidence").await?.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn replacing_an_approved_fact_starts_a_new_recall_lifetime() -> Result<()> {
        let manager = make_manager();
        let old = "Use Rust for the old service";
        let new = "Use Go for the new service";
        write_approved_memory(&manager, "replaced", old, user_draft_meta(old)).await?;
        let (_, current) = manager
            .locate("replaced")
            .await?
            .ok_or_else(|| merge_plan_error("approved memory missing"))?;
        let mut counted = current.meta;
        counted.recall_count = 7;
        counted.last_recalled_at = Some(1_750_000_000);
        manager
            .typed_store
            .update_meta(WARM_NAMESPACE, "replaced", counted)
            .await?;

        manager
            .write_memory("replaced", new, user_draft_meta(new))
            .await?;
        let (_, draft) = manager
            .locate("replaced")
            .await?
            .ok_or_else(|| merge_plan_error("new Draft missing"))?;
        assert_eq!(draft.meta.status, MemoryStatus::Draft);
        assert_eq!(draft.meta.recall_count, 0);
        assert_eq!(draft.meta.last_recalled_at, None);
        Ok(())
    }

    #[tokio::test]
    async fn failed_activation_reconciles_after_file_store_restart() -> Result<()> {
        let dir = tempfile::tempdir().map_err(ReactError::from)?;
        let root = dir.path().join(".echo-agent");
        let store_path = dir.path().join("store.json");
        let audit_path = root.join("evolution/change-log.jsonl");
        let content = "The durable service uses a single memory authority";
        let proposal = {
            let manager = MemoryLayerManager::new(
                root.clone(),
                Arc::new(FileStore::new(&store_path)?),
                Box::new(JsonlChangeLog::new(audit_path.clone())?),
            );
            manager
                .write_memory("restart-activation", content, user_draft_meta(content))
                .await?;
            manager
                .preview_activation("restart-activation")
                .await?
                .ok_or_else(|| merge_plan_error("activation proposal missing"))?
        };

        {
            let manager = MemoryLayerManager::new(
                root.clone(),
                Arc::new(FileStore::new(&store_path)?),
                Box::new(FailOnceChangeLog {
                    inner: JsonlChangeLog::new(audit_path.clone())?,
                    fail_next: AtomicBool::new(true),
                }),
            );
            assert!(
                manager
                    .activate_draft(&proposal, approval("restart-approval"))
                    .await
                    .is_err()
            );
        }

        let reopened = MemoryLayerManager::new(
            root,
            Arc::new(FileStore::new(store_path)?),
            Box::new(JsonlChangeLog::new(audit_path)?),
        );
        reopened.reconcile_pending().await?;
        let (_, current) = reopened
            .locate("restart-activation")
            .await?
            .ok_or_else(|| merge_plan_error("reconciled activation missing"))?;
        assert_eq!(current.meta.status, MemoryStatus::Active);
        assert_eq!(
            current
                .meta
                .provenance
                .approval
                .as_ref()
                .map(|receipt| receipt.approval_id.as_str()),
            Some("restart-approval")
        );
        assert!(matches!(
            reopened
                .activate_draft(&proposal, approval("restart-approval"))
                .await?,
            MemoryActivationOutcome::AlreadyActivated(_)
        ));
        Ok(())
    }

    #[tokio::test]
    async fn approved_hot_file_retains_provenance_after_restart() -> Result<()> {
        let dir = tempfile::tempdir().map_err(ReactError::from)?;
        let root = dir.path().join(".echo-agent");
        let store_path = dir.path().join("store.json");
        let content = "User prefers Rust for durable services";
        {
            let manager = MemoryLayerManager::new(
                root.clone(),
                Arc::new(FileStore::new(&store_path)?),
                Box::new(NullChangeLog),
            );
            let meta = MemoryMeta::new(
                MemoryType::UserPreference,
                MemorySource::AutoExtracted,
                "preferences",
            )
            .with_confidence(0.95)
            .with_stability(0.90);
            write_approved_memory(&manager, "hot-provenance", content, meta).await?;
            assert!(manager.read_hot_content()?.contains(content));
        }

        let reopened = MemoryLayerManager::new(
            root,
            Arc::new(FileStore::new(store_path)?),
            Box::new(NullChangeLog),
        );
        reopened.reconcile_pending().await?;
        let hot = reopened.list_hot()?;
        let record = hot
            .iter()
            .find(|entry| entry.key == "hot-provenance")
            .ok_or_else(|| merge_plan_error("approved hot record missing after restart"))?;
        assert_eq!(record.content, content);
        assert_eq!(record.meta.status, MemoryStatus::Active);
        assert_eq!(record.meta.provenance.trust, MemoryTrust::User);
        assert_eq!(
            record
                .meta
                .provenance
                .evidence
                .first()
                .map(|item| item.quote.as_str()),
            Some(content)
        );
        assert!(record.meta.provenance.is_approved());
        assert!(reopened.read_hot_content()?.contains(content));
        Ok(())
    }

    #[tokio::test]
    async fn historical_hot_record_is_inspectable_but_not_prompt_visible() -> Result<()> {
        let dir = tempfile::tempdir().map_err(ReactError::from)?;
        let manager = MemoryLayerManager::new(
            dir.path().to_path_buf(),
            Arc::new(InMemoryStore::new()),
            Box::new(NullChangeLog),
        );
        let legacy = TypedMemoryEntry {
            key: "legacy-hot".to_string(),
            content: "Old unverified memory".to_string(),
            meta: MemoryMeta::new(
                MemoryType::ProjectFact,
                MemorySource::AutoExtracted,
                "legacy",
            ),
            raw: echo_core::memory::store::StoreItem::new(
                vec!["agent".to_string(), "memories".to_string()],
                "legacy-hot".to_string(),
                serde_json::Value::Null,
            ),
        };
        manager
            .add_to_hot(&legacy)
            .await
            .map_err(ReactError::from)?;
        assert_eq!(manager.list_hot()?.len(), 1);
        assert!(manager.read_hot_content()?.is_empty());
        assert!(manager.search_layered("legacy", 5).await?.is_empty());
        Ok(())
    }

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn sqlite_draft_activation_survives_store_restart() -> Result<()> {
        let dir = tempfile::tempdir().map_err(ReactError::from)?;
        let root = dir.path().join(".echo-agent");
        let sqlite_path = dir.path().join("memory.sqlite");
        let content = "User prefers Rust for persistent services";
        let proposal = {
            let manager = MemoryLayerManager::new(
                root.clone(),
                Arc::new(echo_state::memory::SqliteStore::new(&sqlite_path)?),
                Box::new(NullChangeLog),
            );
            manager
                .write_memory("sqlite-draft", content, user_draft_meta(content))
                .await?;
            manager
                .preview_activation("sqlite-draft")
                .await?
                .ok_or_else(|| merge_plan_error("SQLite Draft proposal missing"))?
        };
        {
            let reopened = MemoryLayerManager::new(
                root.clone(),
                Arc::new(echo_state::memory::SqliteStore::new(&sqlite_path)?),
                Box::new(NullChangeLog),
            );
            reopened.reconcile_pending().await?;
            assert!(matches!(
                reopened
                    .activate_draft(&proposal, approval("sqlite-approval"))
                    .await?,
                MemoryActivationOutcome::Activated(_)
            ));
        }
        let reopened = MemoryLayerManager::new(
            root,
            Arc::new(echo_state::memory::SqliteStore::new(sqlite_path)?),
            Box::new(NullChangeLog),
        );
        reopened.reconcile_pending().await?;
        let (_, current) = reopened
            .locate("sqlite-draft")
            .await?
            .ok_or_else(|| merge_plan_error("activated SQLite memory missing"))?;
        assert!(current.meta.is_recallable());
        assert!(matches!(
            reopened
                .activate_draft(&proposal, approval("sqlite-approval"))
                .await?,
            MemoryActivationOutcome::AlreadyActivated(_)
        ));
        Ok(())
    }

    #[tokio::test]
    async fn cancelled_activation_reconciles_without_losing_approval() -> Result<()> {
        let root = tempfile::tempdir().map_err(ReactError::from)?;
        let store = Arc::new(BlockingStore::new());
        let manager = Arc::new(MemoryLayerManager::new(
            root.path().to_path_buf(),
            store.clone(),
            Box::new(NullChangeLog),
        ));
        let content = "The approved memory survives caller cancellation";
        manager
            .write_memory("cancel-activation", content, user_draft_meta(content))
            .await?;
        let proposal = manager
            .preview_activation("cancel-activation")
            .await?
            .ok_or_else(|| merge_plan_error("activation proposal missing"))?;
        store.block_next_put();

        let task = tokio::spawn({
            let manager = manager.clone();
            let proposal = proposal.clone();
            async move {
                manager
                    .activate_draft(&proposal, approval("cancel-approval"))
                    .await
            }
        });
        store.put_started.notified().await;
        task.abort();
        let _ = task.await;
        store.release_put.notify_one();

        manager.reconcile_pending().await?;
        let (_, current) = manager
            .locate("cancel-activation")
            .await?
            .ok_or_else(|| merge_plan_error("reconciled activation missing"))?;
        assert_eq!(current.meta.status, MemoryStatus::Active);
        assert_eq!(
            current
                .meta
                .provenance
                .approval
                .as_ref()
                .map(|receipt| receipt.approval_id.as_str()),
            Some("cancel-approval")
        );
        Ok(())
    }

    fn latest_memory_audit(path: &std::path::Path, key: &str) -> Result<ChangeEntry> {
        JsonlChangeLog::new(path.to_path_buf())?
            .latest_for(EntityType::Memory, key)?
            .ok_or_else(|| merge_plan_error(format!("expected audit for memory key {key}")))
    }

    fn audit_projection_layer(entry: &ChangeEntry, before: bool) -> Option<&str> {
        let projection = if before {
            entry.before.as_ref()
        } else {
            entry.after.as_ref()
        };
        projection
            .and_then(|value| value.get("layer"))
            .and_then(serde_json::Value::as_str)
    }

    fn assert_machine_readable_rollback_summary(entry: &ChangeEntry) -> Result<()> {
        let before = entry.before.as_ref().and_then(serde_json::Value::as_object);
        let after = entry.after.as_ref().and_then(serde_json::Value::as_object);
        for summary in [before, after] {
            let summary = summary.ok_or_else(|| {
                merge_plan_error("rollback audit must contain an object projection summary")
            })?;
            for field in [
                "layer",
                "warm",
                "hot",
                "rollback_of",
                "target_batch_id",
                "target_generation",
            ] {
                assert!(
                    summary.contains_key(field),
                    "missing rollback field {field}"
                );
            }
        }
        assert_eq!(
            before.and_then(|summary| summary.get("rollback_of")),
            after.and_then(|summary| summary.get("rollback_of"))
        );
        assert_eq!(
            before.and_then(|summary| summary.get("target_batch_id")),
            after.and_then(|summary| summary.get("target_batch_id"))
        );
        assert_eq!(
            before.and_then(|summary| summary.get("target_generation")),
            after.and_then(|summary| summary.get("target_generation"))
        );
        Ok(())
    }

    #[test]
    fn test_parse_memory_file_with_frontmatter() {
        let raw = "\
---
entries:
  - key: build_java8
    memory_type: debugging_lesson
    confidence: 0.90
    stability: 0.80
    source: error_resolution
    topic: build
    risk: low
    last_promoted: \"2026-06-15T10:30:00Z\"
---

- **[build_java8]** Maven compile requires JAVA_HOME pointing to JDK 8.
- **[style/concise]** User prefers concise code comments.
";
        let file = parse_memory_md(raw);
        assert_eq!(file.entries.len(), 1);
        assert_eq!(file.entries[0].key, "build_java8");
        assert_eq!(file.entries[0].memory_type, MemoryType::DebuggingLesson);
        assert!(file.body.contains("**[build_java8]**"));
        assert!(file.body.contains("**[style/concise]**"));
    }

    #[test]
    fn test_parse_memory_file_without_frontmatter() {
        let raw = "- Simple memory without frontmatter\n";
        let file = parse_memory_md(raw);
        assert!(file.entries.is_empty());
        assert!(file.body.contains("Simple memory without frontmatter"));
    }

    #[test]
    fn test_parse_memory_file_empty() {
        let file = parse_memory_md("");
        assert!(file.entries.is_empty());
        assert!(file.body.is_empty());
    }

    #[test]
    fn test_format_memory_md_roundtrip() {
        let file = MemoryFile {
            entries: vec![HotEntryMeta {
                key: "test_key".to_string(),
                memory_type: MemoryType::UserPreference,
                confidence: 0.95,
                stability: 0.85,
                recall_weight: 0.8,
                source: MemorySource::ExplicitSave,
                provenance: MemoryProvenance::default(),
                topic: "style".to_string(),
                risk: MemoryRisk::Low,
                revision_count: 2,
                recall_count: 4,
                last_recalled_at: Some(1_750_000_000),
                content_json: false,
                last_promoted: "2026-06-15T10:00:00Z".to_string(),
            }],
            body: "- **[test_key]** User prefers concise output.\n".to_string(),
        };

        let formatted = format_memory_md(&file);
        let reparsed = parse_memory_md(&formatted);
        assert_eq!(reparsed.entries.len(), 1);
        assert_eq!(reparsed.entries[0].key, "test_key");
        assert!(reparsed.body.contains("**[test_key]**"));
    }

    #[test]
    fn test_read_hot_content_empty() -> Result<()> {
        let manager = make_manager();
        assert!(manager.read_hot_content()?.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_add_to_hot_under_budget() -> Result<()> {
        let manager = make_manager();
        let entry = TypedMemoryEntry {
            key: "test_key".to_string(),
            content: "User prefers concise output.".to_string(),
            meta: approved_test_meta(
                MemoryMeta::new(
                    MemoryType::UserPreference,
                    MemorySource::ExplicitSave,
                    "style",
                ),
                "User prefers concise output.",
            ),
            raw: echo_core::memory::store::StoreItem::new(
                vec!["agent".to_string(), "typed_memories".to_string()],
                "test_key".to_string(),
                serde_json::Value::Null,
            ),
        };

        manager.add_to_hot(&entry).await.expect("add to hot");

        let content = manager.read_hot_content()?;
        assert!(content.contains("**[test_key]**"));
        assert!(content.contains("User prefers concise output"));

        let meta = manager.read_hot_meta()?;
        assert_eq!(meta.len(), 1);
        assert_eq!(meta[0].key, "test_key");
        Ok(())
    }

    #[tokio::test]
    async fn successful_hot_delete_notifies_once_and_missing_retry_does_not_notify() -> Result<()> {
        let dir = tempfile::tempdir()
            .map_err(|error| merge_plan_error(format!("tempdir failed: {error}")))?
            .keep();
        let observer = Arc::new(RecordingObserver::default());
        let manager =
            MemoryLayerManager::new(dir, Arc::new(InMemoryStore::new()), Box::new(NullChangeLog))
                .with_evolution_observer(observer.clone());
        let entry = TypedMemoryEntry {
            key: "delete_hot".to_string(),
            content: "Delete this hot memory".to_string(),
            meta: MemoryMeta::new(
                MemoryType::UserPreference,
                MemorySource::ExplicitSave,
                "test",
            ),
            raw: echo_core::memory::store::StoreItem::new(
                vec!["agent".to_string()],
                "delete_hot".to_string(),
                serde_json::Value::Null,
            ),
        };
        manager.add_to_hot(&entry).await.map_err(ReactError::from)?;

        assert!(manager.delete_memory("delete_hot").await?);
        assert!(!manager.delete_memory("delete_hot").await?);
        assert_eq!(
            observer.changes(),
            vec![(
                "delete_hot".to_string(),
                "hot".to_string(),
                "deleted".to_string(),
            )]
        );
        Ok(())
    }

    #[tokio::test]
    async fn failed_hot_delete_audit_emits_no_layer_change() -> Result<()> {
        let dir = tempfile::tempdir()
            .map_err(|error| merge_plan_error(format!("tempdir failed: {error}")))?
            .keep();
        let observer = Arc::new(RecordingObserver::default());
        let manager = MemoryLayerManager::new(
            dir,
            Arc::new(InMemoryStore::new()),
            Box::new(FailingChangeLog),
        )
        .with_evolution_observer(observer.clone());
        let entry = TypedMemoryEntry {
            key: "failed_delete".to_string(),
            content: "Audit failure".to_string(),
            meta: MemoryMeta::new(MemoryType::ProjectFact, MemorySource::ExplicitSave, "test"),
            raw: echo_core::memory::store::StoreItem::new(
                vec!["agent".to_string()],
                "failed_delete".to_string(),
                serde_json::Value::Null,
            ),
        };
        manager.add_to_hot(&entry).await.map_err(ReactError::from)?;

        assert!(manager.delete_memory("failed_delete").await.is_err());
        assert!(observer.changes().is_empty());
        assert!(manager.read_hot_content().is_err());
        assert!(manager.list_hot().is_err());
        assert!(manager.locate("failed_delete").await.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn two_managers_share_one_root_operation_order() -> Result<()> {
        let dir = tempfile::tempdir().map_err(ReactError::from)?;
        let root = dir.path().join(".echo-agent");
        let audit_path = root.join("evolution/change-log.jsonl");
        let store = Arc::new(FileStore::new(dir.path().join("store.json"))?);
        let first = MemoryLayerManager::new(
            root.clone(),
            store.clone(),
            Box::new(JsonlChangeLog::new(audit_path.clone())?),
        );
        let second = MemoryLayerManager::new(
            root,
            store.clone(),
            Box::new(JsonlChangeLog::new(audit_path.clone())?),
        );
        let meta = MemoryMeta::new(
            MemoryType::ProjectFact,
            MemorySource::AutoExtracted,
            "build",
        );
        let (a, b) = tokio::join!(
            first.write_memory("shared", "A", meta.clone()),
            second.write_memory("shared", "B", meta),
        );
        assert!(a?.is_none());
        assert!(b?.is_none());
        let audit =
            JsonlChangeLog::new(audit_path)?.query(&super::super::audit::ChangeFilter::new())?;
        assert_eq!(audit.len(), 2);
        assert_eq!(
            audit
                .iter()
                .filter(|entry| entry.change_type == ChangeType::Create)
                .count(),
            1
        );
        assert_eq!(
            audit
                .iter()
                .filter(|entry| entry.change_type == ChangeType::Update)
                .count(),
            1
        );
        assert_eq!(store.list(WARM_NAMESPACE).await?.len(), 1);
        Ok(())
    }

    #[test]
    fn journal_lease_child_probe() -> Result<()> {
        let Some(root) = std::env::var_os("ECHO_TEST_MEMORY_JOURNAL_ROOT") else {
            return Ok(());
        };
        let manager = MemoryLayerManager::try_new(
            PathBuf::from(root),
            Arc::new(InMemoryStore::new()),
            Box::new(NullChangeLog),
        );
        if manager.is_ok() {
            return Err(merge_plan_error(
                "competing process opened the live memory operation journal",
            ));
        }
        Ok(())
    }

    #[test]
    fn second_process_fails_closed_while_manager_owns_journal() -> Result<()> {
        let dir = tempfile::tempdir().map_err(ReactError::from)?;
        let root = dir.path().join(".echo-agent");
        let _manager = MemoryLayerManager::try_new(
            root.clone(),
            Arc::new(InMemoryStore::new()),
            Box::new(NullChangeLog),
        )?;
        let executable = std::env::current_exe().map_err(ReactError::from)?;
        let child = std::process::Command::new(executable)
            .arg("--exact")
            .arg("evolution::layer::tests::journal_lease_child_probe")
            .env("ECHO_TEST_MEMORY_JOURNAL_ROOT", root)
            .output()
            .map_err(ReactError::from)?;
        if !child.status.success() {
            return Err(ReactError::Other(format!(
                "competing memory journal process probe failed: {}",
                String::from_utf8_lossy(&child.stdout)
            )));
        }
        Ok(())
    }

    #[tokio::test]
    async fn durable_write_reconciles_one_audit_after_restart() -> Result<()> {
        let dir = tempfile::tempdir().map_err(ReactError::from)?;
        let root = dir.path().join(".echo-agent");
        let store_path = dir.path().join("store.json");
        let audit_path = root.join("evolution/change-log.jsonl");
        let store = Arc::new(FileStore::new(&store_path)?);
        let log = FailOnceChangeLog {
            inner: JsonlChangeLog::new(audit_path.clone())?,
            fail_next: AtomicBool::new(true),
        };
        let manager = MemoryLayerManager::new(root.clone(), store.clone(), Box::new(log));
        let meta = MemoryMeta::new(
            MemoryType::ProjectFact,
            MemorySource::AutoExtracted,
            "build",
        );

        assert!(
            manager
                .write_memory("durable", "Use cargo", meta)
                .await
                .is_err()
        );
        assert!(store.get(WARM_NAMESPACE, "durable").await?.is_some());
        assert_eq!(JsonlChangeLog::new(audit_path.clone())?.len(), 0);
        drop(manager);
        drop(store);

        let reopened = MemoryLayerManager::new(
            root,
            Arc::new(FileStore::new(store_path)?),
            Box::new(JsonlChangeLog::new(audit_path.clone())?),
        );
        reopened.reconcile_pending().await?;
        reopened.reconcile_pending().await?;
        let entries =
            JsonlChangeLog::new(audit_path)?.query(&super::super::audit::ChangeFilter::new())?;
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries.first().map(|entry| entry.entity_key.as_str()),
            Some("durable")
        );
        Ok(())
    }

    #[tokio::test]
    async fn settled_history_repairs_older_store_projection_without_duplicate_audit() -> Result<()>
    {
        let dir = tempfile::tempdir().map_err(ReactError::from)?;
        let root = dir.path().join(".echo-agent");
        let store_path = dir.path().join("store.json");
        let audit_path = root.join("evolution/change-log.jsonl");
        let store = Arc::new(FileStore::new(&store_path)?);
        let manager = MemoryLayerManager::new(
            root.clone(),
            store.clone(),
            Box::new(JsonlChangeLog::new(audit_path.clone())?),
        );
        let meta = MemoryMeta::new(
            MemoryType::ProjectFact,
            MemorySource::AutoExtracted,
            "build",
        );
        manager
            .write_memory("settled", "Old value", meta.clone())
            .await?;
        let old = store
            .get(WARM_NAMESPACE, "settled")
            .await?
            .ok_or_else(|| merge_plan_error("missing old projection"))?
            .value;
        manager.write_memory("settled", "New value", meta).await?;
        drop(manager);
        store.put(WARM_NAMESPACE, "settled", old).await?;
        drop(store);

        let store = Arc::new(FileStore::new(&store_path)?);
        let reopened = MemoryLayerManager::new(
            root,
            store.clone(),
            Box::new(JsonlChangeLog::new(audit_path.clone())?),
        );
        assert!(reopened.read_hot_content().is_err());
        reopened.reconcile_pending().await?;
        reopened.reconcile_pending().await?;
        let restored = reopened
            .typed_store
            .get_typed(WARM_NAMESPACE, "settled")
            .await?
            .ok_or_else(|| merge_plan_error("missing repaired projection"))?;
        assert_eq!(restored.content, "New value");
        assert_eq!(JsonlChangeLog::new(audit_path)?.len(), 2);
        store
            .put(
                WARM_NAMESPACE,
                "settled",
                serde_json::json!({ "external": "different" }),
            )
            .await?;
        assert!(reopened.reconcile_pending().await.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn failed_prepare_does_not_mutate_warm_store() -> Result<()> {
        let dir = tempfile::tempdir().map_err(ReactError::from)?;
        let root = dir.path().join(".echo-agent");
        std::fs::create_dir_all(root.join("evolution/memory-operations.jsonl"))
            .map_err(ReactError::from)?;
        let store = Arc::new(InMemoryStore::new());
        let manager = MemoryLayerManager::new(root, store.clone(), Box::new(NullChangeLog));
        let meta = MemoryMeta::new(
            MemoryType::ProjectFact,
            MemorySource::AutoExtracted,
            "build",
        );
        assert!(
            manager
                .write_memory("no_prepare", "No mutation", meta)
                .await
                .is_err()
        );
        assert!(store.get(WARM_NAMESPACE, "no_prepare").await?.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn restart_reconciles_mid_promotion_and_demotion_once() -> Result<()> {
        for promote in [true, false] {
            let dir = tempfile::tempdir().map_err(ReactError::from)?;
            let root = dir.path().join(".echo-agent");
            let store_path = dir.path().join("store.json");
            let audit_path = root.join("evolution/change-log.jsonl");
            let store = Arc::new(FileStore::new(&store_path)?);
            let manager = MemoryLayerManager::new(
                root.clone(),
                store.clone(),
                Box::new(JsonlChangeLog::new(audit_path.clone())?),
            );
            let meta = MemoryMeta::new(
                MemoryType::UserPreference,
                MemorySource::ExplicitSave,
                "style",
            )
            .with_confidence(0.95)
            .with_stability(0.90);
            let entry = TypedMemoryEntry {
                key: "moving".into(),
                content: "Keep this memory".into(),
                meta: meta.clone(),
                raw: echo_core::memory::store::StoreItem::new(
                    WARM_NAMESPACE
                        .iter()
                        .map(|part| (*part).to_owned())
                        .collect(),
                    "moving".into(),
                    serde_json::Value::Null,
                ),
            };
            if promote {
                manager
                    .typed_store
                    .put_typed(WARM_NAMESPACE, "moving", &entry.content, meta.clone())
                    .await?;
            } else {
                manager.add_to_hot(&entry).await.map_err(ReactError::from)?;
            }
            let hot = HotValue {
                meta: MemoryLayerManager::hot_meta(&entry),
                content: entry.content.clone(),
            };
            let id = uuid::Uuid::new_v4().to_string();
            let operation = MemoryOperation {
                id: id.clone(),
                key: "moving".into(),
                warm_before: store
                    .get(WARM_NAMESPACE, "moving")
                    .await?
                    .map(|item| item.value),
                warm_after: if promote {
                    None
                } else {
                    Some(MemoryLayerManager::typed_value(&entry.content, meta)?)
                },
                hot_before: MemoryLayerManager::hot_value(&manager.parse_memory_file(), "moving"),
                hot_after: if promote { Some(hot.clone()) } else { None },
                audit: MemoryLayerManager::change_builder(
                    "moving",
                    if promote {
                        ChangeType::Promote
                    } else {
                        ChangeType::Demote
                    },
                    Some(if promote { "warm" } else { "hot" }),
                    Some(if promote { "hot" } else { "warm" }),
                    "crash window",
                    "test",
                )
                .build_with(id, chrono::Utc::now()),
            };
            manager.operation_journal()?.prepare(operation)?;
            if promote {
                let mut guard = manager.lock_hot_file().await.map_err(ReactError::from)?;
                MemoryLayerManager::set_hot_value(&mut guard.file, "moving", Some(&hot))?;
                guard.commit().map_err(ReactError::from)?;
            } else {
                manager
                    .typed_store
                    .put_typed(WARM_NAMESPACE, "moving", &entry.content, entry.meta)
                    .await?;
            }
            drop(manager);
            drop(store);

            let reopened = MemoryLayerManager::new(
                root,
                Arc::new(FileStore::new(store_path)?),
                Box::new(JsonlChangeLog::new(audit_path.clone())?),
            );
            reopened.reconcile_pending().await?;
            reopened.reconcile_pending().await?;
            let actual = reopened.locate("moving").await?.map(|(layer, _)| layer);
            assert_eq!(
                actual,
                Some(if promote {
                    MemoryLayer::Hot
                } else {
                    MemoryLayer::Warm
                })
            );
            assert_eq!(JsonlChangeLog::new(audit_path)?.len(), 1);
        }
        Ok(())
    }

    #[tokio::test]
    async fn restart_reconciles_mid_warm_delete_and_meta_update() -> Result<()> {
        for delete in [true, false] {
            let dir = tempfile::tempdir().map_err(ReactError::from)?;
            let root = dir.path().join(".echo-agent");
            let store_path = dir.path().join("store.json");
            let audit_path = root.join("evolution/change-log.jsonl");
            let store = Arc::new(FileStore::new(&store_path)?);
            let manager = MemoryLayerManager::new(
                root.clone(),
                store.clone(),
                Box::new(JsonlChangeLog::new(audit_path.clone())?),
            );
            let meta = MemoryMeta::new(
                MemoryType::ProjectFact,
                MemorySource::AutoExtracted,
                "build",
            );
            manager
                .typed_store
                .put_typed(WARM_NAMESPACE, "warm", "Original", meta.clone())
                .await?;
            let mut after_meta = meta;
            after_meta.status = MemoryStatus::Archived;
            let after = if delete {
                None
            } else {
                Some(MemoryLayerManager::typed_value("Original", after_meta)?)
            };
            let id = uuid::Uuid::new_v4().to_string();
            manager.operation_journal()?.prepare(MemoryOperation {
                id: id.clone(),
                key: "warm".into(),
                warm_before: store
                    .get(WARM_NAMESPACE, "warm")
                    .await?
                    .map(|item| item.value),
                warm_after: after.clone(),
                hot_before: None,
                hot_after: None,
                audit: MemoryLayerManager::change_builder(
                    "warm",
                    if delete {
                        ChangeType::Delete
                    } else {
                        ChangeType::Demote
                    },
                    Some("warm"),
                    if delete { None } else { Some("archived") },
                    "crash window",
                    "test",
                )
                .build_with(id, chrono::Utc::now()),
            })?;
            match after {
                Some(value) => store.put(WARM_NAMESPACE, "warm", value).await?,
                None => {
                    store.delete(WARM_NAMESPACE, "warm").await?;
                }
            }
            drop(manager);
            drop(store);

            let reopened = MemoryLayerManager::new(
                root,
                Arc::new(FileStore::new(store_path)?),
                Box::new(JsonlChangeLog::new(audit_path.clone())?),
            );
            reopened.reconcile_pending().await?;
            reopened.reconcile_pending().await?;
            let entry = reopened
                .typed_store
                .get_typed(WARM_NAMESPACE, "warm")
                .await?;
            if delete {
                assert!(entry.is_none());
            } else {
                assert_eq!(
                    entry.map(|item| item.meta.status),
                    Some(MemoryStatus::Archived)
                );
            }
            assert_eq!(JsonlChangeLog::new(audit_path)?.len(), 1);
        }
        Ok(())
    }

    #[tokio::test]
    async fn merge_group_replays_all_member_audits_after_second_failure() -> Result<()> {
        let dir = tempfile::tempdir().map_err(ReactError::from)?;
        let root = dir.path().join(".echo-agent");
        let store_path = dir.path().join("store.json");
        let audit_path = root.join("evolution/change-log.jsonl");
        let store = Arc::new(FileStore::new(&store_path)?);
        let manager = MemoryLayerManager::new(
            root.clone(),
            store.clone(),
            Box::new(FailSecondChangeLog {
                inner: JsonlChangeLog::new(audit_path.clone())?,
                calls: AtomicUsize::new(0),
            }),
        );
        let primary = MemoryMeta::new(MemoryType::ProjectFact, MemorySource::ExplicitSave, "build")
            .with_confidence(0.95);
        let secondary = MemoryMeta::new(
            MemoryType::ProjectFact,
            MemorySource::AutoExtracted,
            "build",
        )
        .with_confidence(0.55);
        manager
            .typed_store
            .put_typed(WARM_NAMESPACE, "primary", "Use cargo", primary)
            .await?;
        manager
            .typed_store
            .put_typed(WARM_NAMESPACE, "secondary", "Use make", secondary)
            .await?;
        let entries = manager.list_warm_memories(&MemoryFilter::new()).await?;
        let group = ConflictDetector::new()
            .detect(&entries)
            .into_iter()
            .next()
            .ok_or_else(|| merge_plan_error("expected conflict group"))?;
        assert!(
            MemoryMerger::new(&manager)
                .merge_group(&group)
                .await
                .is_err()
        );
        assert_eq!(JsonlChangeLog::new(audit_path.clone())?.len(), 1);
        assert!(manager.read_hot_content().is_err());
        drop(manager);
        drop(store);

        let reopened = MemoryLayerManager::new(
            root,
            Arc::new(FileStore::new(store_path)?),
            Box::new(JsonlChangeLog::new(audit_path.clone())?),
        );
        reopened.reconcile_pending().await?;
        reopened.reconcile_pending().await?;
        let state = reopened.list_warm_memories(&MemoryFilter::new()).await?;
        assert_eq!(
            state
                .iter()
                .find(|entry| entry.key == "secondary")
                .map(|entry| entry.meta.status),
            Some(MemoryStatus::Superseded)
        );
        let audit =
            JsonlChangeLog::new(audit_path)?.query(&super::super::audit::ChangeFilter::new())?;
        assert_eq!(audit.len(), 2);
        assert_eq!(
            audit
                .iter()
                .filter(|entry| entry.change_type == ChangeType::Merge)
                .count(),
            2
        );
        Ok(())
    }

    #[tokio::test]
    async fn prepared_merge_replays_remaining_member_after_interrupted_projection() -> Result<()> {
        let dir = tempfile::tempdir().map_err(ReactError::from)?;
        let root = dir.path().join(".echo-agent");
        let store_path = dir.path().join("store.json");
        let audit_path = root.join("evolution/change-log.jsonl");
        let store = Arc::new(FileStore::new(&store_path)?);
        let manager = MemoryLayerManager::new(
            root.clone(),
            store.clone(),
            Box::new(JsonlChangeLog::new(audit_path.clone())?),
        );
        let mut operations = Vec::new();
        for key in ["first", "second"] {
            let before = MemoryLayerManager::typed_value(
                "Original",
                MemoryMeta::new(
                    MemoryType::ProjectFact,
                    MemorySource::AutoExtracted,
                    "merge",
                ),
            )?;
            store.put(WARM_NAMESPACE, key, before.clone()).await?;
            let mut meta = MemoryMeta::new(
                MemoryType::ProjectFact,
                MemorySource::AutoExtracted,
                "merge",
            );
            meta.status = MemoryStatus::Superseded;
            let after = MemoryLayerManager::typed_value("Original", meta)?;
            let id = uuid::Uuid::new_v4().to_string();
            operations.push(MemoryOperation {
                id: id.clone(),
                key: key.into(),
                warm_before: Some(before),
                warm_after: Some(after),
                hot_before: None,
                hot_after: None,
                audit: MemoryLayerManager::change_builder(
                    key,
                    ChangeType::Merge,
                    Some("warm"),
                    Some("superseded"),
                    "approved merge",
                    "test",
                )
                .build_with(id, chrono::Utc::now()),
            });
        }
        let batch = MemoryOperationBatch {
            id: uuid::Uuid::new_v4().to_string(),
            operations,
            origin: None,
        };
        manager.operation_journal()?.prepare_batch(batch.clone())?;
        let first = batch
            .operations
            .first()
            .ok_or_else(|| merge_plan_error("missing first member"))?;
        manager.apply_operation(first).await?;
        assert_eq!(JsonlChangeLog::new(audit_path.clone())?.len(), 0);
        drop(manager);
        drop(store);

        let reopened = MemoryLayerManager::new(
            root,
            Arc::new(FileStore::new(store_path)?),
            Box::new(JsonlChangeLog::new(audit_path.clone())?),
        );
        reopened.reconcile_pending().await?;
        reopened.reconcile_pending().await?;
        for key in ["first", "second"] {
            assert_eq!(
                reopened
                    .typed_store
                    .get_typed(WARM_NAMESPACE, key)
                    .await?
                    .map(|entry| entry.meta.status),
                Some(MemoryStatus::Superseded)
            );
        }
        assert_eq!(JsonlChangeLog::new(audit_path)?.len(), 2);
        Ok(())
    }

    #[test]
    fn test_demotion_score_ordering() {
        let high_quality = HotEntryMeta {
            key: "hq".to_string(),
            memory_type: MemoryType::UserPreference,
            confidence: 0.95,
            stability: 0.90,
            recall_weight: 0.8,
            source: MemorySource::ExplicitSave,
            provenance: MemoryProvenance::default(),
            topic: "style".to_string(),
            risk: MemoryRisk::Low,
            revision_count: 0,
            recall_count: 0,
            last_recalled_at: None,
            content_json: false,
            last_promoted: "2026-06-15T10:00:00Z".to_string(),
        };

        let low_quality = HotEntryMeta {
            key: "lq".to_string(),
            memory_type: MemoryType::CommandPattern,
            confidence: 0.50,
            stability: 0.30,
            recall_weight: 0.4,
            source: MemorySource::AutoExtracted,
            provenance: MemoryProvenance::default(),
            topic: "build".to_string(),
            risk: MemoryRisk::Low,
            revision_count: 0,
            recall_count: 0,
            last_recalled_at: None,
            content_json: false,
            last_promoted: "2026-06-15T10:00:00Z".to_string(),
        };

        // Higher confidence/stability → lower demotion score
        assert!(
            MemoryLayerManager::demotion_score(&high_quality)
                < MemoryLayerManager::demotion_score(&low_quality)
        );
    }

    #[test]
    fn test_extract_hot_entry_content() {
        let body = "- **[build_java8]** Maven needs Java 8\n- **[style/concise]** Be brief\n";
        assert_eq!(
            extract_hot_entry_content(body, "build_java8"),
            "Maven needs Java 8"
        );
        assert_eq!(extract_hot_entry_content(body, "style/concise"), "Be brief");
        assert_eq!(extract_hot_entry_content(body, "nonexistent"), "");
    }

    #[test]
    fn test_estimate_tokens() {
        // Pure Latin
        let latin = "a".repeat(100);
        assert!(estimate_tokens(&latin) <= 30);

        // Mixed
        let mixed = format!("Hello 世界 {}", "x".repeat(50));
        let tokens = estimate_tokens(&mixed);
        assert!(tokens > 0);
    }

    #[tokio::test]
    async fn test_write_memory_and_consider_promotion() -> Result<()> {
        let manager = make_manager();

        // High confidence, high stability → should be eligible for hot
        let meta = MemoryMeta::new(
            MemoryType::UserPreference,
            MemorySource::ExplicitSave,
            "style",
        )
        .with_confidence(0.95)
        .with_stability(0.90)
        .with_provenance(MemoryProvenance::draft(
            MemoryTrust::User,
            vec![echo_core::memory::MemoryEvidence::new(
                echo_core::memory::MemoryEvidenceRole::User,
                "User prefers concise output",
            )],
        ));

        let result = manager
            .write_memory("test_pref", "User prefers concise output", meta.clone())
            .await?;
        assert!(result.is_none(), "new memory stays Draft until reviewed");
        let proposal = manager
            .preview_activation("test_pref")
            .await?
            .ok_or_else(|| merge_plan_error("Draft proposal missing"))?;
        manager
            .activate_draft(&proposal, approval("test-pref-approval"))
            .await?;
        let (layer, _) = manager
            .locate("test_pref")
            .await?
            .ok_or_else(|| merge_plan_error("approved memory missing"))?;
        assert_eq!(layer, MemoryLayer::Hot);

        // Verify it's in hot
        let content = manager.read_hot_content()?;
        assert!(content.contains("**[test_pref]**"));
        Ok(())
    }

    #[tokio::test]
    async fn promoted_whitespace_and_multiline_content_survives_reconcile() -> Result<()> {
        for content in [
            "  leading and trailing  ",
            "first line\nsecond line\n",
            "- **[formatted]** literal content",
        ] {
            let dir = tempfile::tempdir().map_err(ReactError::from)?;
            let root = dir.path().join(".echo-agent");
            let store_path = dir.path().join("store.json");
            let audit_path = root.join("evolution/change-log.jsonl");
            let store = Arc::new(FileStore::new(&store_path)?);
            let manager = MemoryLayerManager::try_new(
                root.clone(),
                store,
                Box::new(JsonlChangeLog::new(audit_path.clone())?),
            )?;
            let meta = MemoryMeta::new(
                MemoryType::UserPreference,
                MemorySource::ExplicitSave,
                "format",
            )
            .with_confidence(0.95)
            .with_stability(0.90);
            manager
                .write_memory("formatted", content, meta.clone())
                .await?;
            let (_, live) = manager
                .locate("formatted")
                .await?
                .ok_or_else(|| merge_plan_error("promoted memory missing"))?;
            assert_eq!(live.content, content);
            drop(manager);

            let reopened = MemoryLayerManager::try_new(
                root,
                Arc::new(FileStore::new(store_path)?),
                Box::new(JsonlChangeLog::new(audit_path)?),
            )?;
            reopened.reconcile_pending().await?;
            let (_, recovered) = reopened
                .locate("formatted")
                .await?
                .ok_or_else(|| merge_plan_error("recovered memory missing"))?;
            assert_eq!(recovered.content, content);
            reopened.demote("formatted", "format roundtrip").await?;
            let (_, warm) = reopened
                .locate("formatted")
                .await?
                .ok_or_else(|| merge_plan_error("demoted memory missing"))?;
            assert_eq!(warm.content, content);
            reopened
                .write_memory("subsequent", "another fact", meta)
                .await?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn stale_promotion_snapshot_cannot_overwrite_newer_warm_write() -> Result<()> {
        let dir = tempfile::tempdir().map_err(ReactError::from)?;
        let store = Arc::new(InMemoryStore::new());
        let first = MemoryLayerManager::new(
            dir.path().to_path_buf(),
            store.clone(),
            Box::new(NullChangeLog),
        );
        let second = MemoryLayerManager::new(
            dir.path().to_path_buf(),
            store.clone(),
            Box::new(NullChangeLog),
        );
        let eligible = MemoryMeta::new(
            MemoryType::UserPreference,
            MemorySource::ExplicitSave,
            "style",
        )
        .with_confidence(0.95)
        .with_stability(0.90);
        first
            .typed_store
            .put_typed(WARM_NAMESPACE, "shared", "Old fact", eligible)
            .await?;
        let (_, stale) = first
            .locate("shared")
            .await?
            .ok_or_else(|| merge_plan_error("missing initial memory"))?;
        let newer = MemoryMeta::new(
            MemoryType::ProjectFact,
            MemorySource::AutoExtracted,
            "style",
        );
        second.write_memory("shared", "New fact", newer).await?;

        assert!(first.promote_warm_to_hot("shared", stale).await.is_err());
        let (layer, current) = first
            .locate("shared")
            .await?
            .ok_or_else(|| merge_plan_error("newer memory disappeared"))?;
        assert_eq!(layer, MemoryLayer::Warm);
        assert_eq!(current.content, "New fact");
        Ok(())
    }

    #[tokio::test]
    async fn stale_demotion_snapshot_cannot_overwrite_newer_hot_write() -> Result<()> {
        let dir = tempfile::tempdir().map_err(ReactError::from)?;
        let store = Arc::new(InMemoryStore::new());
        let first = MemoryLayerManager::new(
            dir.path().to_path_buf(),
            store.clone(),
            Box::new(NullChangeLog),
        );
        let second =
            MemoryLayerManager::new(dir.path().to_path_buf(), store, Box::new(NullChangeLog));
        let eligible = MemoryMeta::new(
            MemoryType::UserPreference,
            MemorySource::ExplicitSave,
            "style",
        )
        .with_confidence(0.95)
        .with_stability(0.90);
        write_approved_memory(&first, "shared", "Old fact", eligible.clone()).await?;
        let (_, stale) = first
            .locate("shared")
            .await?
            .ok_or_else(|| merge_plan_error("missing initial hot memory"))?;
        write_approved_memory(&second, "shared", "New fact", eligible).await?;

        assert!(
            first
                .demote_hot_to_warm("shared", stale, "stale decision")
                .await
                .is_err()
        );
        let (layer, current) = first
            .locate("shared")
            .await?
            .ok_or_else(|| merge_plan_error("newer hot memory disappeared"))?;
        assert_eq!(layer, MemoryLayer::Hot);
        assert_eq!(current.content, "New fact");
        Ok(())
    }

    #[tokio::test]
    async fn stale_warm_archive_snapshot_cannot_overwrite_newer_write() -> Result<()> {
        let dir = tempfile::tempdir().map_err(ReactError::from)?;
        let store = Arc::new(InMemoryStore::new());
        let first = MemoryLayerManager::new(
            dir.path().to_path_buf(),
            store.clone(),
            Box::new(NullChangeLog),
        );
        let second =
            MemoryLayerManager::new(dir.path().to_path_buf(), store, Box::new(NullChangeLog));
        let meta = MemoryMeta::new(
            MemoryType::ProjectFact,
            MemorySource::AutoExtracted,
            "build",
        );
        first
            .write_memory("shared", "Old fact", meta.clone())
            .await?;
        let (_, stale) = first
            .locate("shared")
            .await?
            .ok_or_else(|| merge_plan_error("missing initial warm memory"))?;
        second.write_memory("shared", "New fact", meta).await?;
        let mut archived = stale.meta.clone();
        archived.status = MemoryStatus::Archived;

        let result = first
            .transition(
                "shared",
                Some(MemoryLayerManager::typed_value(&stale.content, archived)?),
                None,
                MemoryLayerManager::change_builder(
                    "shared",
                    ChangeType::Demote,
                    Some("warm"),
                    Some("archived"),
                    "stale decision",
                    "demote",
                ),
                Some((MemoryLayer::Warm, &stale)),
            )
            .await;
        assert!(result.is_err());
        let (_, current) = first
            .locate("shared")
            .await?
            .ok_or_else(|| merge_plan_error("newer warm memory disappeared"))?;
        assert_eq!(current.content, "New fact");
        assert_eq!(current.meta.status, MemoryStatus::Draft);
        Ok(())
    }

    #[tokio::test]
    async fn test_write_memory_not_eligible_for_hot() -> Result<()> {
        let manager = make_manager();

        // Low confidence → should NOT be promoted to hot
        let meta = MemoryMeta::new(
            MemoryType::ProjectFact,
            MemorySource::AutoExtracted,
            "project",
        )
        .with_confidence(0.50)
        .with_stability(0.30);

        let result = manager
            .write_memory("low_conf", "Some low-confidence fact", meta)
            .await
            .expect("write_memory");

        // Not eligible → should stay in warm
        assert!(result.is_none());
        assert!(manager.read_hot_content()?.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn approved_merge_can_be_restored_exactly() -> crate::error::Result<()> {
        let store = Arc::new(InMemoryStore::new());
        let dir = tempfile::tempdir()
            .map_err(|error| merge_plan_error(format!("tempdir failed: {error}")))?
            .keep();
        let manager = MemoryLayerManager::new(dir, store, Box::new(NullChangeLog));
        let high = MemoryMeta::new(MemoryType::ProjectFact, MemorySource::ExplicitSave, "build")
            .with_confidence(0.95);
        let low = MemoryMeta::new(
            MemoryType::ProjectFact,
            MemorySource::AutoExtracted,
            "build",
        )
        .with_confidence(0.55);
        manager
            .typed_store
            .put_typed(WARM_NAMESPACE, "build_a", "Build uses cargo", high)
            .await?;
        manager
            .typed_store
            .put_typed(WARM_NAMESPACE, "build_b", "Build uses make", low)
            .await?;
        let entries = manager.list_warm_memories(&MemoryFilter::new()).await?;
        let group = ConflictDetector::new()
            .detect(&entries)
            .into_iter()
            .next()
            .ok_or_else(|| merge_plan_error("expected conflict"))?;
        let proposal = MemoryConflictProposal::from_group(&group)
            .ok_or_else(|| merge_plan_error("expected proposal"))?;

        let applied = manager.apply_merge_proposal(&proposal).await?;
        let secondary = manager
            .typed_store
            .get_typed(WARM_NAMESPACE, "build_b")
            .await?
            .ok_or_else(|| merge_plan_error("expected secondary"))?;
        assert_eq!(secondary.meta.status, MemoryStatus::Superseded);

        let batch_id = applied
            .batch_id
            .clone()
            .ok_or_else(|| merge_plan_error("merge did not return a durable batch ID"))?;
        let rollback = manager
            .rollback_memory("merge-undo-1", MemoryRollbackTarget::batch_id(batch_id))
            .await?;
        assert!(matches!(rollback, MemoryRollbackOutcome::Applied(_)));
        let primary = manager
            .typed_store
            .get_typed(WARM_NAMESPACE, "build_a")
            .await?
            .ok_or_else(|| merge_plan_error("expected restored primary"))?;
        assert_eq!(primary.meta.status, MemoryStatus::Active);
        assert_eq!(primary.content, "Build uses cargo");
        let restored = manager
            .typed_store
            .get_typed(WARM_NAMESPACE, "build_b")
            .await?
            .ok_or_else(|| merge_plan_error("expected restored secondary"))?;
        assert_eq!(restored.meta.status, MemoryStatus::Active);
        assert_eq!(restored.content, "Build uses make");
        Ok(())
    }

    #[tokio::test]
    async fn later_rollback_restores_a_settled_memory_change_idempotently()
    -> crate::error::Result<()> {
        let store = Arc::new(InMemoryStore::new());
        let dir = tempfile::tempdir()
            .map_err(|error| merge_plan_error(format!("tempdir failed: {error}")))?
            .keep();
        let audit_path = dir.join("evolution").join("change-log.jsonl");
        let manager = MemoryLayerManager::new(
            dir,
            store,
            Box::new(JsonlChangeLog::new(audit_path.clone())?),
        );
        let meta = MemoryMeta::new(
            MemoryType::ProjectFact,
            MemorySource::ExplicitSave,
            "rollback",
        )
        .with_confidence(0.40);
        manager
            .write_memory("rollback/key", "before", meta.clone())
            .await?;
        manager.write_memory("rollback/key", "after", meta).await?;
        let changes =
            JsonlChangeLog::new(audit_path)?.query(&super::super::audit::ChangeFilter::new())?;
        let target = MemoryRollbackTarget::change_id(
            changes
                .first()
                .ok_or_else(|| merge_plan_error("expected update audit"))?
                .change_id
                .clone(),
        );

        let preview = manager.preview_rollback(&target).await?;
        assert!(matches!(preview, MemoryRollbackPreviewOutcome::Ready(_)));
        let applied = manager.rollback_memory("request-1", target.clone()).await?;
        assert!(matches!(applied, MemoryRollbackOutcome::Applied(_)));
        let (_, restored) = manager
            .locate("rollback/key")
            .await?
            .ok_or_else(|| merge_plan_error("expected restored memory"))?;
        assert_eq!(restored.content, "before");

        let retry = manager.rollback_memory("request-1", target).await?;
        assert!(matches!(retry, MemoryRollbackOutcome::AlreadyApplied(_)));
        Ok(())
    }

    #[tokio::test]
    async fn later_rollback_rejects_non_tip_and_aba_targets() -> crate::error::Result<()> {
        let store = Arc::new(InMemoryStore::new());
        let dir = tempfile::tempdir()
            .map_err(|error| merge_plan_error(format!("tempdir failed: {error}")))?
            .keep();
        let audit_path = dir.join("evolution").join("change-log.jsonl");
        let manager = MemoryLayerManager::new(
            dir,
            store,
            Box::new(JsonlChangeLog::new(audit_path.clone())?),
        );
        let meta = MemoryMeta::new(
            MemoryType::ProjectFact,
            MemorySource::ExplicitSave,
            "rollback",
        )
        .with_confidence(0.40);
        manager.write_memory("aba", "same", meta.clone()).await?;
        manager
            .write_memory("aba", "different", meta.clone())
            .await?;
        manager.write_memory("aba", "same", meta).await?;
        let changes = JsonlChangeLog::new(audit_path)?.query(&ChangeFilter::new())?;
        let first = changes
            .get(2)
            .ok_or_else(|| merge_plan_error("expected three memory changes"))?;
        let outcome = manager
            .preview_rollback(&MemoryRollbackTarget::change_id(&first.change_id))
            .await?;
        assert!(matches!(
            outcome,
            MemoryRollbackPreviewOutcome::Conflict(MemoryRollbackConflict { .. })
        ));
        Ok(())
    }

    #[tokio::test]
    async fn rollback_of_rollback_is_a_new_batch_and_request_conflicts_fail_closed()
    -> crate::error::Result<()> {
        let store = Arc::new(InMemoryStore::new());
        let dir = tempfile::tempdir()
            .map_err(|error| merge_plan_error(format!("tempdir failed: {error}")))?
            .keep();
        let audit_path = dir.join("evolution").join("change-log.jsonl");
        let manager = MemoryLayerManager::new(
            dir,
            store,
            Box::new(JsonlChangeLog::new(audit_path.clone())?),
        );
        let meta = MemoryMeta::new(
            MemoryType::ProjectFact,
            MemorySource::ExplicitSave,
            "rollback",
        )
        .with_confidence(0.40);
        manager
            .write_memory("lineage", "before", meta.clone())
            .await?;
        manager.write_memory("lineage", "after", meta).await?;
        let changes = JsonlChangeLog::new(audit_path)?.query(&ChangeFilter::new())?;
        let target = MemoryRollbackTarget::change_id(
            changes
                .first()
                .ok_or_else(|| merge_plan_error("expected target change"))?
                .change_id
                .clone(),
        );
        let applied = manager.rollback_memory("rollback-a", target).await?;
        let receipt = match applied {
            MemoryRollbackOutcome::Applied(receipt) => receipt,
            other => return Err(merge_plan_error(format!("unexpected outcome: {other:?}"))),
        };
        let conflict = manager
            .rollback_memory(
                "rollback-a",
                MemoryRollbackTarget::batch_id(receipt.inverse_batch_id.clone()),
            )
            .await?;
        assert!(matches!(
            conflict,
            MemoryRollbackOutcome::Conflict(MemoryRollbackConflict { .. })
        ));

        let reverted = manager
            .rollback_memory(
                "rollback-b",
                MemoryRollbackTarget::batch_id(receipt.inverse_batch_id),
            )
            .await?;
        assert!(matches!(reverted, MemoryRollbackOutcome::Applied(_)));
        let (_, current) = manager
            .locate("lineage")
            .await?
            .ok_or_else(|| merge_plan_error("expected lineage memory"))?;
        assert_eq!(current.content, "after");
        Ok(())
    }

    #[tokio::test]
    async fn rollback_receipt_survives_manager_restart() -> crate::error::Result<()> {
        let dir = tempfile::tempdir()
            .map_err(|error| merge_plan_error(format!("tempdir failed: {error}")))?;
        let root = dir.path().join(".echo-agent");
        let store_path = dir.path().join("store.json");
        let audit_path = root.join("evolution").join("change-log.jsonl");
        let meta = MemoryMeta::new(
            MemoryType::ProjectFact,
            MemorySource::ExplicitSave,
            "rollback",
        )
        .with_confidence(0.40);
        let target;
        {
            let store = Arc::new(FileStore::new(&store_path)?);
            let manager = MemoryLayerManager::try_new(
                root.clone(),
                store,
                Box::new(JsonlChangeLog::new(audit_path.clone())?),
            )?;
            manager
                .write_memory("restart", "before", meta.clone())
                .await?;
            manager.write_memory("restart", "after", meta).await?;
            let changes = JsonlChangeLog::new(audit_path.clone())?.query(&ChangeFilter::new())?;
            target = MemoryRollbackTarget::change_id(
                changes
                    .first()
                    .ok_or_else(|| merge_plan_error("expected restart target"))?
                    .change_id
                    .clone(),
            );
            let applied = manager
                .rollback_memory("restart-request", target.clone())
                .await?;
            assert!(matches!(applied, MemoryRollbackOutcome::Applied(_)));
        }
        let store = Arc::new(FileStore::new(&store_path)?);
        let manager =
            MemoryLayerManager::try_new(root, store, Box::new(JsonlChangeLog::new(audit_path)?))?;
        manager.reconcile_pending().await?;
        let (_, restored) = manager
            .locate("restart")
            .await?
            .ok_or_else(|| merge_plan_error("expected restarted memory"))?;
        assert_eq!(restored.content, "before");
        let retry = manager.rollback_memory("restart-request", target).await?;
        assert!(matches!(retry, MemoryRollbackOutcome::AlreadyApplied(_)));
        Ok(())
    }

    #[tokio::test]
    async fn rollback_audit_failure_reconciles_inverse_after_restart() -> crate::error::Result<()> {
        let dir = tempfile::tempdir()
            .map_err(|error| merge_plan_error(format!("tempdir failed: {error}")))?;
        let root = dir.path().join(".echo-agent");
        let store_path = dir.path().join("store.json");
        let audit_path = root.join("evolution").join("change-log.jsonl");
        let meta = MemoryMeta::new(
            MemoryType::ProjectFact,
            MemorySource::ExplicitSave,
            "rollback",
        )
        .with_confidence(0.40);
        let target;
        {
            let manager = MemoryLayerManager::try_new(
                root.clone(),
                Arc::new(FileStore::new(&store_path)?),
                Box::new(JsonlChangeLog::new(audit_path.clone())?),
            )?;
            manager
                .write_memory("audit-crash", "before", meta.clone())
                .await?;
            manager.write_memory("audit-crash", "after", meta).await?;
            let changes = JsonlChangeLog::new(audit_path.clone())?.query(&ChangeFilter::new())?;
            target = MemoryRollbackTarget::change_id(
                changes
                    .first()
                    .ok_or_else(|| merge_plan_error("expected audit-crash target"))?
                    .change_id
                    .clone(),
            );
        }
        {
            let failing_log = FailOnceChangeLog {
                inner: JsonlChangeLog::new(audit_path.clone())?,
                fail_next: AtomicBool::new(true),
            };
            let manager = MemoryLayerManager::try_new(
                root.clone(),
                Arc::new(FileStore::new(&store_path)?),
                Box::new(failing_log),
            )?;
            assert!(
                manager
                    .rollback_memory("audit-crash-request", target.clone())
                    .await
                    .is_err()
            );
        }
        let manager = MemoryLayerManager::try_new(
            root,
            Arc::new(FileStore::new(&store_path)?),
            Box::new(JsonlChangeLog::new(audit_path.clone())?),
        )?;
        manager.reconcile_pending().await?;
        let (_, restored) = manager
            .locate("audit-crash")
            .await?
            .ok_or_else(|| merge_plan_error("expected audit-crash memory"))?;
        assert_eq!(restored.content, "before");
        let retry = manager
            .rollback_memory("audit-crash-request", target)
            .await?;
        assert!(matches!(retry, MemoryRollbackOutcome::AlreadyApplied(_)));
        Ok(())
    }

    #[tokio::test]
    async fn inverse_projection_and_audit_matrix_matches_real_memory_outcomes()
    -> crate::error::Result<()> {
        let store = Arc::new(InMemoryStore::new());
        let root = tempfile::tempdir()
            .map_err(|error| merge_plan_error(format!("tempdir failed: {error}")))?
            .keep();
        let audit_path = root.join("evolution").join("change-log.jsonl");
        let manager = MemoryLayerManager::new(
            root,
            store,
            Box::new(JsonlChangeLog::new(audit_path.clone())?),
        );
        let warm_meta = MemoryMeta::new(
            MemoryType::ProjectFact,
            MemorySource::ExplicitSave,
            "rollback-matrix",
        )
        .with_confidence(0.40);
        let hot_meta = MemoryMeta::new(
            MemoryType::UserPreference,
            MemorySource::ExplicitSave,
            "rollback-matrix",
        )
        .with_confidence(0.95)
        .with_stability(0.90);

        manager
            .write_memory("matrix-create", "created", warm_meta.clone())
            .await?;
        let create = latest_memory_audit(&audit_path, "matrix-create")?;
        manager
            .rollback_memory(
                "matrix-create-request",
                MemoryRollbackTarget::change_id(create.change_id),
            )
            .await?;
        let inverse_create = latest_memory_audit(&audit_path, "matrix-create")?;
        assert_machine_readable_rollback_summary(&inverse_create)?;
        assert_eq!(inverse_create.change_type, ChangeType::Delete);
        assert_eq!(audit_projection_layer(&inverse_create, true), Some("warm"));
        assert_eq!(
            audit_projection_layer(&inverse_create, false),
            Some("absent")
        );
        assert!(manager.locate("matrix-create").await?.is_none());

        manager
            .write_memory("matrix-update", "before", warm_meta.clone())
            .await?;
        manager
            .write_memory("matrix-update", "after", warm_meta.clone())
            .await?;
        let update = latest_memory_audit(&audit_path, "matrix-update")?;
        manager
            .rollback_memory(
                "matrix-update-request",
                MemoryRollbackTarget::change_id(update.change_id),
            )
            .await?;
        let inverse_update = latest_memory_audit(&audit_path, "matrix-update")?;
        assert_machine_readable_rollback_summary(&inverse_update)?;
        assert_eq!(inverse_update.change_type, ChangeType::Update);
        assert_eq!(audit_projection_layer(&inverse_update, true), Some("warm"));
        assert_eq!(audit_projection_layer(&inverse_update, false), Some("warm"));
        let (_, updated) = manager
            .locate("matrix-update")
            .await?
            .ok_or_else(|| merge_plan_error("matrix update memory disappeared"))?;
        assert_eq!(updated.content, "before");

        manager
            .write_memory("matrix-delete", "deleted", warm_meta.clone())
            .await?;
        assert!(manager.delete_memory("matrix-delete").await?);
        let delete = latest_memory_audit(&audit_path, "matrix-delete")?;
        manager
            .rollback_memory(
                "matrix-delete-request",
                MemoryRollbackTarget::change_id(delete.change_id),
            )
            .await?;
        let inverse_delete = latest_memory_audit(&audit_path, "matrix-delete")?;
        assert_machine_readable_rollback_summary(&inverse_delete)?;
        assert_eq!(inverse_delete.change_type, ChangeType::Create);
        assert_eq!(
            audit_projection_layer(&inverse_delete, true),
            Some("absent")
        );
        assert_eq!(audit_projection_layer(&inverse_delete, false), Some("warm"));
        let (_, restored_delete) = manager
            .locate("matrix-delete")
            .await?
            .ok_or_else(|| merge_plan_error("matrix delete memory was not restored"))?;
        assert_eq!(restored_delete.content, "deleted");

        write_approved_memory(&manager, "matrix-promote", "promoted", hot_meta.clone()).await?;
        let promote = latest_memory_audit(&audit_path, "matrix-promote")?;
        assert_eq!(promote.change_type, ChangeType::Promote);
        manager
            .rollback_memory(
                "matrix-promote-request",
                MemoryRollbackTarget::change_id(promote.change_id),
            )
            .await?;
        let inverse_promote = latest_memory_audit(&audit_path, "matrix-promote")?;
        assert_machine_readable_rollback_summary(&inverse_promote)?;
        assert_eq!(inverse_promote.change_type, ChangeType::Demote);
        assert_eq!(audit_projection_layer(&inverse_promote, true), Some("hot"));
        assert_eq!(
            audit_projection_layer(&inverse_promote, false),
            Some("warm")
        );
        assert!(matches!(
            manager.locate("matrix-promote").await?,
            Some((MemoryLayer::Warm, _))
        ));

        write_approved_memory(&manager, "matrix-demote", "demoted", hot_meta).await?;
        manager.demote("matrix-demote", "matrix demotion").await?;
        let demote = latest_memory_audit(&audit_path, "matrix-demote")?;
        assert_eq!(demote.change_type, ChangeType::Demote);
        manager
            .rollback_memory(
                "matrix-demote-request",
                MemoryRollbackTarget::change_id(demote.change_id),
            )
            .await?;
        let inverse_demote = latest_memory_audit(&audit_path, "matrix-demote")?;
        assert_machine_readable_rollback_summary(&inverse_demote)?;
        assert_eq!(inverse_demote.change_type, ChangeType::Promote);
        assert_eq!(audit_projection_layer(&inverse_demote, true), Some("warm"));
        assert_eq!(audit_projection_layer(&inverse_demote, false), Some("hot"));
        assert!(matches!(
            manager.locate("matrix-demote").await?,
            Some((MemoryLayer::Hot, _))
        ));

        manager
            .write_memory("matrix-meta", "metadata", warm_meta.clone())
            .await?;
        manager.demote("matrix-meta", "archive metadata").await?;
        let meta_change = latest_memory_audit(&audit_path, "matrix-meta")?;
        manager
            .rollback_memory(
                "matrix-meta-request",
                MemoryRollbackTarget::change_id(meta_change.change_id),
            )
            .await?;
        let inverse_meta = latest_memory_audit(&audit_path, "matrix-meta")?;
        assert_machine_readable_rollback_summary(&inverse_meta)?;
        assert_eq!(inverse_meta.change_type, ChangeType::Promote);
        assert_eq!(audit_projection_layer(&inverse_meta, true), Some("warm"));
        assert_eq!(audit_projection_layer(&inverse_meta, false), Some("warm"));
        let (_, restored_meta) = manager
            .locate("matrix-meta")
            .await?
            .ok_or_else(|| merge_plan_error("matrix metadata memory disappeared"))?;
        assert_eq!(restored_meta.meta.status, MemoryStatus::Draft);

        let multiline = "  first line\nsecond line  \n";
        manager
            .write_memory("matrix-multiline", multiline, warm_meta)
            .await?;
        let multiline_create = latest_memory_audit(&audit_path, "matrix-multiline")?;
        manager
            .rollback_memory(
                "matrix-multiline-delete",
                MemoryRollbackTarget::change_id(multiline_create.change_id),
            )
            .await?;
        let multiline_delete = latest_memory_audit(&audit_path, "matrix-multiline")?;
        manager
            .rollback_memory(
                "matrix-multiline-restore",
                MemoryRollbackTarget::change_id(multiline_delete.change_id),
            )
            .await?;
        let multiline_restore = latest_memory_audit(&audit_path, "matrix-multiline")?;
        assert_machine_readable_rollback_summary(&multiline_restore)?;
        assert_eq!(multiline_restore.change_type, ChangeType::Create);
        assert_eq!(
            audit_projection_layer(&multiline_restore, true),
            Some("absent")
        );
        assert_eq!(
            audit_projection_layer(&multiline_restore, false),
            Some("warm")
        );
        let (_, restored_multiline) = manager
            .locate("matrix-multiline")
            .await?
            .ok_or_else(|| merge_plan_error("matrix multiline memory disappeared"))?;
        assert_eq!(restored_multiline.content, multiline);
        Ok(())
    }

    #[tokio::test]
    async fn stale_merge_proposal_fails_before_mutation() -> crate::error::Result<()> {
        let store = Arc::new(InMemoryStore::new());
        let dir = tempfile::tempdir()
            .map_err(|error| merge_plan_error(format!("tempdir failed: {error}")))?
            .keep();
        let manager = MemoryLayerManager::new(dir, store, Box::new(NullChangeLog));
        let meta = MemoryMeta::new(MemoryType::ProjectFact, MemorySource::ExplicitSave, "build");
        manager
            .typed_store
            .put_typed(WARM_NAMESPACE, "a", "cargo", meta.clone())
            .await?;
        manager
            .typed_store
            .put_typed(WARM_NAMESPACE, "b", "make", meta.clone())
            .await?;
        let entries = manager.list_warm_memories(&MemoryFilter::new()).await?;
        let group = ConflictDetector::new()
            .detect(&entries)
            .into_iter()
            .next()
            .ok_or_else(|| merge_plan_error("expected conflict"))?;
        let proposal = MemoryConflictProposal::from_group(&group)
            .ok_or_else(|| merge_plan_error("expected proposal"))?;
        manager
            .typed_store
            .put_typed(WARM_NAMESPACE, "b", "ninja", meta)
            .await?;

        assert!(manager.apply_merge_proposal(&proposal).await.is_err());
        let a = manager
            .typed_store
            .get_typed(WARM_NAMESPACE, "a")
            .await?
            .ok_or_else(|| merge_plan_error("expected primary"))?;
        assert_eq!(a.meta.status, MemoryStatus::Active);
        Ok(())
    }

    #[tokio::test]
    async fn test_locate_in_warm() -> Result<()> {
        let manager = make_manager();

        let meta = MemoryMeta::new(
            MemoryType::ProjectFact,
            MemorySource::AutoExtracted,
            "project",
        );
        manager
            .typed_store
            .put_typed(WARM_NAMESPACE, "warm_key", "A warm memory", meta)
            .await
            .expect("put_typed");

        let location = manager.locate("warm_key").await?;
        assert!(location.is_some());
        let (layer, _) = location.unwrap();
        assert_eq!(layer, MemoryLayer::Warm);
        Ok(())
    }

    #[tokio::test]
    async fn test_locate_not_found() -> Result<()> {
        let manager = make_manager();
        let location = manager.locate("nonexistent").await?;
        assert!(location.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn test_demote_hot_to_warm() -> Result<()> {
        let manager = make_manager();

        let entry = TypedMemoryEntry {
            key: "demote_test".to_string(),
            content: "Test memory to demote".to_string(),
            meta: approved_test_meta(
                MemoryMeta::new(
                    MemoryType::UserPreference,
                    MemorySource::ExplicitSave,
                    "style",
                )
                .with_confidence(0.95)
                .with_stability(0.90),
                "Test memory to demote",
            ),
            raw: echo_core::memory::store::StoreItem::new(
                vec!["agent".to_string()],
                "demote_test".to_string(),
                serde_json::Value::Null,
            ),
        };

        manager.add_to_hot(&entry).await.expect("add to hot");
        assert!(manager.read_hot_content()?.contains("**[demote_test]**"));

        let result = manager
            .demote("demote_test", "test demotion")
            .await
            .expect("demote");
        assert_eq!(result.from_layer, MemoryLayer::Hot);
        assert_eq!(result.to_layer, MemoryLayer::Warm);

        // Should no longer be in hot
        assert!(!manager.read_hot_content()?.contains("**[demote_test]**"));

        // Should be in warm
        let location = manager.locate("demote_test").await?;
        assert!(location.is_some());
        let (layer, _) = location.unwrap();
        assert_eq!(layer, MemoryLayer::Warm);
        Ok(())
    }

    #[tokio::test]
    async fn test_search_layered_returns_hot_first() {
        let manager = make_manager();

        // Add to hot
        let hot_entry = TypedMemoryEntry {
            key: "hot_build".to_string(),
            content: "Hot build memory".to_string(),
            meta: approved_test_meta(
                MemoryMeta::new(
                    MemoryType::DebuggingLesson,
                    MemorySource::ExplicitSave,
                    "build",
                )
                .with_confidence(0.95)
                .with_stability(0.90),
                "Hot build memory",
            ),
            raw: echo_core::memory::store::StoreItem::new(
                vec!["agent".to_string()],
                "hot_build".to_string(),
                serde_json::Value::Null,
            ),
        };
        manager.add_to_hot(&hot_entry).await.expect("add to hot");

        // Add to warm
        let warm_meta = approved_test_meta(
            MemoryMeta::new(
                MemoryType::ProjectFact,
                MemorySource::AutoExtracted,
                "build",
            ),
            "Warm build memory",
        );
        manager
            .typed_store
            .put_typed(WARM_NAMESPACE, "warm_build", "Warm build memory", warm_meta)
            .await
            .expect("put_typed");

        let results = manager
            .search_layered("build", 10)
            .await
            .expect("search_layered");

        // Hot should come first
        if results.len() >= 2 {
            assert_eq!(results[0].0, MemoryLayer::Hot);
        }
        assert!(results.iter().any(|(l, _)| *l == MemoryLayer::Hot));
    }

    #[test]
    fn test_memory_layer_display() {
        assert_eq!(MemoryLayer::Hot.to_string(), "hot");
        assert_eq!(MemoryLayer::Warm.to_string(), "warm");
    }

    /// Regression for P0-3 (MEMORY.md no lock): two concurrent `add_to_hot`
    /// calls on the same `MemoryLayerManager` must both land. Before locking,
    /// each read the file, pushed its entry, and renamed — the second rename
    /// silently dropped the first writer's entry.
    #[tokio::test]
    async fn test_concurrent_add_to_hot_does_not_lose_entries() -> Result<()> {
        use std::sync::Arc;
        let dir = tempfile::tempdir().expect("tempdir");
        let dir_path = dir.keep();
        let store = Arc::new(InMemoryStore::new());
        let manager = Arc::new(MemoryLayerManager::new(
            dir_path,
            store,
            Box::new(NullChangeLog),
        ));

        let mk_entry = |key: &'static str| TypedMemoryEntry {
            key: key.to_string(),
            content: format!("memory {key}"),
            meta: approved_test_meta(
                MemoryMeta::new(
                    MemoryType::UserPreference,
                    MemorySource::ExplicitSave,
                    "topic",
                ),
                &format!("memory {key}"),
            ),
            raw: echo_core::memory::store::StoreItem::new(
                vec!["agent".to_string()],
                key.to_string(),
                serde_json::Value::Null,
            ),
        };

        // Fire both adds concurrently.
        let m1 = manager.clone();
        let m2 = manager.clone();
        let e1 = mk_entry("concurrent-a");
        let e2 = mk_entry("concurrent-b");
        let (r1, r2) = tokio::join!(async move { m1.add_to_hot(&e1).await }, async move {
            m2.add_to_hot(&e2).await
        },);
        r1.expect("add a");
        r2.expect("add b");

        let content = manager.read_hot_content()?;
        assert!(
            content.contains("**[concurrent-a]**"),
            "concurrent-a lost (MEMORY.md TOCTOU regression)"
        );
        assert!(
            content.contains("**[concurrent-b]**"),
            "concurrent-b lost (MEMORY.md TOCTOU regression)"
        );
        assert_eq!(manager.read_hot_meta()?.len(), 2);
        Ok(())
    }
}
