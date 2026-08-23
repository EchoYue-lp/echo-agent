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

pub mod dependency_probe;
pub mod external;
pub mod hooks;
pub mod registry;

// -- Convenience re-exports --

pub use echo_core::error::{ReactError, Result, ToolError};
pub use echo_core::tools::skill::{
    Skill, SkillInfo, is_path_safe, minimal_env, minimal_hook_env, minimal_hook_env_with_context,
};

// -- Re-exports --

pub use registry::SkillRegistry;
