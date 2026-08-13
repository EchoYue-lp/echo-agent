//! Skill System — agentskills.io aligned (core re-export from echo_core + echo_execution)
//!
//! Two kinds of skills:
//!
//! | Kind | Registration | Progressive disclosure |
//! |------|-------------|----------------------|
//! | **Code-based** | `agent.add_skill(Box::new(MySkill))` | Eager (tools + prompt injected immediately) |
//! | **File-based** | `agent.discover_skills(path)` | 3-tier (catalog → activate → resources) |
//!
//! File-based skills follow the [agentskills.io specification](https://agentskills.io/specification).

/// Built-in skills provided by the Echo Agent framework (re-export from echo_execution).
pub mod builtin {
    #[cfg(feature = "files")]
    pub use echo_execution::skills::builtin::filesystem::FileSystemSkill;
    #[cfg(feature = "shell")]
    pub use echo_execution::skills::builtin::shell::ShellSkill;
}

/// File-based external skill loading (re-export from echo_execution).
pub mod external {
    pub use echo_execution::skills::external::*;
}

/// Skill hook system (re-export from echo_execution).
pub mod hooks {
    pub use echo_execution::skills::hooks::*;
}

/// Skill registry (re-export from echo_execution).
pub mod registry {
    pub use echo_execution::skills::registry::*;
}

// Core Skill trait + SkillInfo (defined in echo_core::tools::skill)
pub use echo_core::tools::skill::{
    Skill, SkillInfo, is_path_safe, minimal_env, minimal_hook_env, minimal_hook_env_with_context,
};

// Execution-layer types (SkillRegistry, etc.)
pub use echo_execution::skills::SkillRegistry;
