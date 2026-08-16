//! Change audit log — records all mutations to memories, skills, and rules.
//!
//! Every create, update, delete, promote, demote, and merge operation is
//! recorded as a `ChangeEntry` in a JSONL file. This enables:
//!
//! - **Auditing**: review what changed and why
//! - **Rollback**: undo any change by restoring the previous state
//! - **Trending**: detect patterns in evolution activity

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::Result;

/// Per-process counter to disambiguate change_ids that share the same nanosecond.
static CHANGE_COUNTER: AtomicU64 = AtomicU64::new(0);

// ── ChangeType ──────────────────────────────────────────────────────────

/// The kind of mutation that was performed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeType {
    /// A new entity was created.
    Create,
    /// An existing entity was updated.
    Update,
    /// An entity was deleted.
    Delete,
    /// An entity was promoted to a higher layer or status.
    Promote,
    /// An entity was demoted to a lower layer or status.
    Demote,
    /// Two or more entities were merged into one.
    Merge,
}

// ── EntityType ──────────────────────────────────────────────────────────

/// The type of entity that was changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityType {
    /// A memory entry.
    Memory,
    /// A skill (SKILL.md or skill candidate).
    Skill,
    /// A rule (AGENTS.md or instruction file).
    Rule,
}

// ── ChangeEntry ─────────────────────────────────────────────────────────

/// A single recorded change in the evolution audit log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChangeEntry {
    /// Unique identifier for this change.
    pub change_id: String,
    /// When the change occurred.
    #[serde(with = "crate::utils::time::local_rfc3339")]
    pub timestamp: DateTime<Utc>,
    /// What kind of entity was changed.
    pub entity_type: EntityType,
    /// The key/path of the entity that was changed.
    pub entity_key: String,
    /// The kind of mutation.
    pub change_type: ChangeType,
    /// The entity state before the change (if available).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<serde_json::Value>,
    /// The entity state after the change.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<serde_json::Value>,
    /// Human-readable reason for the change.
    pub reason: String,
    /// What triggered this change (e.g., "memory_reviewer", "user_command", "auto_trigger").
    pub trigger: String,
}

// ── ChangeLog trait ─────────────────────────────────────────────────────

/// Trait for recording and querying evolution changes.
pub trait ChangeLog: Send + Sync {
    /// Record a change entry.
    fn record(&self, entry: ChangeEntry) -> Result<()>;

    /// Atomically record an entry identified by its stable `change_id`.
    ///
    /// Replaying an identical entry succeeds without appending a second line.
    /// Reusing the ID for different content fails closed.
    fn record_idempotent(&self, entry: ChangeEntry) -> Result<ChangeRecordOutcome>;

    /// Query changes matching the given filters.
    fn query(&self, filter: &ChangeFilter) -> Result<Vec<ChangeEntry>>;

    /// Find the most recent change for a given entity.
    fn latest_for(&self, entity_type: EntityType, entity_key: &str) -> Result<Option<ChangeEntry>>;

    /// Get the total number of recorded changes.
    fn len(&self) -> usize;

    /// Whether the log is empty.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Result of an idempotent audit append.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeRecordOutcome {
    /// The entry was durably appended.
    Appended,
    /// An identical entry with the same stable ID was already durable.
    AlreadyRecorded,
}

// ── ChangeFilter ────────────────────────────────────────────────────────

/// Filter criteria for querying the change log.
#[derive(Debug, Clone, Default)]
pub struct ChangeFilter {
    /// Filter by entity type.
    pub entity_type: Option<EntityType>,
    /// Filter by change type.
    pub change_type: Option<ChangeType>,
    /// Filter by entity key prefix.
    pub entity_key_prefix: Option<String>,
    /// Only changes after this timestamp.
    pub after: Option<DateTime<Utc>>,
    /// Only changes before this timestamp.
    pub before: Option<DateTime<Utc>>,
    /// Maximum number of results.
    pub limit: Option<usize>,
}

impl ChangeFilter {
    /// Create an empty filter (matches everything).
    pub fn new() -> Self {
        Self::default()
    }

    /// Filter by entity type.
    pub fn with_entity_type(mut self, entity_type: EntityType) -> Self {
        self.entity_type = Some(entity_type);
        self
    }

    /// Filter by change type.
    pub fn with_change_type(mut self, change_type: ChangeType) -> Self {
        self.change_type = Some(change_type);
        self
    }

