//! Cron task definition and persistence.

use chrono::{DateTime, Utc};
use cron::Schedule;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, Mutex as StdMutex, OnceLock, Weak};
use tokio::sync::Mutex;
use tracing::debug;

// ── CronTask ───────────────────────────────────────────────────────

/// A scheduled task that fires according to a cron expression.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CronTask {
    /// Unique identifier.
    pub id: String,
    /// Store-owned immutable identity for this definition generation.
    ///
    /// `CronTaskStore::add` always replaces this value so removing and adding
    /// an exact task clone cannot resurrect callbacks from the prior
    /// definition. Legacy records without this field use `created_at` until
    /// they are re-added. New unpersisted tasks leave this empty until
    /// `CronTaskStore::add` assigns the canonical value.
    #[serde(default)]
    pub definition_id: String,
    /// Human-readable name.
    pub name: String,
    /// 5-field cron expression: `min hour dom month dow`.
    pub cron_expr: String,
    /// The prompt or command to execute when fired.
    pub prompt: String,
    /// Whether the task is active.
    pub status: CronTaskStatus,
    /// Durable revision incremented by every status control operation.
    ///
    /// Scheduled occurrences capture this token so disable followed by enable
    /// cannot admit work selected before either control committed.
    #[serde(default)]
    pub control_revision: u64,
    /// ISO 8601 timestamp of the last execution.
    pub last_run_at: Option<String>,
    /// Truncated result from the last execution (first 500 chars).
    pub last_result: Option<String>,
    /// ISO 8601 timestamp of creation.
    pub created_at: String,
}

/// Whether a cron task is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CronTaskStatus {
    /// Task fires on schedule.
    Enabled,
    /// Task is paused.
    Disabled,
}

impl CronTask {
    /// Create a new enabled cron task.
    pub fn new(name: &str, cron_expr: &str, prompt: &str) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            definition_id: String::new(),
            name: name.to_string(),
            cron_expr: cron_expr.to_string(),
            prompt: prompt.to_string(),
            status: CronTaskStatus::Enabled,
            control_revision: 0,
            last_run_at: None,
            last_result: None,
            created_at: echo_core::utils::time::now_local().to_rfc3339(),
        }
    }

    /// Calculate the next fire time after now.
    ///
    /// Returns `None` if the cron expression is invalid or has no future occurrences.
    pub fn next_run(&self) -> Option<DateTime<Utc>> {
        self.next_run_after(&Utc::now())
    }

    /// Calculate the first fire time strictly after a caller-supplied boundary.
    pub fn next_run_after(&self, after: &DateTime<Utc>) -> Option<DateTime<Utc>> {
        // The cron crate expects 7-field expressions; pad with seconds and year
        let expr = if self.cron_expr.split_whitespace().count() == 5 {
            format!("0 {} *", self.cron_expr)
        } else {
            self.cron_expr.clone()
        };
        let schedule = Schedule::from_str(&expr).ok()?;
        schedule.after(after).next()
    }

    /// Validate the cron expression.
    pub fn validate_cron(&self) -> bool {
        let expr = if self.cron_expr.split_whitespace().count() == 5 {
            format!("0 {} *", self.cron_expr)
        } else {
            self.cron_expr.clone()
        };
        Schedule::from_str(&expr).is_ok()
    }

    pub(crate) fn same_definition(&self, other: &Self) -> bool {
        if self.id != other.id {
            return false;
        }
        if self.definition_id.is_empty() || other.definition_id.is_empty() {
            return self.definition_id.is_empty()
                && other.definition_id.is_empty()
                && self.created_at == other.created_at;
        }
        self.definition_id == other.definition_id
    }
}

// ── CronTaskStore ──────────────────────────────────────────────────

/// Persistent store for cron task definitions.
///
/// Supports two backends:
/// 1. **Store trait** (SQLite/InMemory) — recommended
/// 2. **File-based** — legacy JSON file fallback
///
/// [`super::SchedulerRunner`] derives its occurrence journal and checkpoint
/// paths from this store's definition path. Store-backed definitions must set
/// a stable path anchor with [`Self::with_path`] before constructing a runner.
#[derive(Clone)]
pub struct CronTaskStore {
    backend: Option<Arc<dyn echo_core::memory::Store>>,
    path: PathBuf,
    mutation_lock: Arc<Mutex<()>>,
    health: Arc<CronTaskStoreHealth>,
}

