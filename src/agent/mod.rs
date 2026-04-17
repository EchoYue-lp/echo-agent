//! Agent 模块
//!
//! 定义 [`Agent`] 核心 trait、事件枚举 [`AgentEvent`] 和回调接口 [`AgentCallback`]。
//!
//! ## 内置 Agent 范式
//!
//! | 模块 | 范式 | Feature |
//! |------|------|---------|
//! | [`react`] | ReAct（Think-Act-Observe） | 始终可用 |
//! | [`plan_execute`] | Plan-and-Execute | `plan-execute` |
//! | [`self_reflection`] | Self-Reflection | `self-reflection` |
//! | [`subagent`] | Subagent 子代理系统 | `subagent` |
//!
//! # 快速开始
//!
//! ```rust,no_run
//! use echo_agent::prelude::*;
//!
//! # fn main() -> echo_agent::error::Result<()> {
//! // 使用 Builder 创建 Agent
//! let agent = ReactAgentBuilder::new()
//!     .model("qwen3-max")
//!     .system_prompt("你是一个有帮助的助手")
//!     .enable_tools()
//!     .build()?;
//!
//! println!("Agent name: {}", agent.name());
//! println!("Model: {}", agent.model_name());
//! # Ok(())
//! # }
//! ```

pub use echo_core::agent::{Agent, AgentCallback, AgentEvent, CancellationToken, StepType};

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tokio::sync::Mutex as AsyncMutex;

/// SubAgent 注册表类型别名
pub(crate) type SubAgentMap = Arc<RwLock<HashMap<String, Arc<AsyncMutex<Box<dyn Agent>>>>>>;

// ── 核心子模块 ──────────────────────────────────────────────────────────────

pub mod config;
pub mod react;
pub mod runner;

#[cfg(feature = "plan-execute")]
pub mod plan_execute;
#[cfg(feature = "self-reflection")]
pub mod self_reflection;
#[cfg(feature = "subagent")]
pub mod subagent;

// ── Re-exports ──────────────────────────────────────────────────────────────

pub use crate::agent::react::ReactAgent;
pub use crate::agent::react::builder::ReactAgentBuilder;
pub use crate::agent::react::structured::StructuredAgent;
pub use config::{AgentConfig, AgentRole};
pub use runner::Runner;

/// AgentBuilder 是 ReactAgentBuilder 的别名，用于宏和极简 API
pub type AgentBuilder = ReactAgentBuilder;
