//! Built-in framework-level tools
//!
//! These are core to the agent execution loop. Domain-specific tools
//! (git, browser, data, media, etc.) live in the `echo_tools` crate.

#[cfg(feature = "subagent")]
pub(crate) mod agent_dispatch;
pub(crate) mod answer;
pub(crate) mod cell_tools;
/// Model-invocable human-in-the-loop tool. Public so embedders that swap
/// the approval provider (e.g. the SDK extension bridge) can register the
/// matching appeal tool on agents they construct themselves.
#[cfg(feature = "human-loop")]
pub mod human_in_loop;
pub(crate) mod memory;
#[cfg(feature = "subagent")]
pub(crate) mod subagent_message;
/// Think tool for reasoning and reflection.
pub mod think;