    /// Filter by entity key prefix.
    pub fn with_key_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.entity_key_prefix = Some(prefix.into());
        self
    }

    /// Limit the number of results.
    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Check if a change entry matches this filter.
    pub fn matches(&self, entry: &ChangeEntry) -> bool {
        if let Some(ref et) = self.entity_type
            && entry.entity_type != *et
        {
            return false;
        }
        if let Some(ref ct) = self.change_type
            && entry.change_type != *ct
        {
            return false;
        }
        if let Some(ref prefix) = self.entity_key_prefix
            && !entry.entity_key.starts_with(prefix)
        {
            return false;
        }
        if let Some(after) = self.after
            && entry.timestamp < after
        {
            return false;
        }
        if let Some(before) = self.before
            && entry.timestamp > before
        {
            return false;
        }
        true
    }
}

// ── JsonlChangeLog ──────────────────────────────────────────────────────

/// JSONL-file-backed change log. Each line is a `ChangeEntry` JSON object.
///
/// Thread-safe via internal locking. Appends are atomic (one line per write).
pub struct JsonlChangeLog {
    path: PathBuf,
    /// In-memory cache of entries for fast querying.
    entries: std::sync::Mutex<Vec<ChangeEntry>>,
}

impl JsonlChangeLog {
    /// Create a new JSONL change log at the given path.
    ///
    /// Loads and validates existing entries. The data file is created on the
    /// first durable record.
    pub fn new(path: PathBuf) -> Result<Self> {
        let log = Self {
            path,
            entries: std::sync::Mutex::new(Vec::new()),
        };
        let entries = log.with_disk_lock(|| Self::load_from_disk(&log.path))?;
        *log.entries
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = entries;
        Ok(log)
    }

    /// Load entries from the JSONL file.
    ///
    /// Every newline-terminated record must parse. Only a final record without
    /// a newline can be a crash-torn append: a valid record receives its
    /// missing newline, while an invalid tail is truncated to the last durable
    /// record. Interior corruption is never skipped.
    fn load_from_disk(path: &Path) -> std::io::Result<Vec<ChangeEntry>> {
        let content = match echo_core::utils::fs::read_existing(path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut entries = Vec::new();
        let mut change_ids = HashSet::new();
        let mut committed_len = 0_u64;

        for (line_index, segment) in content.split_inclusive(|byte| *byte == b'\n').enumerate() {
            let complete = segment.last().is_some_and(|byte| *byte == b'\n');
            let line = if complete {
                segment.strip_suffix(b"\n").unwrap_or(segment)
            } else {
                segment
            };
            match serde_json::from_slice::<ChangeEntry>(line) {
                Ok(entry) => {
                    if !change_ids.insert(entry.change_id.clone()) {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!(
                                "audit log {} repeats change_id {:?} at line {}",
                                path.display(),
                                entry.change_id,
                                line_index.saturating_add(1)
                            ),
                        ));
                    }
                    entries.push(entry);
                }
                Err(error) if !complete => {
                    tracing::warn!(
                        path = ?path,
                        error = %error,
                        tail_preview = %String::from_utf8_lossy(line).chars().take(120).collect::<String>(),
                        "audit log: truncating crash-torn final JSONL record"
                    );
                    echo_core::utils::fs::truncate_existing(
                        path,
                        committed_len,
                        echo_core::utils::fs::FileDurability::SyncData,
                    )?;
                    return Ok(entries);
                }
                Err(error) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!(
                            "audit log {} has corrupt complete record at line {}: {}; preview={:?}",
                            path.display(),
                            line_index.saturating_add(1),
                            error,
                            String::from_utf8_lossy(line)
                                .chars()
                                .take(120)
                                .collect::<String>()
                        ),
                    ));
                }
            }

            if complete {
                committed_len = committed_len
                    .checked_add(u64::try_from(segment.len()).map_err(|error| {
                        std::io::Error::other(format!("audit record length overflow: {error}"))
                    })?)
                    .ok_or_else(|| std::io::Error::other("audit log length overflow"))?;
            } else {
                echo_core::utils::fs::append_existing(
                    path,
                    b"\n",
                    echo_core::utils::fs::FileDurability::SyncData,
                )?;
            }
        }
        Ok(entries)
    }

    fn with_disk_lock<T>(
        &self,
        operation: impl FnOnce() -> std::io::Result<T>,
    ) -> std::io::Result<T> {
        if let Some(parent) = self
            .path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)?;
        }

        let lock_path = self.path.with_extension("jsonl.lock");
        let lock_file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)?;
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
                    std::thread::sleep(BACKOFF);
                }
                Err(error) => return Err(error),
            }
        }
        if !got_lock {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                format!("audit log lock unavailable after {MAX_ATTEMPTS} attempts"),
            ));
        }

        let result = operation();
        let unlock_result = lock_file.unlock();
        match (result, unlock_result) {
            (Err(error), _) | (Ok(_), Err(error)) => Err(error),
            (Ok(value), Ok(())) => Ok(value),
        }
    }

    fn create_log_file_if_missing(&self) -> std::io::Result<()> {
        match std::fs::symlink_metadata(&self.path) {
            Ok(_) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                echo_core::utils::fs::atomic_write(&self.path, b"")
            }
            Err(error) => Err(error),
        }
    }

    /// Append an entry to the JSONL file.
    ///
    /// Holds a cross-process advisory `flock` on a sidecar file and calls
    /// `sync_all` before returning, so:
    /// - concurrent appenders (this process or another) cannot interleave two
    ///   large JSON lines into a single corrupt line;
    /// - a power-loss crash cannot lose the just-appended line (the OS flushes
    ///   the durable write before we report success).
    ///
    /// `sync_all` per append is the durability/cost trade-off appropriate for
    /// an *audit* log whose entire purpose is a complete, durable record.
    fn record_idempotent_inner(&self, entry: ChangeEntry) -> Result<ChangeRecordOutcome> {
        if entry.change_id.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "audit change_id must not be empty",
            )
            .into());
        }

        let mut cached = self
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let (entries, outcome) = self.with_disk_lock(|| {
            let mut entries = Self::load_from_disk(&self.path)?;
            if let Some(existing) = entries
                .iter()
                .find(|existing| existing.change_id == entry.change_id)
            {
                if existing == &entry {
                    return Ok((entries, ChangeRecordOutcome::AlreadyRecorded));
                }
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    format!(
                        "audit change_id {:?} already identifies different content",
                        entry.change_id
                    ),
                ));
            }

            let mut line = serde_json::to_vec(&entry).map_err(std::io::Error::other)?;
            line.push(b'\n');
            self.create_log_file_if_missing()?;
            echo_core::utils::fs::append_existing(
                &self.path,
                &line,
                echo_core::utils::fs::FileDurability::SyncData,
            )?;
            entries.push(entry);
            Ok((entries, ChangeRecordOutcome::Appended))
        })?;
        *cached = entries;
        Ok(outcome)
    }
}