struct CronTaskStoreHealth {
    poison: StdMutex<Option<String>>,
    #[cfg(test)]
    file_after_replace_fault: Arc<std::sync::atomic::AtomicBool>,
    #[cfg(test)]
    file_reconcile_read_fault: Arc<std::sync::atomic::AtomicBool>,
}

impl CronTaskStoreHealth {
    fn new() -> Self {
        Self {
            poison: StdMutex::new(None),
            #[cfg(test)]
            file_after_replace_fault: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            #[cfg(test)]
            file_reconcile_read_fault: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }
}

const STORE_NAMESPACE: &[&str] = &["scheduler", "cron_tasks"];
const STORE_KEY: &str = "all_cron_tasks";

fn cron_store_mutation_lock() -> Arc<Mutex<()>> {
    static LOCK: OnceLock<Arc<Mutex<()>>> = OnceLock::new();
    Arc::clone(LOCK.get_or_init(|| Arc::new(Mutex::new(()))))
}

fn cron_store_health_registry() -> &'static StdMutex<HashMap<PathBuf, Weak<CronTaskStoreHealth>>> {
    static REGISTRY: OnceLock<StdMutex<HashMap<PathBuf, Weak<CronTaskStoreHealth>>>> =
        OnceLock::new();
    REGISTRY.get_or_init(|| StdMutex::new(HashMap::new()))
}

fn cron_store_health_key(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    };
    let Some(file_name) = absolute.file_name().map(|name| name.to_os_string()) else {
        return absolute;
    };
    let Some(parent) = absolute.parent() else {
        return absolute;
    };
    let mut cursor = parent.to_path_buf();
    let mut missing = Vec::new();
    loop {
        match std::fs::canonicalize(&cursor) {
            Ok(mut canonical) => {
                for component in missing.into_iter().rev() {
                    canonical.push(component);
                }
                canonical.push(&file_name);
                return canonical;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let Some(component) = cursor.file_name().map(|name| name.to_os_string()) else {
                    return absolute;
                };
                missing.push(component);
                let Some(next) = cursor.parent() else {
                    return absolute;
                };
                cursor = next.to_path_buf();
            }
            Err(_) => return absolute,
        }
    }
}

fn cron_store_health(path: &Path) -> Arc<CronTaskStoreHealth> {
    let key = cron_store_health_key(path);
    let mut registry = cron_store_health_registry()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    registry.retain(|_, health| health.strong_count() > 0);
    if let Some(health) = registry.get(&key).and_then(Weak::upgrade) {
        return health;
    }
    let health = Arc::new(CronTaskStoreHealth::new());
    registry.insert(key, Arc::downgrade(&health));
    health
}

impl CronTaskStore {
    /// Create a file-based store (default: `~/.echo-agent/scheduler/tasks.json`).
    pub fn new() -> Self {
        let path = default_task_path();
        Self {
            backend: None,
            health: cron_store_health(&path),
            path,
            mutation_lock: cron_store_mutation_lock(),
        }
    }

    /// Create a Store-backed store with automatic migration from file.
    pub async fn with_store(
        store: Arc<dyn echo_core::memory::Store>,
    ) -> echo_core::error::Result<Self> {
        let s = Self {
            backend: Some(store),
            path: PathBuf::new(),
            mutation_lock: cron_store_mutation_lock(),
            health: Arc::new(CronTaskStoreHealth::new()),
        };
        s.migrate_from_file().await?;
        Ok(s)
    }

    /// Set a custom file definition path and colocate occurrence durability files.
    ///
    /// For a Store-backed instance the Store remains the definition authority;
    /// this path only selects the sibling occurrence journal location.
    pub fn with_path(mut self, path: PathBuf) -> Self {
        self.health = cron_store_health(&path);
        self.path = path;
        self
    }

    #[cfg(test)]
    pub(crate) fn with_backend_path_for_test(
        backend: Arc<dyn echo_core::memory::Store>,
        path: PathBuf,
    ) -> Self {
        Self {
            backend: Some(backend),
            health: cron_store_health(&path),
            path,
            mutation_lock: cron_store_mutation_lock(),
        }
    }

    #[cfg(test)]
    pub(crate) fn inject_file_after_replace_fault(&self) {
        self.health
            .file_after_replace_fault
            .store(true, std::sync::atomic::Ordering::Release);
    }

