//! A2A (Agent-to-Agent) protocol support
//!
//! Implements the Google A2A protocol specification, supporting:
//! - **Agent Card** publishing: describes Agent capabilities, skills, and endpoints
//! - **Agent discovery**: find remote Agents via `/.well-known/agent.json`
//! - **Task interaction**: send tasks and query status across Agent frameworks
//! - **Task state machine**: `submitted → working → [input-required] → completed / failed`
//! - **Streaming events**: receive real-time SSE event streams via `tasks/sendSubscribe`
//!
//! # State machine
//!
//! ```text
//! submitted → working → completed
//!                     → failed
//!                     → input-required ⇄ working
//!
//! Any non-terminal state → canceled
//! ```
//!
//! # Core types
//!
//! | Type | Description |
//! |------|-------------|
//! | [`AgentCard`] | Agent capability description card |
//! | [`A2AServer`] | Server, exposes Agent Card and task endpoints |
//! | [`A2AClient`] | Client, discovers and invokes remote Agents |
//! | [`TaskState`] | Task lifecycle state enum |
//! | [`A2AStreamEvent`] | Streaming events (status changes / artifact updates) |
//!
//! # Example
//!
//! ```rust,no_run
//! use echo_agent::a2a::{AgentCard, AgentSkill, A2AServer, TaskState};
//! use echo_agent::prelude::*;
//!
//! # #[tokio::main]
//! # async fn main() -> echo_agent::error::Result<()> {
//! let card = AgentCard::builder("translator", "http://localhost:8080")
//!     .description("Multilingual translation agent")
//!     .version("1.0.0")
//!     .skill(AgentSkill::new("translate", "Translate text"))
//!     .streaming()
//!     .build();
//!
//! let agent = ReactAgentBuilder::simple("qwen3-max", "Translation assistant")?;
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

pub use auth::{JwtClaims, JwtConfig, JwtConfigError, get_claims};
pub use client::A2AClient;
pub use serve::{serve, serve_from_config, serve_from_config_with_auth, serve_with_auth};
pub use server::A2AServer;
pub use types::*;
