//! Curator — skill lifecycle management.
//!
//! Manages the full skill lifecycle: `Candidate → Draft → Active → Stale → Deprecated → Archived`.
//! Only operates on agent-created skills (never bundled/external).
//! Never auto-deletes, only archives.
//!
//! # Lifecycle States
//!
//! | State | Meaning | Auto-transition |
//! |-------|---------|-----------------|
//! | `Candidate` | Pattern discovered from memory, not yet formalized | → Draft (after review) |
//! | `Draft` | SKILL.md created but not activated | → Active (after review/usage) |
//! | `Active` | In use, appears in skill catalog | → Stale (after inactivity) |
//! | `Stale` | Not used recently, may be outdated | → Deprecated (after longer inactivity) |
//! | `Deprecated` | Replaced or known outdated, still accessible | → Archived (after even longer inactivity) |
//! | `Archived` | Removed from all paths, kept for reference | Terminal |
//!
//! Inspired by Hermes Agent's curator system.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::error::Result;
use fs2::FileExt;

// ── SkillLifecycle ─────────────────────────────────────────────────

/// Lifecycle state of a skill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillLifecycle {
    /// Pattern discovered from memory, not yet formalized. No SKILL.md file yet.
    Candidate,
    /// SKILL.md created with minimal content, not yet injected into catalog.
    Draft,
    /// Actively used and relevant. Appears in skill catalog.
    Active,
    /// Not used recently, may be outdated.
    Stale,
    /// Replaced by another skill or known to be outdated. Still accessible but not in catalog.
    Deprecated,
    /// No longer relevant, kept for reference only.
    Archived,
}

// ── SkillMeta ──────────────────────────────────────────────────────

/// Metadata tracked by the curator for each skill.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillMeta {
    /// Skill name.
    pub name: String,
    /// Concrete `SKILL.md` path used as the runtime loading identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    /// Current lifecycle state.
    pub lifecycle: SkillLifecycle,
    /// When the skill was first created.
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// When the skill was last used/referenced.
    pub last_used_at: chrono::DateTime<chrono::Utc>,
    /// When the skill was last modified.
    pub last_modified_at: chrono::DateTime<chrono::Utc>,
    /// Whether this skill is pinned (exempt from auto-transitions).
    pub pinned: bool,
    /// Whether this skill is agent-created (vs bundled/external).
    pub agent_created: bool,
    /// If deprecated/archived, which skill supersedes this one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<String>,
}

// ── CuratorConfig ──────────────────────────────────────────────────

/// Configuration for the curator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CuratorConfig {
    /// Days of inactivity before a skill becomes stale.
    pub stale_days: u64,
    /// Days of inactivity before a skill becomes deprecated.
    pub deprecate_days: u64,
    /// Days of inactivity before a skill is archived.
    pub archive_days: u64,
    /// Whether the curator is enabled.
    pub enabled: bool,
}

impl Default for CuratorConfig {
    fn default() -> Self {
        Self {
            stale_days: 30,
            deprecate_days: 60,
            archive_days: 90,
            enabled: true,
        }
    }
}

// ── CuratorState ───────────────────────────────────────────────────

/// Persisted state of the curator.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CuratorState {
    /// Metadata for each tracked skill.
    pub skills: HashMap<String, SkillMeta>,
    /// When the curator last ran.
    pub last_run_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl CuratorState {
    /// Load state from a JSON file.
    ///
    /// - Missing file → returns default (fresh start). This is the only case
    ///   treated as "empty".
    /// - Present but unparseable / unreadable → also returns default, but logs
    ///   a `warn!` so silent corruption does not get overwritten by the next
    ///   save (which would destroy potentially-recoverable data). The caller
    ///   (a curator method) will proceed and rewrite, but at least the loss is
    ///   observable in logs.
    pub fn try_load(path: &PathBuf) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(data) => serde_json::from_str(&data).map_err(Into::into),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// Save state to a JSON file atomically.
    ///
    /// Writes to a temp sibling then `rename`s over the target. `rename` is
    /// atomic on POSIX, so a crash mid-write never leaves a truncated or
    /// partially-written state file — readers either see the old or the new
    /// file in full, never a torn mix.
    pub fn save(&self, path: &PathBuf) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_string_pretty(self)?;
        let tmp_path = path.with_extension("json.tmp");
        {
            let mut file = std::fs::File::create(&tmp_path)?;
            use std::io::Write;
            file.write_all(data.as_bytes())?;
            // fsync the data before rename so a crash after rename cannot
            // expose an empty file.
            file.sync_all()?;
        }
        std::fs::rename(&tmp_path, path)?;
        Ok(())
    }
}

