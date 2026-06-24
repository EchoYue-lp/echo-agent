//! Built-in framework-level tools
//!
//! These are core to the agent execution loop. Domain-specific tools
//! (git, browser, data, media, etc.) live in the `echo_tools` crate.

#[cfg(feature = "subagent")]
pub(crate) mod agent_dispatch;
pub(crate) mod answer;
#[cfg(feature = "tasks")]
pub(crate) mod check_task;
#[cfg(feature = "human-loop")]
pub(crate) mod human_in_loop;
pub(crate) mod memory;
#[cfg(feature = "tasks")]
pub(crate) mod spawn_task;
/// Think tool for reasoning and reflection.
pub mod think;
pub(crate) mod todo;