impl ChangeLog for JsonlChangeLog {
    fn record(&self, entry: ChangeEntry) -> Result<()> {
        self.record_idempotent_inner(entry)?;
        Ok(())
    }

    fn record_idempotent(&self, entry: ChangeEntry) -> Result<ChangeRecordOutcome> {
        self.record_idempotent_inner(entry)
    }

    fn query(&self, filter: &ChangeFilter) -> Result<Vec<ChangeEntry>> {
        let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let mut results: Vec<ChangeEntry> = entries
            .iter()
            .rev() // most recent first
            .filter(|e| filter.matches(e))
            .cloned()
            .collect();

        if let Some(limit) = filter.limit {
            results.truncate(limit);
        }
        Ok(results)
    }

    fn latest_for(&self, entity_type: EntityType, entity_key: &str) -> Result<Option<ChangeEntry>> {
        let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        Ok(entries
            .iter()
            .rev()
            .find(|e| e.entity_type == entity_type && e.entity_key == entity_key)
            .cloned())
    }

    fn len(&self) -> usize {
        self.entries.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}

// ── Helper for creating change entries ──────────────────────────────────

/// Builder helper for creating `ChangeEntry` instances.
pub struct ChangeEntryBuilder {
    entity_type: EntityType,
    entity_key: String,
    change_type: ChangeType,
    before: Option<serde_json::Value>,
    after: Option<serde_json::Value>,
    reason: String,
    trigger: String,
}

impl ChangeEntryBuilder {
    /// Start building a change entry.
    pub fn new(
        entity_type: EntityType,
        entity_key: impl Into<String>,
        change_type: ChangeType,
    ) -> Self {
        Self {
            entity_type,
            entity_key: entity_key.into(),
            change_type,
            before: None,
            after: None,
            reason: String::new(),
            trigger: String::new(),
        }
    }

    /// Set the before state.
    pub fn before(mut self, value: serde_json::Value) -> Self {
        self.before = Some(value);
        self
    }

    /// Set the after state.
    pub fn after(mut self, value: serde_json::Value) -> Self {
        self.after = Some(value);
        self
    }