    #[cfg(test)]
    pub(crate) fn inject_file_after_replace_reconcile_read_fault(&self) {
        self.health
            .file_after_replace_fault
            .store(true, std::sync::atomic::Ordering::Release);
        self.health
            .file_reconcile_read_fault
            .store(true, std::sync::atomic::Ordering::Release);
    }

    pub(crate) fn occurrence_paths(&self) -> echo_core::error::Result<(PathBuf, PathBuf)> {
        if self.backend.is_some() && self.path.as_os_str().is_empty() {
            return Err(echo_core::error::ReactError::Other(
                "Store-backed CronTaskStore requires with_path() to select a stable occurrence journal identity"
                    .to_string(),
            ));
        }
        let definition_path = if self.path.as_os_str().is_empty() {
            default_task_path()
        } else {
            self.path.clone()
        };
        Ok((
            path_with_suffix(&definition_path, ".occurrences.jsonl")?,
            path_with_suffix(&definition_path, ".occurrences.checkpoint.json")?,
        ))
    }

    /// Load all cron tasks.
    pub async fn load_all(&self) -> echo_core::error::Result<Vec<CronTask>> {
        self.check_authority()?;
        self.load_all_unchecked().await
    }

    async fn load_all_unchecked(&self) -> echo_core::error::Result<Vec<CronTask>> {
        if let Some(ref backend) = self.backend {
            let item = backend.get(STORE_NAMESPACE, STORE_KEY).await?;
            match item {
                Some(store_item) => {
                    // Value is stored as serde_json::Value — extract the string
                    let json_str = match &store_item.value {
                        serde_json::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    let tasks: Vec<CronTask> = serde_json::from_str(&json_str).map_err(|e| {
                        echo_core::error::ReactError::Other(format!(
                            "Failed to deserialize cron tasks: {e}"
                        ))
                    })?;
                    validate_task_ids(&tasks)?;
                    Ok(tasks)
                }
                None => Ok(vec![]),
            }
        } else {
            self.load_from_file()
        }
    }

    /// Save all cron tasks.
    async fn save_all_unlocked(&self, tasks: &[CronTask]) -> echo_core::error::Result<()> {
        self.check_authority()?;
        validate_task_ids(tasks)?;
        let json = serde_json::to_string_pretty(tasks).map_err(|e| {
            echo_core::error::ReactError::Other(format!("Failed to serialize cron tasks: {e}"))
        })?;

        if let Some(ref backend) = self.backend {
            if let Err(error) = backend
                .put(STORE_NAMESPACE, STORE_KEY, serde_json::Value::String(json))
                .await
            {
                if matches!(
                    &error,
                    echo_core::error::ReactError::Memory(memory)
                        if matches!(
                            memory.as_ref(),
                            echo_core::error::MemoryError::TransientNoCommit(_)
                        )
                ) {
                    return Err(error);
                }
                return Err(self.poison_authority(format!(
                    "Store backend mutation returned an uncertain result: {error}"
                )));
            }
        } else {
            self.save_to_file(&json)?;
        }
        Ok(())
    }

    /// Add a task as a fresh definition incarnation and persist it.
    ///
    /// The store replaces caller-supplied `definition_id` and returns the exact
    /// committed definition so callers can update projections without a second
    /// fallible backend read.
    pub async fn add(&self, mut task: CronTask) -> echo_core::error::Result<CronTask> {
        unique_id(&task.id)?;
        let _guard = self.mutation_lock.lock().await;
        let mut tasks = self.load_all().await?;
        if tasks.iter().any(|existing| existing.id == task.id) {
            return Err(echo_core::error::ReactError::Other(format!(
                "Cron task ID '{}' already exists",
                task.id
            )));
        }
        task.definition_id = uuid::Uuid::new_v4().to_string();
        tasks.push(task.clone());
        self.save_all_unlocked(&tasks).await?;
        Ok(task)
    }

    /// Remove a task by ID and persist. Returns true if found.
    pub async fn remove(&self, id: &str) -> echo_core::error::Result<bool> {
        Ok(self.remove_with_snapshot(id).await?.is_some())
    }

    /// Return the exact committed definition set to update an in-memory view.
    pub(crate) async fn remove_with_snapshot(
        &self,
        id: &str,
    ) -> echo_core::error::Result<Option<Vec<CronTask>>> {
        let id = unique_id(id)?;
        let _guard = self.mutation_lock.lock().await;
        let mut tasks = self.load_all().await?;
        let before = tasks.len();
        tasks.retain(|task| task.id != id);
        let removed = tasks.len() < before;
        if removed {
            self.save_all_unlocked(&tasks).await?;
        }
        Ok(removed.then_some(tasks))
    }

    /// Remove exactly one task by its complete ID and persist.
    pub async fn remove_exact(&self, id: &str) -> echo_core::error::Result<bool> {
        self.remove(id).await
    }

    /// Update the status of a task by ID and persist.
    pub async fn set_status(
        &self,
        id: &str,
        status: CronTaskStatus,
    ) -> echo_core::error::Result<bool> {
        Ok(self.set_status_with_snapshot(id, status).await?.is_some())
    }

    pub(crate) async fn set_status_with_snapshot(
        &self,
        id: &str,
        status: CronTaskStatus,
    ) -> echo_core::error::Result<Option<Vec<CronTask>>> {
        let id = unique_id(id)?;
        let _guard = self.mutation_lock.lock().await;
        let mut tasks = self.load_all().await?;
        let mut found = false;
        for task in &mut tasks {
            if task.id == id {
                let next_revision = task.control_revision.checked_add(1).ok_or_else(|| {
                    echo_core::error::ReactError::Other(format!(
                        "Cron task '{id}' control revision is exhausted"
                    ))
                })?;
                task.status = status;
                task.control_revision = next_revision;
                found = true;
                break;
            }
        }
        if found {
            self.save_all_unlocked(&tasks).await?;
        }
        Ok(found.then_some(tasks))
    }

    /// Update last_run info after a task fires.
    pub async fn update_last_run(&self, id: &str, result: &str) -> echo_core::error::Result<()> {
        let id = unique_id(id)?;
        let task = self.get(id).await?.ok_or_else(|| {
            echo_core::error::ReactError::Other(format!("Cron task '{id}' not found"))
        })?;
        self.update_last_run_for_task(&task, result)
            .await
            .map(|_| ())
    }

    /// Update last-run information only for the captured task definition.
    ///
    /// The store-owned `definition_id` prevents a callback from an older
    /// definition from updating a task that was removed and recreated with the
    /// same public ID. Legacy records use `created_at` as their identity.
    pub(crate) async fn update_last_run_for_task(
        &self,
        expected: &CronTask,
        result: &str,
    ) -> echo_core::error::Result<CronTask> {
        let id = unique_id(&expected.id)?;
        let _guard = self.mutation_lock.lock().await;
        let mut tasks = self.load_all().await?;
        let updated = tasks.iter_mut().find_map(|task| {
            if task.same_definition(expected) {
                task.last_run_at = Some(echo_core::utils::time::now_local().to_rfc3339());
                task.last_result = Some(result.chars().take(500).collect());
                Some(task.clone())
            } else {
                None
            }
        });
        let Some(updated) = updated else {
            return Err(echo_core::error::ReactError::Other(format!(
                "Cron task '{id}' definition is no longer current"
            )));
        };
        self.save_all_unlocked(&tasks).await?;
        Ok(updated)
    }

    /// Get a task by its unique ID.
    pub async fn get(&self, id: &str) -> echo_core::error::Result<Option<CronTask>> {
        let id = unique_id(id)?;
        let tasks = self.load_all().await?;
        Ok(tasks.into_iter().find(|task| task.id == id))
    }

    // ── Private helpers ────────────────────────────────────────────

    pub(crate) fn check_authority(&self) -> echo_core::error::Result<()> {
        let poison = self
            .health
            .poison
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(reason) = poison.as_ref() {
            return Err(echo_core::error::ReactError::Other(format!(
                "Cron task definition authority is poisoned: {reason}; recover the backend and reload before executing callbacks"
            )));
        }
        Ok(())
    }

    pub(crate) fn is_authority_poisoned(&self) -> bool {
        self.health
            .poison
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_some()
    }

    pub(crate) async fn recover_authority(&self) -> echo_core::error::Result<Vec<CronTask>> {
        let _guard = self.mutation_lock.lock().await;
        let tasks = self.load_all_unchecked().await?;
        *self
            .health
            .poison
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
        Ok(tasks)
    }

    fn poison_authority(&self, reason: String) -> echo_core::error::ReactError {
        *self
            .health
            .poison
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(reason.clone());
        echo_core::error::ReactError::Other(format!(
            "Cron task definition authority became ambiguous: {reason}; recover the backend and reload before executing callbacks"
        ))
    }

    fn load_from_file(&self) -> echo_core::error::Result<Vec<CronTask>> {
        if !self.path.exists() {
            return Ok(vec![]);
        }
        let content = std::fs::read_to_string(&self.path).map_err(|e| {
            echo_core::error::ReactError::Other(format!("Failed to read cron tasks file: {e}"))
        })?;
        if content.trim().is_empty() {
            return Ok(vec![]);
        }
        let tasks: Vec<CronTask> = serde_json::from_str(&content).map_err(|e| {
            echo_core::error::ReactError::Other(format!("Failed to parse cron tasks: {e}"))
        })?;
        validate_task_ids(&tasks)?;
        Ok(tasks)
    }

    fn save_to_file(&self, json: &str) -> echo_core::error::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                echo_core::error::ReactError::Other(format!(
                    "Failed to create scheduler directory: {e}"
                ))
            })?;
        }
        let prior_bytes = match std::fs::read(&self.path) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(echo_core::error::ReactError::Other(format!(
                    "Failed to read prior cron tasks file: {error}"
                )));
            }
        };
        let candidate = json.as_bytes();
        let write_result = echo_core::utils::fs::atomic_write(&self.path, candidate);
        #[cfg(test)]
        let write_result = match write_result {
            Ok(())
                if self
                    .health
                    .file_after_replace_fault
                    .swap(false, std::sync::atomic::Ordering::AcqRel) =>
            {
                Err(std::io::Error::other(
                    "injected parent-directory sync failure after visible replace",
                ))
            }
            result => result,
        };
        match write_result {
            Ok(()) => Ok(()),
            Err(write_error) => {
                #[cfg(test)]
                let observed = if self
                    .health
                    .file_reconcile_read_fault
                    .swap(false, std::sync::atomic::Ordering::AcqRel)
                {
                    Err(std::io::Error::other(
                        "injected reconciliation read failure after visible replace",
                    ))
                } else {
                    std::fs::read(&self.path)
                };
                #[cfg(not(test))]
                let observed = std::fs::read(&self.path);
                match observed {
                Ok(observed) if observed == candidate => {
                    tracing::warn!(
                        path = %self.path.display(),
                        error = %write_error,
                        "Cron task candidate is visible despite a degraded durability result"
                    );
                    Ok(())
                }
                Ok(observed) if prior_bytes.as_ref() == Some(&observed) => {
                    Err(echo_core::error::ReactError::Other(format!(
                        "Failed to write cron tasks file: {write_error}"
                    )))
                }
                Err(error)
                    if error.kind() == std::io::ErrorKind::NotFound && prior_bytes.is_none() =>
                {
                    Err(echo_core::error::ReactError::Other(format!(
                        "Failed to write cron tasks file: {write_error}"
                    )))
                }
                    Ok(observed) => Err(self.poison_authority(format!(
                        "persistence failed ({write_error}) and disk contains {} unreconciled bytes",
                        observed.len()
                    ))),
                    Err(error) => Err(self.poison_authority(format!(
                        "persistence failed ({write_error}) and disk reconciliation failed ({error})"
                    ))),
                }
            }
        }
    }

    async fn migrate_from_file(&self) -> echo_core::error::Result<()> {
        let _guard = self.mutation_lock.lock().await;
        let Some(backend) = self.backend.as_ref() else {
            return Ok(());
        };
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        let legacy_path = PathBuf::from(home).join(".echo-agent/scheduler/tasks.json");
        if !legacy_path.exists() {
            return Ok(());
        }

        // A present destination is already authoritative, even when it stores
        // an empty list.  Never replace it with a legacy snapshot.
        if backend.get(STORE_NAMESPACE, STORE_KEY).await?.is_some() {
            let _ = self.load_all().await?;
            debug!("Scheduler store already contains cron tasks; preserving legacy file");
            return Ok(());
        }

        debug!("Migrating cron tasks from file to Store backend");
        let content = std::fs::read_to_string(&legacy_path).map_err(|error| {
            echo_core::error::ReactError::Other(format!(
                "Failed to read legacy cron tasks: {error}"
            ))
        })?;
        if content.trim().is_empty() {
            return Ok(());
        }
        let tasks: Vec<CronTask> = serde_json::from_str(&content).map_err(|error| {
            echo_core::error::ReactError::Other(format!(
                "Failed to parse legacy cron tasks: {error}"
            ))
        })?;
        validate_task_ids(&tasks)?;
        self.save_all_unlocked(&tasks).await?;
        // Remove legacy file after successful migration
        let _ = std::fs::remove_file(&legacy_path);
        debug!(
            "Migrated {} cron tasks and removed legacy file",
            tasks.len()
        );
        Ok(())
    }
}

