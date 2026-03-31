//! A2A (Agent-to-Agent) 协议支持
//!
//! 实现 Google A2A 协议规范，支持：
//! - **Agent Card** 发布：描述 Agent 的能力、技能和端点
//! - **Agent 发现**：通过 `/.well-known/agent.json` 发现远程 Agent
//! - **任务交互**：跨框架 Agent 间的任务发送与状态查询
//!
//! # 核心概念
//!
//! - [`AgentCard`]: Agent 能力描述卡片（符合 A2A 规范）
//! - [`A2AServer`]: HTTP 服务端，暴露 Agent Card 和任务接口
//! - [`A2AClient`]: HTTP 客户端，用于发现和调用远程 Agent
//!
//! # 示例
//!
//! ```rust,no_run
//! use echo_agent::a2a::{AgentCard, AgentSkill, A2AServer};
//! use echo_agent::prelude::*;
//!
//! # #[tokio::main]
//! # async fn main() -> echo_agent::error::Result<()> {
//! // 构建 Agent Card
//! let card = AgentCard::builder("translator", "http://localhost:8080")
//!     .description("多语言翻译 Agent")
//!     .version("1.0.0")
//!     .skill(AgentSkill::new("translate", "翻译文本"))
//!     .build();
//!
//! // 启动 A2A 服务
//! let agent = ReactAgentBuilder::simple("qwen3-max", "翻译助手")?;
//! let server = A2AServer::new(card, agent);
//! // server.serve("0.0.0.0:8080").await?;
//! # Ok(())
//! # }
//! ```

mod client;
mod server;
mod types;

pub use client::A2AClient;
pub use server::A2AServer;
pub use types::*;
