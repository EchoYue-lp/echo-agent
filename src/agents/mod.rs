//! 内置 Agent 范式
//!
//! | 模块 | 范式 | Feature |
//! |------|------|---------|
//! | [`react`] | ReAct（Think-Act-Observe） | 始终可用 |
//! | [`plan_execute`] | Plan-and-Execute | `plan-execute` |
//! | [`self_reflection`] | Self-Reflection | `self-reflection` |

pub mod react;

#[cfg(feature = "plan-execute")]
pub mod plan_execute;

#[cfg(feature = "self-reflection")]
pub mod self_reflection;