// ── Curator ────────────────────────────────────────────────────────

/// Skill lifecycle manager.
///
/// Manages the transition of skills through lifecycle states:
/// - Candidate → Draft (manual promotion)
/// - Draft → Active (manual promotion or after N successful uses)
/// - Active → Stale (after `stale_days` of inactivity)
/// - Stale → Deprecated (after `deprecate_days` of inactivity)
/// - Deprecated → Archived (after `archive_days` of inactivity)
///
/// Only operates on agent-created skills. Pinned skills are exempt.
/// Skill lifecycle curator backed by a JSON state file.
///
/// Read-modify-write operations are serialized with an advisory file lock shared
/// by all `Curator` instances that use the same state path.
pub struct Curator {
    config: CuratorConfig,
    state_path: PathBuf,
}

// P2-9: 需要 Clone 以便在 spawn_blocking 闭包里使用 (touch_skill 是同步阻塞,
// 移到阻塞线程池)。Curator 是无状态句柄 (touch_skill 内部 flock), Clone 成本极低。
impl Clone for Curator {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            state_path: self.state_path.clone(),
        }
    }
}

impl Curator {
    /// Create a new curator with the given config and state file path.
    pub fn new(config: CuratorConfig, state_path: impl Into<PathBuf>) -> Self {
        Self {
            config,
            state_path: state_path.into(),
        }
    }

    /// Load the current state under an advisory file lock.
    ///
    /// Holds an exclusive `flock` on the sidecar lock file for the duration of
    /// the read only. For read-modify-write that must be atomic, use
    /// `with_locked_state` instead — that holds the lock across load, mutate,
    /// and save in a single critical section.
    ///
    /// The lock is an OS advisory lock (`fs2::FileExt::try_lock_exclusive`) on a
    /// sidecar file. It is **kill-safe**: if this process dies (kill -9, panic,
    /// power loss), the OS closes the fd and the lock is released automatically.
    /// This is the critical difference from the previous `create_new` sidecar
    /// design, whose lock file survived a crash and deadlocked every future
    /// locker forever (P0 — Curator TOCTOU).
    pub fn load_state(&self) -> Result<CuratorState> {
        let _guard = self.acquire_lock()?;
        CuratorState::try_load(&self.state_path)
    }

    /// Save the state under an advisory file lock.
    ///
    /// Holds an exclusive lock for the duration of the write only. Callers that
    /// need load-mutate-save atomicity must use `with_locked_state`.
    pub fn save_state(&self, state: &CuratorState) -> Result<()> {
        let _guard = self.acquire_lock()?;
        state.save(&self.state_path)
    }

