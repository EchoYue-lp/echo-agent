//! Agent 抽象层
//!
//! 定义 [`Agent`] 核心 trait、事件枚举 [`AgentEvent`] 和回调接口 [`AgentCallback`]。
//! 主要实现为 [`react_agent::ReactAgent`]。
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

pub use config::{AgentConfig, AgentRole};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tokio::sync::Mutex as AsyncMutex;

pub use echo_core::agent::{Agent, AgentCallback, AgentEvent, CancellationToken, StepType};

/// SubAgent 注册表类型别名
pub(crate) type SubAgentMap = Arc<RwLock<HashMap<String, Arc<AsyncMutex<Box<dyn Agent>>>>>>;

mod config;
#[cfg(feature = "tasks")]
mod planning;
pub mod react_agent;
pub mod runner;

pub use react_agent::builder::ReactAgentBuilder;
pub use runner::Runner;

/// AgentBuilder 是 ReactAgentBuilder 的别名，用于宏和极简 API
pub type AgentBuilder = ReactAgentBuilder;
