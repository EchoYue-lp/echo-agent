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
//! | **Hot** | `.echo-agent/MEMORY.md` (YAML frontmatter + markdown body) | File | Always loaded into context, max ~2000 tokens |
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

use echo_core::memory::store::Store;
use echo_core::memory::types::{MemoryMeta, MemoryRisk, MemorySource, MemoryStatus, MemoryType};
use echo_state::memory::typed_store::{MemoryFilter, TypedMemoryEntry, TypedMemoryStore};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::Mutex;

use super::audit::{ChangeEntryBuilder, ChangeLog, ChangeType, EntityType};
use super::security::{EvolutionSecurityGuard, InputTrustLevel};
use echo_core::error::{ConfigError, ReactError};

/// Alias for layer operation results.
type Result<T> = std::result::Result<T, ReactError>;

// ── Constants ──────────────────────────────────────────────────────────

/// Namespace for the unified typed-memory store (stage4: warm+cold+L3 all
/// merged here; Archived status replaces the former cold namespace).
pub const WARM_NAMESPACE: &[&str] = &["agent", "memories"];

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
}

impl std::fmt::Display for MemoryLayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Hot => write!(f, "hot"),
            Self::Warm => write!(f, "warm"),
        }
    }
}

// ── HotEntryMeta ───────────────────────────────────────────────────────

/// Metadata for a single entry within the MEMORY.md hot layer.
///
/// Stored in YAML frontmatter alongside the markdown body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HotEntryMeta {
    /// Key identifier for this memory.
    pub key: String,
    /// Memory type classification.
    pub memory_type: MemoryType,
    /// Confidence score (0.0-1.0).
    pub confidence: f32,
    /// Stability score (0.0-1.0).
    pub stability: f32,
    /// Source of this memory.
    pub source: MemorySource,
    /// Topic category.
    pub topic: String,
    /// Risk level.
    #[serde(default)]
    pub risk: MemoryRisk,
    /// When this entry was promoted to hot (ISO 8601).
    pub last_promoted: String,
}

impl HotEntryMeta {
    /// Convert to a MemoryMeta for reconstruction.
    fn to_memory_meta(&self) -> MemoryMeta {
        MemoryMeta::new(self.memory_type, self.source, &self.topic)
            .with_confidence(self.confidence)
            .with_stability(self.stability)
            .with_risk(self.risk)
    }
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

// ── MemoryLayerManager ─────────────────────────────────────────────────

/// Manages the three-tier memory layer system.
///
/// - **Hot layer**: `.echo-agent/MEMORY.md` (YAML frontmatter + markdown body).
///   Always loaded into context. Max ~2000 tokens.
/// - **Warm layer**: Store KV under `["agent", "typed_memories"]`.
///   Available on-demand via search.
/// - **Cold layer**: Store KV under `["agent", "cold_memories"]`.
///   Archive for old/low-confidence memories.
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
    /// Security guard for write-time checks (secret scan, injection, rate limit).
    security_guard: EvolutionSecurityGuard,
    /// Shared write counter for triggering periodic reviews.
    write_counter: Arc<AtomicU64>,
    /// Number of writes between automatic reviews.
    review_every_n_writes: u64,
    /// Optional observer called after a real memory write succeeds.
    write_observer: Option<Arc<dyn MemoryWriteObserver>>,
}