    /// Atomically read-modify-write the curator state under a single held lock.
    ///
    /// The closure receives the freshly-loaded state and may mutate it. On
    /// success, the state is saved back while the lock is still held; on
    /// error, the in-memory change is discarded and the on-disk file is left
    /// untouched. This closes the TOCTOU window where two `Curator` instances
    /// interleaved load → mutate → save and clobbered each other.
    ///
    /// The closure returns `(T, bool)`: the value to return to the caller,
    /// plus a `dirty` flag. When `dirty` is false the save is skipped
    /// (read-only path) — this avoids a needless rewrite on every call.
    fn with_locked_state<T, F>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&mut CuratorState) -> Result<(T, bool)>,
    {
        let _guard = self.acquire_lock()?;
        let mut state = CuratorState::try_load(&self.state_path)?;
        let (result, dirty) = f(&mut state)?;
        if dirty {
            state.save(&self.state_path)?;
        }
        Ok(result)
    }

    /// Acquire an exclusive advisory lock on a sidecar `.lock` file.
    ///
    /// Retries non-blockingly for a bounded period. Returns a guard whose `Drop`
    /// releases the lock. The sidecar file is intentionally persistent: deleting
    /// it after unlock could let another process create a different inode and
    /// bypass an existing lock.
    fn acquire_lock(&self) -> Result<CuratorLockGuard> {
        let lock_path = self.state_path.with_extension("json.lock");
        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Ensure the sidecar file exists (does not fail if already present).
        // `truncate(false)` keeps any existing content (irrelevant for flock,
        // but avoids clobbering a file another process just created).
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)?;

        const MAX_ATTEMPTS: u32 = 25;
        const BACKOFF: std::time::Duration = std::time::Duration::from_millis(40);

        let mut last_err = None;
        for _ in 0..MAX_ATTEMPTS {
            match file.try_lock_exclusive() {
                Ok(()) => {
                    return Ok(CuratorLockGuard { file: Some(file) });
                }
                Err(e) => {
                    last_err = Some(e);
                    std::thread::sleep(BACKOFF);
                }
            }
        }

        Err(crate::error::ReactError::Other(format!(
            "curator: could not acquire lock at {} after {} attempts: {}",
            lock_path.display(),
            MAX_ATTEMPTS,
            last_err
                .map(|error| error.to_string())
                .unwrap_or_else(|| "unknown lock error".to_string())
        )))
    }

    /// Register a new skill or update an existing one's last-used timestamp.
    pub fn touch_skill(&self, name: &str, agent_created: bool) -> Result<()> {
        self.touch_skill_at(name, None, agent_created)
    }

    /// Register or touch a skill while binding its concrete `SKILL.md` path.
    pub fn touch_skill_at(
        &self,
        name: &str,
        path: Option<&std::path::Path>,
        agent_created: bool,
    ) -> Result<()> {
        let normalized_path = path.map(normalize_skill_path);
        self.with_locked_state(|state| {
            let now = chrono::Utc::now();
            if let Some(meta) = state.skills.get_mut(name) {
                meta.last_used_at = now;
                if normalized_path.is_some() {
                    meta.path = normalized_path.clone();
                    meta.last_modified_at = now;
                }
            } else {
                state.skills.insert(
                    name.to_string(),
                    SkillMeta {
                        name: name.to_string(),
                        path: normalized_path.clone(),
                        lifecycle: SkillLifecycle::Active,
                        created_at: now,
                        last_used_at: now,
                        last_modified_at: now,
                        pinned: false,
                        agent_created,
                        superseded_by: None,
                    },
                );
            }
            Ok(((), true))
        })
    }

    /// Register a new skill candidate (discovered from memory patterns).
    pub fn register_candidate(&self, name: &str) -> Result<()> {
        self.register_candidate_at(name, None)
    }

    /// Register a candidate and optionally bind a future draft path.
    pub fn register_candidate_at(&self, name: &str, path: Option<&std::path::Path>) -> Result<()> {
        let normalized_path = path.map(normalize_skill_path);
        self.with_locked_state(|state| {
            if let Some(meta) = state.skills.get_mut(name) {
                if normalized_path.is_some() && meta.path != normalized_path {
                    meta.path = normalized_path.clone();
                    meta.last_modified_at = chrono::Utc::now();
                    return Ok(((), true));
                }
                return Ok(((), false));
            }
            let now = chrono::Utc::now();
            state.skills.insert(
                name.to_string(),
                SkillMeta {
                    name: name.to_string(),
                    path: normalized_path.clone(),
                    lifecycle: SkillLifecycle::Candidate,
                    created_at: now,
                    last_used_at: now,
                    last_modified_at: now,
                    pinned: false,
                    agent_created: true,
                    superseded_by: None,
                },
            );
            Ok(((), true))
        })
    }

    pub(crate) fn remove_candidate(&self, name: &str) -> Result<bool> {
        self.with_locked_state(|state| {
            let removable = state
                .skills
                .get(name)
                .is_some_and(|meta| meta.lifecycle == SkillLifecycle::Candidate);
            if removable {
                state.skills.remove(name);
            }
            Ok((removable, removable))
        })
    }

    pub(crate) fn revert_draft_to_candidate(&self, name: &str) -> Result<bool> {
        self.with_locked_state(|state| {
            let Some(meta) = state.skills.get_mut(name) else {
                return Ok((false, false));
            };
            if meta.lifecycle != SkillLifecycle::Draft {
                return Ok((false, false));
            }
            meta.lifecycle = SkillLifecycle::Candidate;
            meta.path = None;
            meta.last_modified_at = chrono::Utc::now();
            Ok((true, true))
        })
    }

    pub(crate) fn skill(&self, name: &str) -> Result<Option<SkillMeta>> {
        let _guard = self.acquire_lock()?;
        Ok(CuratorState::try_load(&self.state_path)?
            .skills
            .get(name)
            .cloned())
    }

    pub(crate) fn restore_skill(&self, name: &str, previous: Option<SkillMeta>) -> Result<()> {
        self.with_locked_state(|state| {
            match previous {
                Some(meta) => {
                    state.skills.insert(name.to_string(), meta);
                }
                None => {
                    state.skills.remove(name);
                }
            }
            Ok(((), true))
        })
    }

    /// Promote a Candidate skill to Draft.
    pub fn promote_to_draft(&self, name: &str) -> Result<bool> {
        self.promote_to_draft_at(name, None)
    }

    /// Promote a Candidate to Draft and bind the generated draft path.
    pub fn promote_to_draft_at(&self, name: &str, path: Option<&std::path::Path>) -> Result<bool> {
        let normalized_path = path.map(normalize_skill_path);
        self.with_locked_state(|state| {
            let now = chrono::Utc::now();
            if let Some(meta) = state.skills.get_mut(name)
                && meta.lifecycle == SkillLifecycle::Candidate
            {
                meta.lifecycle = SkillLifecycle::Draft;
                if normalized_path.is_some() {
                    meta.path = normalized_path.clone();
                }
                meta.last_modified_at = now;
                return Ok((true, true));
            }
            Ok((false, false))
        })
    }

    /// Promote a Draft skill to Active.
    pub fn promote_to_active(&self, name: &str) -> Result<bool> {
        self.promote_to_active_at(name, None)
    }

    /// Promote a Draft to Active and bind the authoritative runtime path.
    pub fn promote_to_active_at(&self, name: &str, path: Option<&std::path::Path>) -> Result<bool> {
        let normalized_path = path.map(normalize_skill_path);
        self.with_locked_state(|state| {
            let now = chrono::Utc::now();
            if let Some(meta) = state.skills.get_mut(name)
                && meta.lifecycle == SkillLifecycle::Draft
            {
                meta.lifecycle = SkillLifecycle::Active;
                if normalized_path.is_some() {
                    meta.path = normalized_path.clone();
                }
                meta.last_modified_at = now;
                return Ok((true, true));
            }
            Ok((false, false))
        })
    }

    /// Return lifecycle metadata for one concrete `SKILL.md` path.
    pub fn skill_for_path(&self, path: &std::path::Path) -> Result<Option<SkillMeta>> {
        let normalized = normalize_skill_path(path);
        Ok(self
            .load_state()?
            .skills
            .into_values()
            .find(|meta| meta.path.as_ref() == Some(&normalized)))
    }

    /// Deprecate a skill, optionally specifying which skill supersedes it.
    pub fn deprecate_skill(&self, name: &str, superseded_by: Option<&str>) -> Result<bool> {
        self.with_locked_state(|state| {
            let now = chrono::Utc::now();
            if let Some(meta) = state.skills.get_mut(name)
                && matches!(
                    meta.lifecycle,
                    SkillLifecycle::Active | SkillLifecycle::Stale
                )
            {
                meta.lifecycle = SkillLifecycle::Deprecated;
                meta.superseded_by = superseded_by.map(|s| s.to_string());
                meta.last_modified_at = now;
                return Ok((true, true));
            }
            Ok((false, false))
        })
    }

    /// Pin a skill (exempt from auto-transitions).
    pub fn pin_skill(&self, name: &str) -> Result<()> {
        self.with_locked_state(|state| {
            if let Some(meta) = state.skills.get_mut(name) {
                meta.pinned = true;
                return Ok(((), true));
            }
            Ok(((), false))
        })
    }

    /// Unpin a skill.
    pub fn unpin_skill(&self, name: &str) -> Result<()> {
        self.with_locked_state(|state| {
            if let Some(meta) = state.skills.get_mut(name) {
                meta.pinned = false;
                return Ok(((), true));
            }
            Ok(((), false))
        })
    }

    /// Apply automatic lifecycle transitions based on inactivity.
    ///
    /// Returns a list of transitions that were applied.
    pub fn apply_transitions(&self) -> Result<Vec<(String, SkillLifecycle, SkillLifecycle)>> {
        if !self.config.enabled {
            return Ok(vec![]);
        }

        self.with_locked_state(|state| {
            let now = chrono::Utc::now();
            let mut transitions = Vec::new();

            for meta in state.skills.values_mut() {
                // Skip pinned and non-agent-created skills.
                if meta.pinned || !meta.agent_created {
                    continue;
                }

                // Candidates and Drafts don't auto-transition based on inactivity.
                if matches!(
                    meta.lifecycle,
                    SkillLifecycle::Candidate | SkillLifecycle::Draft
                ) {
                    continue;
                }

                // `num_days()` is i64 and can be negative under clock skew
                // (last_used_at later than now). `.max(0)` prevents the `as u64`
                // cast from wrapping a negative into a huge value, which would
                // instantly satisfy every idle threshold and wrongly archive
                // freshly-used skills (N9).
                let idle_days = (now - meta.last_used_at).num_days().max(0) as u64;

                let new_lifecycle = match meta.lifecycle {
                    SkillLifecycle::Active if idle_days >= self.config.stale_days => {
                        Some(SkillLifecycle::Stale)
                    }
                    SkillLifecycle::Stale if idle_days >= self.config.deprecate_days => {
                        Some(SkillLifecycle::Deprecated)
                    }
                    SkillLifecycle::Deprecated if idle_days >= self.config.archive_days => {
                        Some(SkillLifecycle::Archived)
                    }
                    _ => None,
                };

                if let Some(new_lc) = new_lifecycle {
                    transitions.push((meta.name.clone(), meta.lifecycle, new_lc));
                    meta.lifecycle = new_lc;
                }
            }

            state.last_run_at = Some(now);
            Ok((transitions, true))
        })
    }

    /// Get a summary of the curator state.
    pub fn status(&self) -> Result<CuratorStatus> {
        let state = self.load_state()?;
        let mut candidate = 0;
        let mut draft = 0;
        let mut active = 0;
        let mut stale = 0;
        let mut deprecated = 0;
        let mut archived = 0;
        let mut pinned = 0;

        for meta in state.skills.values() {
            if meta.pinned {
                pinned += 1;
            }
            match meta.lifecycle {
                SkillLifecycle::Candidate => candidate += 1,
                SkillLifecycle::Draft => draft += 1,
                SkillLifecycle::Active => active += 1,
                SkillLifecycle::Stale => stale += 1,
                SkillLifecycle::Deprecated => deprecated += 1,
                SkillLifecycle::Archived => archived += 1,
            }
        }

        Ok(CuratorStatus {
            total: state.skills.len(),
            candidate,
            draft,
            active,
            stale,
            deprecated,
            archived,
            pinned,
            last_run_at: state.last_run_at,
        })
    }
}