fn unique_id(id: &str) -> echo_core::error::Result<&str> {
    if id.trim().is_empty() {
        return Err(echo_core::error::ReactError::Other(
            "Cron task ID cannot be empty".into(),
        ));
    }
    Ok(id)
}

fn default_task_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".echo-agent/scheduler/tasks.json")
}

fn path_with_suffix(path: &std::path::Path, suffix: &str) -> echo_core::error::Result<PathBuf> {
    let file_name = path.file_name().ok_or_else(|| {
        echo_core::error::ReactError::Other(format!(
            "Cron task store path '{}' has no file name",
            path.display()
        ))
    })?;
    let mut suffixed = file_name.to_os_string();
    suffixed.push(suffix);
    Ok(path.with_file_name(suffixed))
}

fn validate_task_ids(tasks: &[CronTask]) -> echo_core::error::Result<()> {
    let mut seen = HashSet::with_capacity(tasks.len());
    for task in tasks {
        unique_id(&task.id)?;
        if !seen.insert(task.id.as_str()) {
            return Err(echo_core::error::ReactError::Other(format!(
                "Duplicate cron task ID '{}'",
                task.id
            )));
        }
    }
    Ok(())
}

impl Default for CronTaskStore {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cron_task_creation() {
        let task = CronTask::new("test", "*/5 * * * *", "Hello");
        assert_eq!(task.name, "test");
        assert!(task.definition_id.is_empty());
        assert_eq!(task.status, CronTaskStatus::Enabled);
        assert_eq!(task.control_revision, 0);
        assert!(task.validate_cron());
    }

