//! Skill System — agentskills.io aligned (core re-export from echo_execution::skills + builtin skills)
//!
//! Two kinds of skills:
//!
//! | Kind | Registration | Progressive disclosure |
//! |------|-------------|----------------------|
//! | **Code-based** | `agent.add_skill(Box::new(MySkill))` | Eager (tools + prompt injected immediately) |
//! | **File-based** | `agent.discover_skills(path)` | 3-tier (catalog → activate → resources) |
//!
//! File-based skills follow the [agentskills.io specification](https://agentskills.io/specification).

/// Built-in skills provided by the Echo Agent framework.
pub mod builtin;

/// File-based external skill loading (re-export from echo_execution).
pub mod external {
    pub use echo_execution::skills::external::*;
}

/// Skill hook system (re-export from echo_execution).
pub mod hooks {
    pub use echo_execution::skills::hooks::*;
}

// Re-export all types from echo_execution::skills
pub use echo_execution::skills::*;