    /// Set the reason.
    pub fn reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = reason.into();
        self
    }

    /// Set the trigger source.
    pub fn trigger(mut self, trigger: impl Into<String>) -> Self {
        self.trigger = trigger.into();
        self
    }

    /// Build the change entry, assigning a new ID and timestamp.
    ///
    /// Uses nanosecond-precision timestamp for the ID to avoid the TOCTOU race
    /// that existed with `log.len() + 1` (which was read outside the lock).
    pub fn build(self, _log: &dyn ChangeLog) -> ChangeEntry {
        let now = Utc::now();
        // timestamp_nanos_opt() returns None only on extreme overflow; fall back
        // to 0 so the id stays well-formed (uniqueness is still backed by `now`).
        let nanos = now.timestamp_nanos_opt().unwrap_or(0);
        ChangeEntry {
            change_id: format!(
                "chg_{:016x}_{}",
                nanos,
                CHANGE_COUNTER.fetch_add(1, Ordering::Relaxed)
            ),
            timestamp: now,
            entity_type: self.entity_type,
            entity_key: self.entity_key,
            change_type: self.change_type,
            before: self.before,
            after: self.after,
            reason: self.reason,
            trigger: self.trigger,
        }
    }

    /// Build with a caller-owned stable ID and timestamp.
    ///
    /// Recovery workflows persist these values and replay the same complete
    /// entry through [`ChangeLog::record_idempotent`].
    pub fn build_with(self, change_id: impl Into<String>, timestamp: DateTime<Utc>) -> ChangeEntry {
        ChangeEntry {
            change_id: change_id.into(),
            timestamp,
            entity_type: self.entity_type,
            entity_key: self.entity_key,
            change_type: self.change_type,
            before: self.before,
            after: self.after,
            reason: self.reason,
            trigger: self.trigger,
        }
    }
}

// ── Shared test/mock implementations ──────────────────────────────────

/// A no-op `ChangeLog` for tests and feature-disabled builds. Use this
/// instead of defining per-module copies (P1 — nullChangeLog dedup).
pub struct NullChangeLog;

impl ChangeLog for NullChangeLog {
    fn record(&self, _entry: ChangeEntry) -> Result<()> {
        Ok(())
    }
    fn record_idempotent(&self, _entry: ChangeEntry) -> Result<ChangeRecordOutcome> {
        Ok(ChangeRecordOutcome::AlreadyRecorded)
    }
    fn query(&self, _filter: &ChangeFilter) -> Result<Vec<ChangeEntry>> {
        Ok(vec![])
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

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_entry(change_id: &str, entity_key: &str) -> ChangeEntry {
        ChangeEntryBuilder::new(EntityType::Memory, entity_key, ChangeType::Create)
            .reason("Test")
            .trigger("test")
            .build_with(change_id, Utc::now())
    }

    #[test]
    fn test_record_and_query() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let log = JsonlChangeLog::new(dir.path().join("change-log.jsonl"))?;

        let entry = ChangeEntryBuilder::new(EntityType::Memory, "mem_001", ChangeType::Create)
            .reason("Auto-extracted observation")
            .trigger("auto_memory")
            .build_with("chg_000001", Utc::now());

        log.record(entry)?;
        assert_eq!(log.len(), 1);

        let results = log.query(
            &ChangeFilter::new()
                .with_entity_type(EntityType::Memory)
                .with_limit(10),
        )?;
        assert_eq!(results.len(), 1);
        assert_eq!(
            results.first().map(|entry| entry.entity_key.as_str()),
            Some("mem_001")
        );
        Ok(())
    }

    #[test]
    fn test_latest_for() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let log = JsonlChangeLog::new(dir.path().join("change-log.jsonl"))?;

        let entry1 = ChangeEntryBuilder::new(EntityType::Memory, "mem_001", ChangeType::Create)
            .reason("Created")
            .trigger("test")
            .build_with("chg_000001", Utc::now());

        let entry2 = ChangeEntryBuilder::new(EntityType::Memory, "mem_001", ChangeType::Update)
            .reason("Updated confidence")
            .trigger("memory_reviewer")
            .build_with("chg_000002", Utc::now());

        log.record(entry1)?;
        log.record(entry2)?;

