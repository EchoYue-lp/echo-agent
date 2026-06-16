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
    /// Load state from a JSON file, or return default if not found.
    pub fn load(path: &PathBuf) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|data| serde_json::from_str(&data).ok())
            .unwrap_or_default()
    }

    /// Save state to a JSON file.
    pub fn save(&self, path: &PathBuf) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_string_pretty(self)?;
        std::fs::write(path, data)?;
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
pub struct Curator {
    config: CuratorConfig,
    state_path: PathBuf,
}

impl Curator {
    /// Create a new curator with the given config and state file path.
    pub fn new(config: CuratorConfig, state_path: impl Into<PathBuf>) -> Self {
        Self {
            config,
            state_path: state_path.into(),
        }
    }

    /// Create a curator with the default state path (`~/.echo-agent/curator_state.json`).
    pub fn default_path(config: CuratorConfig) -> Self {
        let home = std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."));
        let path = home.join(".echo-agent").join("curator_state.json");
        Self::new(config, path)
    }

    /// Load the current state.
    pub fn load_state(&self) -> CuratorState {
        CuratorState::load(&self.state_path)
    }

    /// Save the state.
    pub fn save_state(&self, state: &CuratorState) -> Result<()> {
        state.save(&self.state_path)
    }

    /// Register a new skill or update an existing one's last-used timestamp.
    pub fn touch_skill(&self, name: &str, agent_created: bool) -> Result<()> {
        let mut state = self.load_state();
        let now = chrono::Utc::now();

        if let Some(meta) = state.skills.get_mut(name) {
            meta.last_used_at = now;
        } else {
            state.skills.insert(
                name.to_string(),
                SkillMeta {
                    name: name.to_string(),
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

        self.save_state(&state)
    }

    /// Register a new skill candidate (discovered from memory patterns).
    pub fn register_candidate(&self, name: &str) -> Result<()> {
        let mut state = self.load_state();
        let now = chrono::Utc::now();

        if state.skills.contains_key(name) {
            // Already registered, skip
            return Ok(());
        }

        state.skills.insert(
            name.to_string(),
            SkillMeta {
                name: name.to_string(),
                lifecycle: SkillLifecycle::Candidate,
                created_at: now,
                last_used_at: now,
                last_modified_at: now,
                pinned: false,
                agent_created: true,
                superseded_by: None,
            },
        );

        self.save_state(&state)
    }

    /// Promote a Candidate skill to Draft.
    pub fn promote_to_draft(&self, name: &str) -> Result<bool> {
        let mut state = self.load_state();
        let now = chrono::Utc::now();

        if let Some(meta) = state.skills.get_mut(name)
            && meta.lifecycle == SkillLifecycle::Candidate
        {
            meta.lifecycle = SkillLifecycle::Draft;
            meta.last_modified_at = now;
            self.save_state(&state)?;
            return Ok(true);
        }
        Ok(false)
    }

    /// Promote a Draft skill to Active.
    pub fn promote_to_active(&self, name: &str) -> Result<bool> {
        let mut state = self.load_state();
        let now = chrono::Utc::now();

        if let Some(meta) = state.skills.get_mut(name)
            && meta.lifecycle == SkillLifecycle::Draft
        {
            meta.lifecycle = SkillLifecycle::Active;
            meta.last_modified_at = now;
            self.save_state(&state)?;
            return Ok(true);
        }
        Ok(false)
    }

    /// Deprecate a skill, optionally specifying which skill supersedes it.
    pub fn deprecate_skill(&self, name: &str, superseded_by: Option<&str>) -> Result<bool> {
        let mut state = self.load_state();
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
            self.save_state(&state)?;
            return Ok(true);
        }
        Ok(false)
    }

    /// Pin a skill (exempt from auto-transitions).
    pub fn pin_skill(&self, name: &str) -> Result<()> {
        let mut state = self.load_state();
        if let Some(meta) = state.skills.get_mut(name) {
            meta.pinned = true;
            self.save_state(&state)?;
        }
        Ok(())
    }

    /// Unpin a skill.
    pub fn unpin_skill(&self, name: &str) -> Result<()> {
        let mut state = self.load_state();
        if let Some(meta) = state.skills.get_mut(name) {
            meta.pinned = false;
            self.save_state(&state)?;
        }
        Ok(())
    }

    /// Apply automatic lifecycle transitions based on inactivity.
    ///
    /// Returns a list of transitions that were applied.
    pub fn apply_transitions(&self) -> Result<Vec<(String, SkillLifecycle, SkillLifecycle)>> {
        if !self.config.enabled {
            return Ok(vec![]);
        }

        let mut state = self.load_state();
        let now = chrono::Utc::now();
        let mut transitions = Vec::new();

        for meta in state.skills.values_mut() {
            // Skip pinned and non-agent-created skills
            if meta.pinned || !meta.agent_created {
                continue;
            }

            // Candidates and Drafts don't auto-transition based on inactivity
            if matches!(
                meta.lifecycle,
                SkillLifecycle::Candidate | SkillLifecycle::Draft
            ) {
                continue;
            }

            let idle_days = (now - meta.last_used_at).num_days() as u64;

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
        self.save_state(&state)?;

        Ok(transitions)
    }

    /// Get a summary of the curator state.
    pub fn status(&self) -> CuratorStatus {
        let state = self.load_state();
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

        CuratorStatus {
            total: state.skills.len(),
            candidate,
            draft,
            active,
            stale,
            deprecated,
            archived,
            pinned,
            last_run_at: state.last_run_at,
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_curator() -> Curator {
        let dir = std::env::temp_dir().join(format!("echo_curator_test_{}", uuid::Uuid::new_v4()));
        let path = dir.join("curator_state.json");
        Curator::new(CuratorConfig::default(), path)
    }

    #[test]
    fn test_touch_and_status() {
        let curator = temp_curator();
        curator.touch_skill("test-skill", true).unwrap();

        let status = curator.status();
        assert_eq!(status.total, 1);
        assert_eq!(status.active, 1);
        assert_eq!(status.stale, 0);
        assert_eq!(status.archived, 0);
    }

    #[test]
    fn test_pin_unpin() {
        let curator = temp_curator();
        curator.touch_skill("test-skill", true).unwrap();
        curator.pin_skill("test-skill").unwrap();

        let state = curator.load_state();
        assert!(state.skills["test-skill"].pinned);

        curator.unpin_skill("test-skill").unwrap();
        let state = curator.load_state();
        assert!(!state.skills["test-skill"].pinned);
    }

    #[test]
    fn test_no_transition_when_fresh() {
        let curator = temp_curator();
        curator.touch_skill("fresh-skill", true).unwrap();

        let transitions = curator.apply_transitions().unwrap();
        assert!(transitions.is_empty());

        let status = curator.status();
        assert_eq!(status.active, 1);
    }

    #[test]
    fn test_skip_non_agent_created() {
        let curator = temp_curator();
        curator.touch_skill("bundled-skill", false).unwrap();

        // Manually set last_used_at to old date
        let mut state = curator.load_state();
        state.skills.get_mut("bundled-skill").unwrap().last_used_at =
            chrono::Utc::now() - chrono::Duration::days(100);
        curator.save_state(&state).unwrap();

        let transitions = curator.apply_transitions().unwrap();
        // Should not transition non-agent-created skills
        assert!(transitions.is_empty());
    }

    #[test]
    fn test_stale_transition() {
        let curator = temp_curator();
        curator.touch_skill("old-skill", true).unwrap();

        // Manually set last_used_at to 31 days ago
        let mut state = curator.load_state();
        state.skills.get_mut("old-skill").unwrap().last_used_at =
            chrono::Utc::now() - chrono::Duration::days(31);
        curator.save_state(&state).unwrap();

        let transitions = curator.apply_transitions().unwrap();
        assert_eq!(transitions.len(), 1);
        assert_eq!(transitions[0].1, SkillLifecycle::Active);
        assert_eq!(transitions[0].2, SkillLifecycle::Stale);

        let status = curator.status();
        assert_eq!(status.stale, 1);
    }

    #[test]
    fn test_pinned_skips_transition() {
        let curator = temp_curator();
        curator.touch_skill("pinned-skill", true).unwrap();
        curator.pin_skill("pinned-skill").unwrap();

        // Manually set last_used_at to 100 days ago
        let mut state = curator.load_state();
        state.skills.get_mut("pinned-skill").unwrap().last_used_at =
            chrono::Utc::now() - chrono::Duration::days(100);
        curator.save_state(&state).unwrap();

        let transitions = curator.apply_transitions().unwrap();
        assert!(transitions.is_empty());

        let status = curator.status();
        assert_eq!(status.active, 1);
        assert_eq!(status.pinned, 1);
    }

    #[test]
    fn test_candidate_to_draft_to_active() {
        let curator = temp_curator();

        // Register as candidate
        curator.register_candidate("my-skill").unwrap();
        let state = curator.load_state();
        assert_eq!(
            state.skills["my-skill"].lifecycle,
            SkillLifecycle::Candidate
        );

        // Promote to draft
        let promoted = curator.promote_to_draft("my-skill").unwrap();
        assert!(promoted);
        let state = curator.load_state();
        assert_eq!(state.skills["my-skill"].lifecycle, SkillLifecycle::Draft);

        // Promote to active
        let promoted = curator.promote_to_active("my-skill").unwrap();
        assert!(promoted);
        let state = curator.load_state();
        assert_eq!(state.skills["my-skill"].lifecycle, SkillLifecycle::Active);
    }

    #[test]
    fn test_deprecate_skill() {
        let curator = temp_curator();
        curator.touch_skill("old-skill", true).unwrap();

        let deprecated = curator
            .deprecate_skill("old-skill", Some("new-skill"))
            .unwrap();
        assert!(deprecated);

        let state = curator.load_state();
        assert_eq!(
            state.skills["old-skill"].lifecycle,
            SkillLifecycle::Deprecated
        );
        assert_eq!(
            state.skills["old-skill"].superseded_by.as_deref(),
            Some("new-skill")
        );
    }

    #[test]
    fn test_full_lifecycle() {
        let curator = temp_curator();

        // Candidate -> Draft -> Active
        curator.register_candidate("lifecycle-skill").unwrap();
        curator.promote_to_draft("lifecycle-skill").unwrap();
        curator.promote_to_active("lifecycle-skill").unwrap();

        // Set last_used_at to make it stale
        let mut state = curator.load_state();
        state
            .skills
            .get_mut("lifecycle-skill")
            .unwrap()
            .last_used_at = chrono::Utc::now() - chrono::Duration::days(35);
        curator.save_state(&state).unwrap();

        let transitions = curator.apply_transitions().unwrap();
        assert_eq!(transitions.len(), 1);
        assert_eq!(transitions[0].2, SkillLifecycle::Stale);

        // Set last_used_at to make it deprecated
        let mut state = curator.load_state();
        state
            .skills
            .get_mut("lifecycle-skill")
            .unwrap()
            .last_used_at = chrono::Utc::now() - chrono::Duration::days(65);
        curator.save_state(&state).unwrap();

        let transitions = curator.apply_transitions().unwrap();
        assert_eq!(transitions.len(), 1);
        assert_eq!(transitions[0].2, SkillLifecycle::Deprecated);

        // Set last_used_at to make it archived
        let mut state = curator.load_state();
        state
            .skills
            .get_mut("lifecycle-skill")
            .unwrap()
            .last_used_at = chrono::Utc::now() - chrono::Duration::days(95);
        curator.save_state(&state).unwrap();

        let transitions = curator.apply_transitions().unwrap();
        assert_eq!(transitions.len(), 1);
        assert_eq!(transitions[0].2, SkillLifecycle::Archived);
    }

    #[test]
    fn test_candidate_no_auto_transition() {
        let curator = temp_curator();
        curator.register_candidate("cand-skill").unwrap();

        // Set last_used_at to very old
        let mut state = curator.load_state();
        state.skills.get_mut("cand-skill").unwrap().last_used_at =
            chrono::Utc::now() - chrono::Duration::days(200);
        curator.save_state(&state).unwrap();

        // Candidates should NOT auto-transition
        let transitions = curator.apply_transitions().unwrap();
        assert!(transitions.is_empty());

        let state = curator.load_state();
        assert_eq!(
            state.skills["cand-skill"].lifecycle,
            SkillLifecycle::Candidate
        );
    }

    #[test]
    fn test_register_candidate_idempotent() {
        let curator = temp_curator();
        curator.register_candidate("my-skill").unwrap();
        curator.register_candidate("my-skill").unwrap(); // Should not fail

        let state = curator.load_state();
        assert_eq!(state.skills.len(), 1);
    }
}