    #[test]
    fn legacy_task_defaults_control_revision_to_zero() -> Result<(), String> {
        let task: CronTask = serde_json::from_value(serde_json::json!({
            "id": "legacy",
            "name": "legacy",
            "cron_expr": "*/5 * * * *",
            "prompt": "run",
            "status": "enabled",
            "last_run_at": null,
            "last_result": null,
            "created_at": "2026-01-01T00:00:00Z"
        }))
        .map_err(|error| error.to_string())?;
        assert!(task.definition_id.is_empty());
        assert_eq!(task.control_revision, 0);
        Ok(())
    }

    #[test]
    fn occurrence_paths_preserve_the_complete_definition_file_name() -> Result<(), String> {
        let root = std::env::temp_dir().join("echo-scheduler-path-identity");
        let json = CronTaskStore::new().with_path(root.join("tasks.json"));
        let toml = CronTaskStore::new().with_path(root.join("tasks.toml"));
        let (json_journal, json_checkpoint) =
            json.occurrence_paths().map_err(|error| error.to_string())?;
        let (toml_journal, toml_checkpoint) =
            toml.occurrence_paths().map_err(|error| error.to_string())?;

        assert_ne!(json_journal, toml_journal);
        assert_ne!(json_checkpoint, toml_checkpoint);
        assert_eq!(
            json_journal.file_name().and_then(|name| name.to_str()),
            Some("tasks.json.occurrences.jsonl")
        );
        Ok(())
    }