        let latest = log.latest_for(EntityType::Memory, "mem_001")?;
        assert_eq!(
            latest.map(|entry| entry.change_type),
            Some(ChangeType::Update)
        );
        Ok(())
    }

    #[test]
    fn test_filter_by_change_type() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let log = JsonlChangeLog::new(dir.path().join("change-log.jsonl"))?;

        for (i, ct) in [ChangeType::Create, ChangeType::Update, ChangeType::Promote]
            .iter()
            .enumerate()
        {
            let entry = ChangeEntryBuilder::new(EntityType::Skill, format!("skill_{i}"), *ct)
                .reason("Test")
                .trigger("test")
                .build_with(format!("chg_{:06}", i + 1), Utc::now());
            log.record(entry)?;
        }

        let promotes = log.query(&ChangeFilter::new().with_change_type(ChangeType::Promote))?;
        assert_eq!(promotes.len(), 1);
        assert_eq!(
            promotes.first().map(|entry| entry.change_type),
            Some(ChangeType::Promote)
        );
        Ok(())
    }

    #[test]
    fn test_filter_by_key_prefix() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let log = JsonlChangeLog::new(dir.path().join("change-log.jsonl"))?;

        let entry1 = ChangeEntryBuilder::new(EntityType::Memory, "build/java8", ChangeType::Create)
            .reason("Test")
            .trigger("test")
            .build_with("chg_000001", Utc::now());
        let entry2 =
            ChangeEntryBuilder::new(EntityType::Memory, "style/concise", ChangeType::Create)
                .reason("Test")
                .trigger("test")
                .build_with("chg_000002", Utc::now());

        log.record(entry1)?;
        log.record(entry2)?;

        let build_entries = log.query(&ChangeFilter::new().with_key_prefix("build/"))?;
        assert_eq!(build_entries.len(), 1);
        assert_eq!(
            build_entries.first().map(|entry| entry.entity_key.as_str()),
            Some("build/java8")
        );
        Ok(())
    }

    #[test]
    fn interior_corruption_fails_closed_without_mutation()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("change-log.jsonl");
        let first = serde_json::to_vec(&sample_entry("promotion-1", "rule/one"))?;
        let second = serde_json::to_vec(&sample_entry("promotion-2", "rule/two"))?;
        let mut bytes = first;
        bytes.extend_from_slice(b"\n{not-json}\n");
        bytes.extend_from_slice(&second);
        bytes.push(b'\n');
        std::fs::write(&path, &bytes)?;

        assert!(JsonlChangeLog::new(path.clone()).is_err());
        assert_eq!(std::fs::read(path)?, bytes);
        Ok(())
    }

    #[test]
    fn torn_final_record_is_truncated_to_last_complete_entry()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("change-log.jsonl");
        let first = serde_json::to_vec(&sample_entry("promotion-1", "rule/one"))?;
        let mut durable = first;
        durable.push(b'\n');
        let mut bytes = durable.clone();
        bytes.extend_from_slice(b"{\"change_id\":\"promotion-2\"");
        std::fs::write(&path, bytes)?;

        let log = JsonlChangeLog::new(path.clone())?;
        assert_eq!(log.len(), 1);
        assert_eq!(std::fs::read(path)?, durable);
        Ok(())
    }

    #[test]
    fn valid_final_record_without_newline_is_preserved_and_terminated()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("change-log.jsonl");
        let mut expected = serde_json::to_vec(&sample_entry("promotion-1", "rule/one"))?;
        std::fs::write(&path, &expected)?;

        let log = JsonlChangeLog::new(path.clone())?;
        expected.push(b'\n');
        assert_eq!(log.len(), 1);
        assert_eq!(std::fs::read(path)?, expected);
        Ok(())
    }

    #[test]
    fn stable_change_id_is_idempotent_and_rejects_content_collision()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("change-log.jsonl");
        let log = JsonlChangeLog::new(path.clone())?;
        let entry = sample_entry("promotion-stable", "rule/one");

        assert_eq!(
            log.record_idempotent(entry.clone())?,
            ChangeRecordOutcome::Appended
        );
        assert_eq!(
            log.record_idempotent(entry.clone())?,
            ChangeRecordOutcome::AlreadyRecorded
        );
        assert_eq!(log.len(), 1);

        let mut conflicting = entry;
        conflicting.reason = "different promotion payload".to_string();
        assert!(log.record_idempotent(conflicting).is_err());
        assert_eq!(log.len(), 1);
        assert_eq!(JsonlChangeLog::new(path.clone())?.len(), 1);

        let durable = std::fs::read(&path)?;
        let mut duplicated = durable.clone();
        duplicated.extend_from_slice(&durable);
        std::fs::write(&path, duplicated)?;
        assert!(JsonlChangeLog::new(path).is_err());
        Ok(())
    }
}
