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
use std::path::PathBuf;

use crate::error::Result;

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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangeEntry {
    /// Unique identifier for this change.
    pub change_id: String,
    /// When the change occurred.
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
    /// Creates the file if it doesn't exist; loads existing entries if it does.
    pub fn new(path: PathBuf) -> Self {
        let entries = Self::load_from_disk(&path).unwrap_or_default();
        Self {
            path,
            entries: std::sync::Mutex::new(entries),
        }
    }

    /// Load entries from the JSONL file.
    fn load_from_disk(path: &PathBuf) -> std::io::Result<Vec<ChangeEntry>> {
        if !path.exists() {
            return Ok(Vec::new());
        }
        let content = std::fs::read_to_string(path)?;
        let mut entries = Vec::new();
        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(entry) = serde_json::from_str::<ChangeEntry>(line) {
                entries.push(entry);
            }
        }
        Ok(entries)
    }

    /// Append an entry to the JSONL file.
    fn append_to_disk(&self, entry: &ChangeEntry) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let line = serde_json::to_string(entry)?;
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        writeln!(file, "{line}")?;
        Ok(())
    }
}

impl ChangeLog for JsonlChangeLog {
    fn record(&self, entry: ChangeEntry) -> Result<()> {
        self.append_to_disk(&entry)?;
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(entry);
        Ok(())
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
    pub fn build(self, log: &dyn ChangeLog) -> ChangeEntry {
        ChangeEntry {
            change_id: format!("chg_{:06}", log.len() + 1),
            timestamp: Utc::now(),
            entity_type: self.entity_type,
            entity_key: self.entity_key,
            change_type: self.change_type,
            before: self.before,
            after: self.after,
            reason: self.reason,
            trigger: self.trigger,
        }
    }

    /// Build with explicit ID and timestamp (for testing).
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

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_log() -> JsonlChangeLog {
        let dir =
            std::env::temp_dir().join(format!("echo_changelog_test_{}", uuid::Uuid::new_v4()));
        let path = dir.join("change-log.jsonl");
        JsonlChangeLog::new(path)
    }

    #[test]
    fn test_record_and_query() {
        let log = temp_log();

        let entry = ChangeEntryBuilder::new(EntityType::Memory, "mem_001", ChangeType::Create)
            .reason("Auto-extracted observation")
            .trigger("auto_memory")
            .build_with("chg_000001", Utc::now());

        log.record(entry).unwrap();
        assert_eq!(log.len(), 1);

        let results = log
            .query(
                &ChangeFilter::new()
                    .with_entity_type(EntityType::Memory)
                    .with_limit(10),
            )
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].entity_key, "mem_001");
    }

    #[test]
    fn test_latest_for() {
        let log = temp_log();

        let entry1 = ChangeEntryBuilder::new(EntityType::Memory, "mem_001", ChangeType::Create)
            .reason("Created")
            .trigger("test")
            .build_with("chg_000001", Utc::now());

        let entry2 = ChangeEntryBuilder::new(EntityType::Memory, "mem_001", ChangeType::Update)
            .reason("Updated confidence")
            .trigger("memory_reviewer")
            .build_with("chg_000002", Utc::now());

        log.record(entry1).unwrap();
        log.record(entry2).unwrap();

        let latest = log.latest_for(EntityType::Memory, "mem_001").unwrap();
        assert!(latest.is_some());
        assert_eq!(latest.unwrap().change_type, ChangeType::Update);
    }

    #[test]
    fn test_filter_by_change_type() {
        let log = temp_log();

        for (i, ct) in [ChangeType::Create, ChangeType::Update, ChangeType::Promote]
            .iter()
            .enumerate()
        {
            let entry = ChangeEntryBuilder::new(EntityType::Skill, format!("skill_{i}"), *ct)
                .reason("Test")
                .trigger("test")
                .build_with(format!("chg_{:06}", i + 1), Utc::now());
            log.record(entry).unwrap();
        }

        let promotes = log
            .query(&ChangeFilter::new().with_change_type(ChangeType::Promote))
            .unwrap();
        assert_eq!(promotes.len(), 1);
        assert_eq!(promotes[0].change_type, ChangeType::Promote);
    }

    #[test]
    fn test_filter_by_key_prefix() {
        let log = temp_log();

        let entry1 = ChangeEntryBuilder::new(EntityType::Memory, "build/java8", ChangeType::Create)
            .reason("Test")
            .trigger("test")
            .build_with("chg_000001", Utc::now());
        let entry2 =
            ChangeEntryBuilder::new(EntityType::Memory, "style/concise", ChangeType::Create)
                .reason("Test")
                .trigger("test")
                .build_with("chg_000002", Utc::now());

        log.record(entry1).unwrap();
        log.record(entry2).unwrap();

        let build_entries = log
            .query(&ChangeFilter::new().with_key_prefix("build/"))
            .unwrap();
        assert_eq!(build_entries.len(), 1);
        assert_eq!(build_entries[0].entity_key, "build/java8");
    }
}