/// Summary of curator state.
#[derive(Debug, Clone)]
pub struct CuratorStatus {
    pub total: usize,
    pub candidate: usize,
    pub draft: usize,
    pub active: usize,
    pub stale: usize,
    pub deprecated: usize,
    pub archived: usize,
    pub pinned: usize,
    pub last_run_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// RAII guard that holds an exclusive advisory lock on the sidecar file.
///
/// Created by `Curator::acquire_lock()`. On drop, the lock is explicitly
/// released (`unlock`) and the fd closed — which also releases the OS advisory
/// lock as a safety net. The sidecar file remains in place so every process
/// locks the same inode.
///
/// Kill-safety relies on the OS releasing the flock when the fd is closed
/// (including on process death), NOT on the Drop running. So even if a process
/// is killed with SIGKILL mid-critical-section, no other curator is left
/// blocked.
struct CuratorLockGuard {
    file: Option<std::fs::File>,
}

impl Drop for CuratorLockGuard {
    fn drop(&mut self) {
        if let Some(file) = self.file.take() {
            // Explicit unlock first (best effort); closing the fd below also
            // releases the OS advisory lock.
            let _ = file.unlock();
            drop(file);
        }
    }
}

fn normalize_skill_path(path: &std::path::Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_curator() -> Curator {
        let dir = std::env::temp_dir().join(format!("echo_curator_test_{}", uuid::Uuid::new_v4()));
        let path = dir.join("curator_state.json");
        Curator::new(CuratorConfig::default(), path)
    }

    #[test]
    fn test_touch_and_status() -> Result<()> {
        let curator = temp_curator();
        curator.touch_skill("test-skill", true)?;

        let status = curator.status()?;
        assert_eq!(status.total, 1);
        assert_eq!(status.active, 1);
        assert_eq!(status.stale, 0);
        assert_eq!(status.archived, 0);
        Ok(())
    }

    #[test]
    fn test_tracks_skill_by_concrete_path() -> Result<()> {
        let curator = temp_curator();
        let skill_path = std::env::temp_dir()
            .join("echo-curator-path-test")
            .join("SKILL.md");
        curator.touch_skill_at("path-skill", Some(&skill_path), true)?;

        let Some(meta) = curator.skill_for_path(&skill_path)? else {
            return Err(crate::error::ReactError::Other(
                "skill path was not tracked".to_string(),
            ));
        };
        assert_eq!(meta.name, "path-skill");
        assert_eq!(meta.lifecycle, SkillLifecycle::Active);
        Ok(())
    }

    #[test]
    fn test_pin_unpin() -> Result<()> {
        let curator = temp_curator();
        curator.touch_skill("test-skill", true)?;
        curator.pin_skill("test-skill")?;

        let state = curator.load_state()?;
        assert!(
            state
                .skills
                .get("test-skill")
                .is_some_and(|meta| meta.pinned)
        );

        curator.unpin_skill("test-skill")?;
        let state = curator.load_state()?;
        assert!(
            state
                .skills
                .get("test-skill")
                .is_some_and(|meta| !meta.pinned)
        );
        Ok(())
    }

    #[test]
    fn test_no_transition_when_fresh() -> Result<()> {
        let curator = temp_curator();
        curator.touch_skill("fresh-skill", true)?;

        let transitions = curator.apply_transitions()?;
        assert!(transitions.is_empty());

        let status = curator.status()?;
        assert_eq!(status.active, 1);
        Ok(())
    }

    #[test]
    fn test_skip_non_agent_created() -> Result<()> {
        let curator = temp_curator();
        curator.touch_skill("bundled-skill", false)?;

        // Manually set last_used_at to old date
        let mut state = curator.load_state()?;
        state
            .skills
            .get_mut("bundled-skill")
            .ok_or_else(|| crate::error::ReactError::Other("missing bundled-skill".into()))?
            .last_used_at = chrono::Utc::now() - chrono::Duration::days(100);
        curator.save_state(&state)?;

        let transitions = curator.apply_transitions()?;
        // Should not transition non-agent-created skills
        assert!(transitions.is_empty());
        Ok(())
    }

    #[test]
    fn test_stale_transition() -> Result<()> {
        let curator = temp_curator();
        curator.touch_skill("old-skill", true)?;

        // Manually set last_used_at to 31 days ago
        let mut state = curator.load_state()?;
        state
            .skills
            .get_mut("old-skill")
            .ok_or_else(|| crate::error::ReactError::Other("missing old-skill".into()))?
            .last_used_at = chrono::Utc::now() - chrono::Duration::days(31);
        curator.save_state(&state)?;

        let transitions = curator.apply_transitions()?;
        assert_eq!(transitions.len(), 1);
        assert_eq!(transitions[0].1, SkillLifecycle::Active);
        assert_eq!(transitions[0].2, SkillLifecycle::Stale);

        let status = curator.status()?;
        assert_eq!(status.stale, 1);
        Ok(())
    }

    #[test]
    fn test_pinned_skips_transition() -> Result<()> {
        let curator = temp_curator();
        curator.touch_skill("pinned-skill", true)?;
        curator.pin_skill("pinned-skill")?;

        // Manually set last_used_at to 100 days ago
        let mut state = curator.load_state()?;
        state
            .skills
            .get_mut("pinned-skill")
            .ok_or_else(|| crate::error::ReactError::Other("missing pinned-skill".into()))?
            .last_used_at = chrono::Utc::now() - chrono::Duration::days(100);
        curator.save_state(&state)?;

        let transitions = curator.apply_transitions()?;
        assert!(transitions.is_empty());

        let status = curator.status()?;
        assert_eq!(status.active, 1);
        assert_eq!(status.pinned, 1);
        Ok(())
    }

    #[test]
    fn test_candidate_to_draft_to_active() -> Result<()> {
        let curator = temp_curator();

        // Register as candidate
        curator.register_candidate("my-skill")?;
        let state = curator.load_state()?;
        assert_eq!(
            state.skills.get("my-skill").map(|meta| meta.lifecycle),
            Some(SkillLifecycle::Candidate)
        );

        // Promote to draft
        let promoted = curator.promote_to_draft("my-skill")?;
        assert!(promoted);
        let state = curator.load_state()?;
        assert_eq!(
            state.skills.get("my-skill").map(|meta| meta.lifecycle),
            Some(SkillLifecycle::Draft)
        );

        // Promote to active
        let promoted = curator.promote_to_active("my-skill")?;
        assert!(promoted);
        let state = curator.load_state()?;
        assert_eq!(
            state.skills.get("my-skill").map(|meta| meta.lifecycle),
            Some(SkillLifecycle::Active)
        );
        Ok(())
    }

    #[test]
    fn test_deprecate_skill() -> Result<()> {
        let curator = temp_curator();
        curator.touch_skill("old-skill", true)?;

        let deprecated = curator.deprecate_skill("old-skill", Some("new-skill"))?;
        assert!(deprecated);

        let state = curator.load_state()?;
        assert_eq!(
            state.skills.get("old-skill").map(|meta| meta.lifecycle),
            Some(SkillLifecycle::Deprecated)
        );
        assert_eq!(
            state
                .skills
                .get("old-skill")
                .and_then(|meta| meta.superseded_by.as_deref()),
            Some("new-skill")
        );
        Ok(())
    }

    #[test]
    fn test_full_lifecycle() -> Result<()> {
        let curator = temp_curator();

        // Candidate -> Draft -> Active
        curator.register_candidate("lifecycle-skill")?;
        curator.promote_to_draft("lifecycle-skill")?;
        curator.promote_to_active("lifecycle-skill")?;

        // Set last_used_at to make it stale
        let mut state = curator.load_state()?;
        state
            .skills
            .get_mut("lifecycle-skill")
            .ok_or_else(|| crate::error::ReactError::Other("missing lifecycle-skill".into()))?
            .last_used_at = chrono::Utc::now() - chrono::Duration::days(35);
        curator.save_state(&state)?;

        let transitions = curator.apply_transitions()?;
        assert_eq!(transitions.len(), 1);
        assert_eq!(transitions[0].2, SkillLifecycle::Stale);

        // Set last_used_at to make it deprecated
        let mut state = curator.load_state()?;
        state
            .skills
            .get_mut("lifecycle-skill")
            .ok_or_else(|| crate::error::ReactError::Other("missing lifecycle-skill".into()))?
            .last_used_at = chrono::Utc::now() - chrono::Duration::days(65);
        curator.save_state(&state)?;

        let transitions = curator.apply_transitions()?;
        assert_eq!(transitions.len(), 1);
        assert_eq!(transitions[0].2, SkillLifecycle::Deprecated);

        // Set last_used_at to make it archived
        let mut state = curator.load_state()?;
        state
            .skills
            .get_mut("lifecycle-skill")
            .ok_or_else(|| crate::error::ReactError::Other("missing lifecycle-skill".into()))?
            .last_used_at = chrono::Utc::now() - chrono::Duration::days(95);
        curator.save_state(&state)?;

        let transitions = curator.apply_transitions()?;
        assert_eq!(transitions.len(), 1);
        assert_eq!(transitions[0].2, SkillLifecycle::Archived);
        Ok(())
    }

    #[test]
    fn test_candidate_no_auto_transition() -> Result<()> {
        let curator = temp_curator();
        curator.register_candidate("cand-skill")?;

        // Set last_used_at to very old
        let mut state = curator.load_state()?;
        state
            .skills
            .get_mut("cand-skill")
            .ok_or_else(|| crate::error::ReactError::Other("missing cand-skill".into()))?
            .last_used_at = chrono::Utc::now() - chrono::Duration::days(200);
        curator.save_state(&state)?;

        // Candidates should NOT auto-transition
        let transitions = curator.apply_transitions()?;
        assert!(transitions.is_empty());

        let state = curator.load_state()?;
        assert_eq!(
            state.skills.get("cand-skill").map(|meta| meta.lifecycle),
            Some(SkillLifecycle::Candidate)
        );
        Ok(())
    }

    #[test]
    fn test_register_candidate_idempotent() -> Result<()> {
        let curator = temp_curator();
        curator.register_candidate("my-skill")?;
        curator.register_candidate("my-skill")?;

        let state = curator.load_state()?;
        assert_eq!(state.skills.len(), 1);
        Ok(())
    }

    /// Regression for P0-1 (Curator TOCTOU): two `Curator` instances over the
    /// same state path, each registering a *different* skill, must both land.
    /// Under the pre-fix design (separate load/save locks), the second writer
    /// clobbered the first and one skill silently disappeared.
    #[test]
    fn test_concurrent_curators_do_not_lose_updates() -> Result<()> {
        // Shared state path (simulates two modules calling default_path()).
        let dir = std::env::temp_dir().join(format!("echo_curator_conc_{}", uuid::Uuid::new_v4()));
        let path = dir.join("curator_state.json");

        let a = Curator::new(CuratorConfig::default(), path.clone());
        let b = Curator::new(CuratorConfig::default(), path.clone());

        // Interleave the two instances as a race would.
        a.register_candidate("skill-a")?;
        b.register_candidate("skill-b")?;

        let state = a.load_state()?;
        assert!(
            state.skills.contains_key("skill-a"),
            "skill-a lost (TOCTOU regression)"
        );
        assert!(
            state.skills.contains_key("skill-b"),
            "skill-b lost (TOCTOU regression)"
        );
        Ok(())
    }

    /// Regression for the kill-9 deadlock (R1): acquiring the lock, dropping
    /// the guard without running normal cleanup (simulating a process that
    /// died but whose fd was closed by the OS), must NOT wedge the next
    /// locker. This holds because flock auto-releases on fd close; the stale
    /// sidecar file is not an obstacle since we never require `create_new`.
    #[test]
    fn test_lock_survives_abandoned_sidecar() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("echo_curator_stale_{}", uuid::Uuid::new_v4()));
        let path = dir.join("curator_state.json");

