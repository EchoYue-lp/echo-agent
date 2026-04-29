//! A2A (Agent-to-Agent) 协议支持
//!
//! 实现 Google A2A 协议规范，支持：
//! - **Agent Card** 发布：描述 Agent 的能力、技能和端点
//! - **Agent 发现**：通过 `/.well-known/agent.json` 发现远程 Agent
//! - **任务交互**：跨框架 Agent 间的任务发送与状态查询
//! - **任务状态机**：`submitted → working → [input-required] → completed / failed`
//! - **流式事件**：通过 `tasks/sendSubscribe` 接收实时 SSE 事件流
//!
//! # 状态机
//!
//! ```text
//! submitted → working → completed
//!                     → failed
//!                     → input-required ⇄ working
//!
//! 任何非终态 → canceled
//! ```
//!
//! # 核心类型
//!
//! | 类型 | 说明 |
//! |------|------|
//! | [`AgentCard`] | Agent 能力描述卡片 |
//! | [`A2AServer`] | 服务端，暴露 Agent Card 和任务接口 |
//! | [`A2AClient`] | 客户端，发现和调用远程 Agent |
//! | [`TaskState`] | 任务生命周期状态枚举 |
//! | [`A2AStreamEvent`] | 流式事件（状态变更 / Artifact 更新）|
//!
//! # 示例
//!
//! ```rust,no_run
//! use echo_agent::a2a::{AgentCard, AgentSkill, A2AServer, TaskState};
//! use echo_agent::prelude::*;
//!
//! # #[tokio::main]
//! # async fn main() -> echo_agent::error::Result<()> {
//! let card = AgentCard::builder("translator", "http://localhost:8080")
//!     .description("多语言翻译 Agent")
//!     .version("1.0.0")
//!     .skill(AgentSkill::new("translate", "翻译文本"))
//!     .streaming()
//!     .build();
//!
//! let agent = ReactAgentBuilder::simple("qwen3-max", "翻译助手")?;
//! let server = A2AServer::new(card, agent);
//! // server.handle_request(...) for sync
//! // server.handle_request_stream(...) for SSE streaming
//! # Ok(())
//! # }
//! ```

mod auth;
mod client;
mod serve;
mod server;
mod types;

pub use auth::{JwtClaims, JwtConfig, get_claims};
pub use client::A2AClient;
pub use serve::{serve, serve_from_config, serve_from_config_with_auth, serve_with_auth};
pub use server::A2AServer;
pub use types::*;