    #[test]
    fn definition_path_keys_shared_poison_authority() {
        let root = std::env::temp_dir().join(format!(
            "echo-scheduler-poison-identity-{}",
            uuid::Uuid::new_v4()
        ));
        let path_a = root.join("a.json");
        let path_b = root.join("b.json");
        let base = CronTaskStore::new();
        let store_a = base.clone().with_path(path_a.clone());
        let same_a = store_a.clone().with_path(path_a);
        let store_b = base.with_path(path_b);

        let _ = store_a.poison_authority("injected ambiguity".to_string());
        assert!(store_a.check_authority().is_err());
        assert!(same_a.check_authority().is_err());
        assert!(store_b.check_authority().is_ok());
    }

    #[test]
    fn independently_configured_clones_share_same_path_poison() {
        let root = std::env::temp_dir().join(format!(
            "echo-scheduler-shared-poison-{}",
            uuid::Uuid::new_v4()
        ));
        let path = root.join("tasks.json");
        let base = CronTaskStore::new();
        let first = base.clone().with_path(path.clone());
        let second = base.with_path(path);

        let _ = first.poison_authority("injected ambiguity".to_string());
        assert!(first.check_authority().is_err());
        assert!(second.check_authority().is_err());
    }

    #[test]
    fn health_key_is_stable_before_and_after_parent_creation() -> Result<(), String> {
        let root = std::env::temp_dir().join(format!(
            "echo-scheduler-health-key-{}",
            uuid::Uuid::new_v4()
        ));
        let parent = root.join("nested");
        let path = parent.join("tasks.json");
        std::fs::create_dir_all(&root).map_err(|error| error.to_string())?;
        let before = CronTaskStore::new().with_path(path.clone());
        std::fs::create_dir_all(&parent).map_err(|error| error.to_string())?;
        let after = CronTaskStore::new().with_path(path);

        let _ = before.poison_authority("injected ambiguity".to_string());
        assert!(after.check_authority().is_err());
        std::fs::remove_dir_all(root).map_err(|error| error.to_string())?;
        Ok(())
    }