        // Pre-create a stale sidecar lock file (as if a prior process crashed).
        let lock_path = path.with_extension("json.lock");
        std::fs::create_dir_all(&dir)?;
        std::fs::write(&lock_path, b"")?;
        assert!(lock_path.exists(), "precondition: stale sidecar exists");

        // A new curator must still succeed — the stale file is not a barrier.
        let curator = Curator::new(CuratorConfig::default(), path.clone());
        curator.register_candidate("after-stale")?;
        assert!(curator.load_state()?.skills.contains_key("after-stale"));
        Ok(())
    }

    /// `with_locked_state` must not persist when the closure errors — the
    /// on-disk file stays at its pre-call state.
    #[test]
    fn test_with_locked_state_rollback_on_error() -> Result<()> {
        let curator = temp_curator();
        curator.register_candidate("keep-me")?;

        use crate::error::ReactError;
        let result: Result<()> = curator.with_locked_state(|state| {
            // Mutate in memory, then fail.
            state.skills.insert(
                "should-not-persist".to_string(),
                SkillMeta {
                    name: "should-not-persist".to_string(),
                    path: None,
                    lifecycle: SkillLifecycle::Candidate,
                    created_at: chrono::Utc::now(),
                    last_used_at: chrono::Utc::now(),
                    last_modified_at: chrono::Utc::now(),
                    pinned: false,
                    agent_created: true,
                    superseded_by: None,
                },
            );
            Err(ReactError::Other("simulated failure".to_string()))
        });
        assert!(result.is_err());

        let state = curator.load_state()?;
        assert!(state.skills.contains_key("keep-me"));
        assert!(
            !state.skills.contains_key("should-not-persist"),
            "failed mutation must not be persisted"
        );
        Ok(())
    }

    #[test]
    fn corrupt_state_is_reported_and_not_rewritten() -> Result<()> {
        let curator = temp_curator();
        let original = b"{not-json";
        if let Some(parent) = curator.state_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&curator.state_path, original)?;

        assert!(curator.load_state().is_err());
        assert!(curator.status().is_err());
        assert!(curator.register_candidate("must-not-overwrite").is_err());
        assert_eq!(std::fs::read(&curator.state_path)?, original);
        Ok(())
    }
}
