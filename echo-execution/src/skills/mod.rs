//! Skill System -- agentskills.io aligned
//!
//! Two kinds of skills:
//!
//! | Kind | Registration | Progressive disclosure |
//! |------|-------------|----------------------|
//! | **Code-based** | `agent.add_skill(Box::new(MySkill))` | Eager (tools + prompt injected immediately) |
//! | **File-based** | `agent.discover_skills(path)` | 3-tier (catalog -> activate -> resources) |
//!
//! File-based skills follow the [agentskills.io specification](https://agentskills.io/specification).

pub mod external;
pub mod hooks;
pub mod registry;

use echo_core::tools::Tool;
use std::collections::HashMap;
use std::path::{Component, Path};

// -- Convenience re-exports --

pub use echo_core::error::{ReactError, Result, ToolError};

// -- Shared security utilities --

/// Check whether `sub` is a safe sub-path of `base`.
///
/// Uses `Path::components` to detect `ParentDir` ("..") components,
/// which is more robust than string-level `contains("..")` checks.
/// Returns `false` if any component is `ParentDir` or if the resolved
/// path would escape the base directory.
pub fn is_path_safe(base: &Path, sub: &Path) -> bool {
    // Reject any ParentDir component in the relative path
    for component in sub.components() {
        if matches!(component, Component::ParentDir) {
            return false;
        }
    }

    // Verify resolved path stays under base
    if let (Ok(canonical_base), Ok(canonical_sub)) =
        (base.canonicalize(), base.join(sub).canonicalize())
    {
        canonical_sub.starts_with(&canonical_base)
    } else {
        // If canonicalize fails on base, accept the simple component check
        sub.components().all(|c| !matches!(c, Component::ParentDir))
    }
}

/// Return a minimal environment for subprocess execution.
///
/// Only includes PATH (cleaned) and explicitly passed variables.
/// This prevents leaking sensitive environment variables (e.g., AWS keys,
/// API tokens, credentials) to child processes.
pub fn minimal_env(
    skill_dir: &str,
    session_id: &str,
    extra: HashMap<String, String>,
) -> HashMap<String, String> {
    let mut env = HashMap::new();

    // Only pass a cleaned PATH (no inherited vars like PYTHONPATH, etc.)
    if let Ok(path) = std::env::var("PATH") {
        env.insert("PATH".to_string(), path);
    }

    // Skill-specific variables
    env.insert("SKILL_DIR".to_string(), skill_dir.to_string());
    env.insert("SESSION_ID".to_string(), session_id.to_string());

    // Caller-provided extras
    for (k, v) in extra {
        env.insert(k, v);
    }

    env
}

/// Return minimal env for hook execution (skill dir + session only).
pub fn minimal_hook_env(skill_dir: &str, session_id: &str) -> HashMap<String, String> {
    let mut env = HashMap::new();
    if let Ok(path) = std::env::var("PATH") {
        env.insert("PATH".to_string(), path);
    }
    env.insert("SKILL_DIR".to_string(), skill_dir.to_string());
    env.insert("SESSION_ID".to_string(), session_id.to_string());
    env
}

// -- Skill Trait (code-based skills) --

/// Agent skill -- a higher-level capability unit that bundles related tools
/// with an optional system-prompt injection.
///
/// For file-based skills loaded from `SKILL.md`, see [`external::SkillDescriptor`]
/// and the [`registry::SkillRegistry`] progressive disclosure system.
///
/// # Skill vs Tool
///
/// | Dimension | Tool | Skill |
/// |-----------|------|-------|
/// | Granularity | Single atomic operation | Domain capability (multi-tool + prompt) |
/// | Registration | `agent.add_tool(box)` | `agent.add_skill(box)` |
/// | Prompt | None | Optional guidance injection |
/// | Semantics | "do one thing" | "I master a domain"
pub trait Skill: Send + Sync {
    /// Unique skill identifier (lowercase, e.g. `"calculator"`).
    fn name(&self) -> &str;

    /// Human-readable description.
    fn description(&self) -> &str;

    /// Tools provided by this skill.
    ///
    /// Each call should return fresh `Box<dyn Tool>` instances.
    fn tools(&self) -> Vec<Box<dyn Tool>>;

    /// Optional text appended to the agent's system prompt when this skill is installed.
    fn system_prompt_injection(&self) -> Option<String> {
        None
    }

    /// Shut down the skill, releasing any held resources.
    /// Default implementation does nothing.
    fn shutdown(&self) {}
}

// -- SkillInfo --

/// Metadata snapshot for an installed code-based skill.
#[derive(Debug, Clone)]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    pub tool_names: Vec<String>,
    pub has_prompt_injection: bool,
}

// -- Re-exports --

pub use registry::SkillRegistry;

// Backward compatibility
#[allow(deprecated)]
pub use registry::SkillManager;

// -- Tests --

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_skill_info_structure() {
        let info = SkillInfo {
            name: "calculator".to_string(),
            description: "Performs calculations".to_string(),
            tool_names: vec!["add".to_string(), "subtract".to_string()],
            has_prompt_injection: true,
        };

        assert_eq!(info.name, "calculator");
        assert_eq!(info.description, "Performs calculations");
        assert_eq!(info.tool_names.len(), 2);
        assert!(info.has_prompt_injection);
    }

    #[test]
    fn test_is_path_safe_basic() {
        let base = Path::new("/tmp/skill");
        assert!(is_path_safe(base, Path::new("scripts/run.py")));
        assert!(is_path_safe(base, Path::new("README.md")));
        assert!(!is_path_safe(base, Path::new("../secret.txt")));
        assert!(!is_path_safe(base, Path::new("foo/../../escape")));
    }

    #[test]
    fn test_minimal_env_contains_expected_keys() {
        let env = minimal_env("/tmp/skill", "sess-1", HashMap::new());
        assert!(env.contains_key("PATH") || env.is_empty()); // PATH might not exist
        assert_eq!(env.get("SKILL_DIR").unwrap(), "/tmp/skill");
        assert_eq!(env.get("SESSION_ID").unwrap(), "sess-1");
        assert!(!env.contains_key("HOME"));
    }
}