    #[test]
    fn health_registry_prunes_dead_path_entries() {
        let root = std::env::temp_dir().join(format!(
            "echo-scheduler-health-prune-{}",
            uuid::Uuid::new_v4()
        ));
        let stale_path = root.join("stale.json");
        let stale_key = cron_store_health_key(&stale_path);
        let stale = cron_store_health(&stale_path);
        drop(stale);

        let live = cron_store_health(&root.join("live.json"));
        let registry = cron_store_health_registry()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        assert!(!registry.contains_key(&stale_key));
        drop(registry);
        drop(live);
    }

    #[tokio::test]
    async fn status_controls_increment_the_durable_revision() -> Result<(), String> {
        let temp = std::env::temp_dir().join(format!(
            "echo-scheduler-control-revision-{}",
            uuid::Uuid::new_v4()
        ));
        let store = CronTaskStore::new().with_path(temp.join("tasks.json"));
        let task = CronTask::new("controlled", "*/5 * * * *", "run");
        let task_id = task.id.clone();
        store.add(task).await.map_err(|error| error.to_string())?;
        assert!(
            store
                .set_status(&task_id, CronTaskStatus::Disabled)
                .await
                .map_err(|error| error.to_string())?
        );
        assert!(
            store
                .set_status(&task_id, CronTaskStatus::Enabled)
                .await
                .map_err(|error| error.to_string())?
        );
        let current = store
            .get(&task_id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "controlled task missing".to_string())?;
        assert_eq!(current.control_revision, 2);
        std::fs::remove_dir_all(&temp).map_err(|error| error.to_string())?;
        Ok(())
    }

    #[tokio::test]
    async fn add_replaces_an_exact_clones_definition_incarnation() -> Result<(), String> {
        let temp = std::env::temp_dir().join(format!(
            "echo-scheduler-definition-incarnation-{}",
            uuid::Uuid::new_v4()
        ));
        let store = CronTaskStore::new().with_path(temp.join("tasks.json"));
        let task = CronTask::new("incarnation", "*/5 * * * *", "run");
        let task_id = task.id.clone();
        store.add(task).await.map_err(|error| error.to_string())?;
        let first = store
            .get(&task_id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "first definition missing".to_string())?;
        assert!(!first.definition_id.is_empty());
        assert!(
            store
                .remove_exact(&task_id)
                .await
                .map_err(|error| error.to_string())?
        );
        store
            .add(first.clone())
            .await
            .map_err(|error| error.to_string())?;
        let second = store
            .get(&task_id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "second definition missing".to_string())?;
        assert_ne!(first.definition_id, second.definition_id);
        assert!(!first.same_definition(&second));
        std::fs::remove_dir_all(&temp).map_err(|error| error.to_string())?;
        Ok(())
    }

    #[test]
    fn test_cron_task_next_run() {
        let task = CronTask::new("test", "0 12 * * *", "Noon task");
        let next = task.next_run();
        assert!(next.is_some());
    }

    #[test]
    fn test_cron_task_invalid_expr() {
        let task = CronTask::new("bad", "not a cron", "test");
        assert!(!task.validate_cron());
        assert!(task.next_run().is_none());
    }

