//! Skill System — agentskills.io aligned（核心 re-export from echo_skills crate + builtin skills）
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

// Re-export all types from echo_execution::skills
pub use echo_execution::skills::*;