/// Observer notified only after a memory write has reached the real layered store.
pub trait MemoryWriteObserver: Send + Sync {
    /// Called after [`MemoryLayerManager::write_memory`] succeeds.
    fn on_memory_write<'a>(&'a self) -> BoxFuture<'a, ()>;
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
        Self {
            hot_path,
            hot_lock: Mutex::new(()),
            typed_store: TypedMemoryStore::new(store),
            change_log,
            security_guard: EvolutionSecurityGuard::default_config(),
            write_counter: Arc::new(AtomicU64::new(0)),
            review_every_n_writes: 50,
            write_observer: None,
        }
    }

    /// Configure the review trigger threshold and shared write counter.
    ///
    /// Pass the same `Arc<AtomicU64>` that your [`ReviewIntegration`] uses so
    /// the layer manager can increment it on every write.
    pub fn with_review_trigger(mut self, counter: Arc<AtomicU64>, every_n: u64) -> Self {
        self.write_counter = counter;
        self.review_every_n_writes = every_n;
        self
    }

    /// Configure a write observer invoked after successful real memory writes.
    pub fn with_write_observer(mut self, observer: Arc<dyn MemoryWriteObserver>) -> Self {
        self.write_observer = Some(observer);
        self
    }

    /// Get a clone of the shared write counter (for external readers like `ReviewIntegration`).
    pub fn write_counter(&self) -> Arc<AtomicU64> {
        self.write_counter.clone()
    }

    // ── Reading ─────────────────────────────────────────────────────

    /// Read the hot layer content (MEMORY.md body, frontmatter stripped).
    ///
    /// Returns empty string if the file doesn't exist or has no body.
    pub fn read_hot_content(&self) -> String {
        self.parse_memory_file().body
    }

    /// Read the hot layer metadata (MEMORY.md YAML frontmatter entries).
    ///
    /// Returns empty vec if the file doesn't exist or has no frontmatter.
    pub fn read_hot_meta(&self) -> Vec<HotEntryMeta> {
        self.parse_memory_file().entries
    }

    /// Determine which layer a memory key currently resides in.
    ///
    /// Checks hot (MEMORY.md), then warm (Store). Stage4 removed the cold
    /// layer — Archived memories live on in the warm/unified namespace.
    /// Returns `None` if the key is not found in any layer.
    pub async fn locate(&self, key: &str) -> Option<(MemoryLayer, TypedMemoryEntry)> {
        // Check hot layer
        let file = self.parse_memory_file();
        if let Some(entry) = self.find_in_hot(&file, key) {
            return Some((MemoryLayer::Hot, entry));
        }

        // Check warm layer (unified namespace; includes Archived entries)
        if let Ok(Some(entry)) = self.typed_store.get_typed(WARM_NAMESPACE, key).await {
            return Some((MemoryLayer::Warm, entry));
        }

        None
    }

    /// Get all hot entries as TypedMemoryEntries (reconstructed from MEMORY.md).
    pub fn list_hot(&self) -> Vec<TypedMemoryEntry> {
        let file = self.parse_memory_file();
        file.entries
            .iter()
            .filter_map(|meta| self.hot_meta_to_entry(meta, &file.body))
            .collect()
    }

    /// Get all warm entries matching a filter.
    pub async fn list_warm(&self, filter: &MemoryFilter) -> Result<Vec<TypedMemoryEntry>> {
        self.typed_store.list_typed(WARM_NAMESPACE, filter).await
    }

    // ── Promotion / Demotion ────────────────────────────────────────
    // (stage4 B1) `promote` (the cold→warm dispatcher) was removed — zero
    // callers, and the cold layer no longer exists. Warm→hot promotion still
    // happens via `consider_promotion` → `promote_warm_to_hot`.

    /// Demote a memory: hot→warm, or warm→Archived (in place).
    ///
    /// (stage4 B1) Warm demotion no longer moves the entry to a cold namespace;
    /// it marks `status = Archived` in place so the memory stays recallable
    /// (with decay) in the unified namespace. Revival Archived→Active is
    /// handled by Dreaming (stage 2). `update_meta` takes a full `MemoryMeta`
    /// (not a closure), hence get-modify-put.
    pub async fn demote(&self, key: &str, reason: &str) -> Result<LayerChangeResult> {
        let Some((layer, entry)) = self.locate(key).await else {
            return Err(ReactError::Config(Box::new(ConfigError::ConfigFileError(
                format!("Memory key '{key}' not found in any layer"),
            ))));
        };

        match layer {
            MemoryLayer::Hot => self.demote_hot_to_warm(key, entry, reason).await,
            MemoryLayer::Warm => {
                let mut meta = entry.meta.clone();
                meta.status = MemoryStatus::Archived;
                self.typed_store
                    .update_meta(WARM_NAMESPACE, key, meta)
                    .await?;
                self.record_change(
                    key,
                    ChangeType::Demote,
                    Some("warm"),
                    Some("archived"),
                    reason,
                    "demote",
                )?;
                Ok(LayerChangeResult {
                    key: key.to_string(),
                    from_layer: MemoryLayer::Warm,
                    to_layer: MemoryLayer::Warm,
                    reason: reason.to_string(),
                })
            }
        }
    }

    /// Consider promoting a warm entry to hot if eligible and space exists.
    ///
    /// Called after every new memory write.
    pub async fn consider_promotion(&self, key: &str) -> Result<Option<LayerChangeResult>> {
        let Some((layer, entry)) = self.locate(key).await else {
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
        if !InputTrustLevel::from_source(entry.meta.source).can_auto_promote() {
            return Ok(None);
        }

        self.promote_warm_to_hot(key, entry).await
    }

    /// Enforce the hot layer token budget by demoting lowest-priority entries.
    ///
    /// Called after every hot-layer write. Returns a list of demotion results.
    pub async fn enforce_hot_budget(&self) -> Result<Vec<LayerChangeResult>> {
        // Hold the hot-file lock across the whole demotion pass: we read the
        // body, demote entries to the warm Store (async), and rewrite the file,
        // all of which must be one atomic critical section so a concurrent
        // writer cannot interleave and lose entries (P0-3).
        let mut guard = self.lock_hot_file().await.map_err(ReactError::from)?;

        let tokens = estimate_tokens(&guard.file.body);
        if tokens <= HOT_TOKEN_BUDGET {
            return Ok(Vec::new());
        }

        // Sort entries by demotion score (highest = demote first)
        let mut scored: Vec<(f32, HotEntryMeta)> = guard
            .file
            .entries
            .iter()
            .map(|e| (Self::demotion_score(e), e.clone()))
            .collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        let mut results = Vec::new();
        let mut removed_keys: Vec<String> = Vec::new();

        // Demote in-place on the guard's body until within budget.
        for (_, meta) in scored {
            if estimate_tokens(&guard.file.body) <= HOT_TOKEN_BUDGET {
                break;
            }

            // Capture the entry's content BEFORE we strip its bullet from the
            // body (extract_hot_entry_content scans the body text).
            let removed_content = extract_hot_entry_content(&guard.file.body, &meta.key);

            // Remove this entry's bullet from the body.
            let pattern = format!("- **[{}]**", meta.key);
            guard.file.body = guard
                .file
                .body
                .lines()
                .filter(|line| !line.starts_with(&pattern))
                .collect::<Vec<_>>()
                .join("\n");
            if !guard.file.body.ends_with('\n') {
                guard.file.body.push('\n');
            }

            let warm_meta = meta.to_memory_meta();
            let removed_entry = TypedMemoryEntry {
                key: meta.key.clone(),
                content: removed_content,
                meta: warm_meta.clone(),
                raw: echo_core::memory::store::StoreItem::new(
                    WARM_NAMESPACE.iter().map(|s| s.to_string()).collect(),
                    meta.key.clone(),
                    serde_json::Value::Null,
                ),
            };

            removed_keys.push(meta.key.clone());

            // Write the removed entry to warm layer (async, but still under lock).
            self.typed_store
                .put_typed(WARM_NAMESPACE, &meta.key, &removed_entry.content, warm_meta)
                .await?;

            // Record the change
            self.record_change(
                &meta.key,
                ChangeType::Demote,
                Some("hot"),
                Some("warm"),
                "hot layer budget enforcement",
                "enforce_hot_budget",
            )?;

            results.push(LayerChangeResult {
                key: meta.key.clone(),
                from_layer: MemoryLayer::Hot,
                to_layer: MemoryLayer::Warm,
                reason: "hot layer budget enforcement".to_string(),
            });
        }

        // Drop the demoted entries from the frontmatter.
        if !removed_keys.is_empty() {
            guard
                .file
                .entries
                .retain(|e| !removed_keys.contains(&e.key));
            guard.commit().map_err(ReactError::from)?;
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

    /// Write a new typed memory to the warm layer and consider promotion.
    ///
    /// Returns `Ok(Some(result))` if the memory was promoted to hot.
    pub async fn write_memory(
        &self,
        key: &str,
        content: &str,
        meta: MemoryMeta,
    ) -> Result<Option<LayerChangeResult>> {
        // Security check: scan secrets, detect injection, rate limit, trust assignment.
        let trust = InputTrustLevel::from_source(meta.source);
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

        // Write to warm layer
        self.typed_store
            .put_typed(WARM_NAMESPACE, key, &safe_content, meta.clone())
            .await?;

        // Increment write counter and check if review should be triggered.
        let count = self.write_counter.fetch_add(1, Ordering::Relaxed) + 1;
        if count.is_multiple_of(self.review_every_n_writes) {
            tracing::info!(
                count,
                threshold = self.review_every_n_writes,
                "Memory write counter reached review threshold"
            );
        }

        // Record the creation
        self.record_change(
            key,
            ChangeType::Create,
            None,
            Some("warm"),
            &format!(
                "new memory via {}",
                meta.source.as_str().unwrap_or("unknown")
            ),
            "write_memory",
        )?;

        // Consider promotion
        let promotion = self.consider_promotion(key).await;
        if promotion.is_ok()
            && let Some(observer) = &self.write_observer
        {
            observer.on_memory_write().await;
        }
        promotion
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
        let mut results = Vec::new();

        // Search hot layer (keyword match on body)
        let file = self.parse_memory_file();
        let query_lower = query.to_lowercase();
        for meta in &file.entries {
            if results.len() >= limit {
                break;
            }
            // Simple keyword match: check if key or topic contains the query
            if (meta.key.to_lowercase().contains(&query_lower)
                || meta.topic.to_lowercase().contains(&query_lower))
                && let Some(entry) = self.hot_meta_to_entry(meta, &file.body)
            {
                results.push((MemoryLayer::Hot, entry));
            }
        }

        // (stage4 D1) Warm layer via the unified composite-score recall entry —
        // same ranking / Superseded filter / recall_count as the auto path.
        let remaining = limit.saturating_sub(results.len());
        if remaining > 0 {
            let reca =
                crate::evolution::recall::MemoryRecaller::new(self.typed_store.inner().clone());
            if let Ok(warm_items) = reca.recall(query, remaining).await {
                for item in warm_items {
                    let entry = TypedMemoryEntry::from_store_item(item);
                    results.push((MemoryLayer::Warm, entry));
                }
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
            let _ = tmp.sync_all();
        }
        std::fs::rename(&tmp_path, &self.hot_path)?;

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
    /// Note: the flock acquisition below blocks the worker thread. This is
    /// acceptable because curator/memory writes are infrequent and
    /// low-contention; the bounded retry loop prevents indefinite wedging.
    async fn lock_hot_file(&self) -> std::io::Result<HotFileGuard<'_>> {
        // In-process serialization first. tokio::sync::Mutex so the guard is
        // `Send` and can cross `.await`s.
        let inproc = self.hot_lock.lock().await;

        // Cross-process advisory lock on a sidecar file.
        let lock_path = self.hot_path.with_extension("md.lock");
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&lock_path);
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
            if lock_file.lock_exclusive().is_ok() {
                got_lock = true;
                break;
            }
            tokio::time::sleep(BACKOFF).await;
        }
        if !got_lock {
            tracing::error!(
                attempts = MAX_ATTEMPTS,
                "MEMORY.md lock unavailable after retries; proceeding WITHOUT cross-process lock (TOCTOU risk across processes)"
            );
        }

        let file = self.parse_memory_file();
        Ok(HotFileGuard {
            manager: self,
            file,
            _inproc: inproc,
            _lock_file: lock_file,
        })
    }

    /// Add an entry to the MEMORY.md hot layer.
    async fn add_to_hot(&self, entry: &TypedMemoryEntry) -> std::io::Result<()> {
        let mut guard = self.lock_hot_file().await?;
        let hot_meta = HotEntryMeta {
            key: entry.key.clone(),
            memory_type: entry.meta.memory_type,
            confidence: entry.meta.confidence,
            stability: entry.meta.stability,
            source: entry.meta.source,
            topic: entry.meta.topic.clone(),
            risk: entry.meta.risk,
            last_promoted: chrono::Utc::now().to_rfc3339(),
        };

        // Add to frontmatter
        guard.file.entries.push(hot_meta);

        // Add to body
        let bullet = format!("- **[{}]** {}\n", entry.key, entry.content.trim());
        guard.file.body.push_str(&bullet);

        guard.commit()
    }

    /// Find a hot entry by key in the parsed file.
    fn find_in_hot(&self, file: &MemoryFile, key: &str) -> Option<TypedMemoryEntry> {
        let meta = file.entries.iter().find(|e| e.key == key)?;
        self.hot_meta_to_entry(meta, &file.body)
    }

    /// Reconstruct a TypedMemoryEntry from a HotEntryMeta and the file body.
    fn hot_meta_to_entry(&self, meta: &HotEntryMeta, body: &str) -> Option<TypedMemoryEntry> {
        let content = extract_hot_entry_content(body, &meta.key);
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
        // Write to the DESTINATION (hot) FIRST. If this fails, the warm entry
        // is untouched — at worst we have a duplicate, never a loss (P0-2).
        if let Err(e) = self.add_to_hot(&entry).await {
            return Err(ReactError::from(e));
        }

        // Only after hot write succeeds, remove from warm.
        self.typed_store.delete_typed(WARM_NAMESPACE, key).await?;

        // Enforce budget (may demote other entries)
        self.enforce_hot_budget().await?;

        self.record_change(
            key,
            ChangeType::Promote,
            Some("warm"),
            Some("hot"),
            "warm→hot promotion (eligible)",
            "promote",
        )?;

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
        // Write to warm layer FIRST. If this fails, the hot entry is untouched
        // — at worst we have a duplicate, never a loss (P0-2).
        self.typed_store
            .put_typed(WARM_NAMESPACE, key, &entry.content, entry.meta.clone())
            .await?;

        // Only after warm write succeeds, remove from hot layer file — under
        // the lock so a concurrent writer cannot interleave (P0-3).
        let mut guard = self.lock_hot_file().await.map_err(ReactError::from)?;
        guard.file.entries.retain(|e| e.key != key);

        let pattern = format!("- **[{key}]**");
        guard.file.body = guard
            .file
            .body
            .lines()
            .filter(|line| !line.starts_with(&pattern))
            .collect::<Vec<_>>()
            .join("\n");

        guard.commit().map_err(ReactError::from)?;

        self.record_change(
            key,
            ChangeType::Demote,
            Some("hot"),
            Some("warm"),
            reason,
            "demote",
        )?;

        Ok(LayerChangeResult {
            key: key.to_string(),
            from_layer: MemoryLayer::Hot,
            to_layer: MemoryLayer::Warm,
            reason: reason.to_string(),
        })
    }

    /// Record a change in the audit log.
    fn record_change(
        &self,
        key: &str,
        change_type: ChangeType,
        from: Option<&str>,
        to: Option<&str>,
        reason: &str,
        trigger: &str,
    ) -> Result<()> {
        let mut builder = ChangeEntryBuilder::new(EntityType::Memory, key, change_type);

        if let Some(f) = from {
            builder = builder.before(serde_json::json!({ "layer": f }));
        }
        if let Some(t) = to {
            builder = builder.after(serde_json::json!({ "layer": t }));
        }
        builder = builder.reason(reason.to_string());
        builder = builder.trigger(trigger.to_string());

        let entry = builder.build(&*self.change_log);
        self.change_log.record(entry)
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
        .find(|line| line.starts_with(&pattern))
        .map(|line| line.trim_start_matches(&pattern).to_string())
        .unwrap_or_default()
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
    use echo_core::memory::types::MemorySource;

    impl InputTrustLevel {
        /// Derive the trust level from the memory source.
        pub fn from_source(source: MemorySource) -> Self {
            match source {
                MemorySource::ExplicitSave | MemorySource::UserCorrection => {
                    InputTrustLevel::Trusted
                }
                MemorySource::AutoExtracted | MemorySource::L3Promotion => {
                    InputTrustLevel::Assistant
                }
                MemorySource::ErrorResolution | MemorySource::RepeatedWorkflow => {
                    InputTrustLevel::Assistant
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::audit::ChangeEntry;
    use super::*;
    use echo_core::memory::types::{MemoryMeta, MemorySource, MemoryType};
    use echo_state::memory::store::InMemoryStore;

    /// A no-op ChangeLog for testing.
    struct NullChangeLog;
    impl ChangeLog for NullChangeLog {
        fn record(&self, _entry: ChangeEntry) -> Result<()> {
            Ok(())
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
                source: MemorySource::ExplicitSave,
                topic: "style".to_string(),
                risk: MemoryRisk::Low,
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
    fn test_read_hot_content_empty() {
        let manager = make_manager();
        assert!(manager.read_hot_content().is_empty());
    }

    #[tokio::test]
    async fn test_add_to_hot_under_budget() {
        let manager = make_manager();
        let entry = TypedMemoryEntry {
            key: "test_key".to_string(),
            content: "User prefers concise output.".to_string(),
            meta: MemoryMeta::new(
                MemoryType::UserPreference,
                MemorySource::ExplicitSave,
                "style",
            ),
            raw: echo_core::memory::store::StoreItem::new(
                vec!["agent".to_string(), "typed_memories".to_string()],
                "test_key".to_string(),
                serde_json::Value::Null,
            ),
        };

        manager.add_to_hot(&entry).await.expect("add to hot");

        let content = manager.read_hot_content();
        assert!(content.contains("**[test_key]**"));
        assert!(content.contains("User prefers concise output"));

        let meta = manager.read_hot_meta();
        assert_eq!(meta.len(), 1);
        assert_eq!(meta[0].key, "test_key");
    }

    #[test]
    fn test_demotion_score_ordering() {
        let high_quality = HotEntryMeta {
            key: "hq".to_string(),
            memory_type: MemoryType::UserPreference,
            confidence: 0.95,
            stability: 0.90,
            source: MemorySource::ExplicitSave,
            topic: "style".to_string(),
            risk: MemoryRisk::Low,
            last_promoted: "2026-06-15T10:00:00Z".to_string(),
        };

        let low_quality = HotEntryMeta {
            key: "lq".to_string(),
            memory_type: MemoryType::CommandPattern,
            confidence: 0.50,
            stability: 0.30,
            source: MemorySource::AutoExtracted,
            topic: "build".to_string(),
            risk: MemoryRisk::Low,
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
    async fn test_write_memory_and_consider_promotion() {
        let manager = make_manager();

        // High confidence, high stability → should be eligible for hot
        let meta = MemoryMeta::new(
            MemoryType::UserPreference,
            MemorySource::ExplicitSave,
            "style",
        )
        .with_confidence(0.95)
        .with_stability(0.90);

        let result = manager
            .write_memory("test_pref", "User prefers concise output", meta)
            .await
            .expect("write_memory");

        // ExplicitSave with high confidence/stability → should auto-promote
        assert!(result.is_some());
        let change = result.unwrap();
        assert_eq!(change.from_layer, MemoryLayer::Warm);
        assert_eq!(change.to_layer, MemoryLayer::Hot);

        // Verify it's in hot
        let content = manager.read_hot_content();
        assert!(content.contains("**[test_pref]**"));
    }

    #[tokio::test]
    async fn test_write_memory_not_eligible_for_hot() {
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
        assert!(manager.read_hot_content().is_empty());
    }

    #[tokio::test]
    async fn test_locate_in_warm() {
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

        let location = manager.locate("warm_key").await;
        assert!(location.is_some());
        let (layer, _) = location.unwrap();
        assert_eq!(layer, MemoryLayer::Warm);
    }

    #[tokio::test]
    async fn test_locate_not_found() {
        let manager = make_manager();
        let location = manager.locate("nonexistent").await;
        assert!(location.is_none());
    }

    #[tokio::test]
    async fn test_demote_hot_to_warm() {
        let manager = make_manager();

        let entry = TypedMemoryEntry {
            key: "demote_test".to_string(),
            content: "Test memory to demote".to_string(),
            meta: MemoryMeta::new(
                MemoryType::UserPreference,
                MemorySource::ExplicitSave,
                "style",
            )
            .with_confidence(0.95)
            .with_stability(0.90),
            raw: echo_core::memory::store::StoreItem::new(
                vec!["agent".to_string()],
                "demote_test".to_string(),
                serde_json::Value::Null,
            ),
        };

        manager.add_to_hot(&entry).await.expect("add to hot");
        assert!(manager.read_hot_content().contains("**[demote_test]**"));

        let result = manager
            .demote("demote_test", "test demotion")
            .await
            .expect("demote");
        assert_eq!(result.from_layer, MemoryLayer::Hot);
        assert_eq!(result.to_layer, MemoryLayer::Warm);

        // Should no longer be in hot
        assert!(!manager.read_hot_content().contains("**[demote_test]**"));

        // Should be in warm
        let location = manager.locate("demote_test").await;
        assert!(location.is_some());
        let (layer, _) = location.unwrap();
        assert_eq!(layer, MemoryLayer::Warm);
    }

    #[tokio::test]
    async fn test_search_layered_returns_hot_first() {
        let manager = make_manager();

        // Add to hot
        let hot_entry = TypedMemoryEntry {
            key: "hot_build".to_string(),
            content: "Hot build memory".to_string(),
            meta: MemoryMeta::new(
                MemoryType::DebuggingLesson,
                MemorySource::ExplicitSave,
                "build",
            )
            .with_confidence(0.95)
            .with_stability(0.90),
            raw: echo_core::memory::store::StoreItem::new(
                vec!["agent".to_string()],
                "hot_build".to_string(),
                serde_json::Value::Null,
            ),
        };
        manager.add_to_hot(&hot_entry).await.expect("add to hot");

        // Add to warm
        let warm_meta = MemoryMeta::new(
            MemoryType::ProjectFact,
            MemorySource::AutoExtracted,
            "build",
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
    async fn test_concurrent_add_to_hot_does_not_lose_entries() {
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
            meta: MemoryMeta::new(
                MemoryType::UserPreference,
                MemorySource::ExplicitSave,
                "topic",
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

        let content = manager.read_hot_content();
        assert!(
            content.contains("**[concurrent-a]**"),
            "concurrent-a lost (MEMORY.md TOCTOU regression)"
        );
        assert!(
            content.contains("**[concurrent-b]**"),
            "concurrent-b lost (MEMORY.md TOCTOU regression)"
        );
        assert_eq!(manager.read_hot_meta().len(), 2);
    }
}
