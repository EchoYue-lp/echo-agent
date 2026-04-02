//! Skill System — agentskills.io aligned
//!
//! Two kinds of skills:
//!
//! | Kind | Registration | Progressive disclosure |
//! |------|-------------|----------------------|
//! | **Code-based** | `agent.add_skill(Box::new(MySkill))` | Eager (tools + prompt injected immediately) |
//! | **File-based** | `agent.discover_skills(path)` | 3-tier (catalog → activate → resources) |
//!
//! File-based skills follow the [agentskills.io specification](https://agentskills.io/specification).

pub mod builtin;
pub mod external;
pub mod hooks;
pub mod registry;

use crate::tools::Tool;

// ── Skill Trait (code-based skills) ──────────────────────────────────────────

/// Agent skill — a higher-level capability unit that bundles related tools
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
/// | Semantics | "do one thing" | "I master a domain" |
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
}

// ── SkillInfo ────────────────────────────────────────────────────────────────

/// Metadata snapshot for an installed code-based skill.
#[derive(Debug, Clone)]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    pub tool_names: Vec<String>,
    pub has_prompt_injection: bool,
}

// ── Re-exports ───────────────────────────────────────────────────────────────

pub use registry::SkillRegistry;

// Backward compatibility
#[allow(deprecated)]
pub use registry::SkillManager;

// ── Tests ────────────────────────────────────────────────────────────────────

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
}