    #[tokio::test]
    async fn remove_exact_does_not_remove_tasks_with_the_same_prefix() -> Result<(), String> {
        let temp = std::env::temp_dir().join(format!(
            "echo-scheduler-remove-exact-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&temp).map_err(|error| error.to_string())?;
        let store = CronTaskStore::new().with_path(temp.join("cron-tasks.json"));
        let mut exact = CronTask::new("exact", "*/5 * * * *", "exact");
        exact.id = "plugin-monitor".to_string();
        let mut prefixed = CronTask::new("prefixed", "*/5 * * * *", "prefixed");
        prefixed.id = "plugin-monitor-longer".to_string();
        store.add(exact).await.map_err(|error| error.to_string())?;
        store
            .add(prefixed)
            .await
            .map_err(|error| error.to_string())?;

        assert!(
            store
                .remove_exact("plugin-monitor")
                .await
                .map_err(|error| error.to_string())?
        );
        let remaining = store.load_all().await.map_err(|error| error.to_string())?;
        assert_eq!(remaining.len(), 1);
        assert_eq!(
            remaining.first().map(|task| task.id.as_str()),
            Some("plugin-monitor-longer")
        );
        std::fs::remove_dir_all(&temp).map_err(|error| error.to_string())?;
        Ok(())
    }

    #[tokio::test]
    async fn independent_file_store_instances_do_not_lose_concurrent_adds() -> Result<(), String> {
        let temp = std::env::temp_dir().join(format!(
            "echo-scheduler-concurrent-add-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&temp).map_err(|error| error.to_string())?;
        let path = temp.join("cron-tasks.json");
        let first = CronTaskStore::new().with_path(path.clone());
        let second = CronTaskStore::new().with_path(path);

        let (first_result, second_result) = tokio::join!(
            first.add(CronTask::new("first", "*/5 * * * *", "first")),
            second.add(CronTask::new("second", "*/5 * * * *", "second")),
        );
        first_result.map_err(|error| error.to_string())?;
        second_result.map_err(|error| error.to_string())?;

        let tasks = first.load_all().await.map_err(|error| error.to_string())?;
        assert_eq!(tasks.len(), 2);
        assert!(tasks.iter().any(|task| task.name == "first"));
        assert!(tasks.iter().any(|task| task.name == "second"));
        std::fs::remove_dir_all(&temp).map_err(|error| error.to_string())?;
        Ok(())
    }

    #[tokio::test]
    async fn add_rejects_duplicate_task_ids() -> Result<(), String> {
        let temp = std::env::temp_dir().join(format!(
            "echo-scheduler-duplicate-id-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&temp).map_err(|error| error.to_string())?;
        let store = CronTaskStore::new().with_path(temp.join("cron-tasks.json"));
        let first = CronTask::new("first", "*/5 * * * *", "first");
        let mut duplicate = CronTask::new("second", "*/5 * * * *", "second");
        duplicate.id = first.id.clone();
        store.add(first).await.map_err(|error| error.to_string())?;
        assert!(store.add(duplicate).await.is_err());
        assert_eq!(
            store
                .load_all()
                .await
                .map_err(|error| error.to_string())?
                .len(),
            1
        );
        std::fs::remove_dir_all(&temp).map_err(|error| error.to_string())?;
        Ok(())
    }

    #[tokio::test]
    async fn load_rejects_duplicate_task_ids_in_persisted_file() -> Result<(), String> {
        let temp = std::env::temp_dir().join(format!(
            "echo-scheduler-duplicate-file-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&temp).map_err(|error| error.to_string())?;
        let path = temp.join("cron-tasks.json");
        let mut first = CronTask::new("first", "*/5 * * * *", "first");
        let mut duplicate = CronTask::new("second", "*/5 * * * *", "second");
        duplicate.id = first.id.clone();
        first.created_at = "2026-01-01T00:00:00Z".to_string();
        duplicate.created_at = "2026-01-01T00:00:01Z".to_string();
        let payload =
            serde_json::to_string(&vec![first, duplicate]).map_err(|error| error.to_string())?;
        std::fs::write(&path, payload).map_err(|error| error.to_string())?;
        let store = CronTaskStore::new().with_path(path);
        assert!(store.load_all().await.is_err());
        std::fs::remove_dir_all(&temp).map_err(|error| error.to_string())?;
        Ok(())
    }
}
