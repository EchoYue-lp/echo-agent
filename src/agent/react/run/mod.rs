//! ReactAgent execution engine
//!
//! Contains all internal implementations of the ReAct loop:
//! - `retry` — LLM retry logic + tool concurrent timeout calculation
//! - `context` — Context management + long-term memory + persistence + auditing
//! - `approval` — Tool execution approval (human-in-the-loop)
//! - `execution` — Tool execution (invocation, guards, truncation)
//! - `react_loop` — ReAct loop core (think / process_steps / run_react_loop)
//! - `direct` — Direct execution entry (run_direct / run_chat_direct)
//! - `stream_channel` — Channel-based streaming execution (primary)
//! - `processor` — SSE chunk → AgentEvent conversion
//! - `types` — Shared types (StreamMode, StreamInit)

pub(crate) mod approval;
pub(crate) mod context;
pub(crate) mod direct;
pub(crate) mod execution;
/// Tool execution pipeline — composable 13-stage middleware for tool calls.
pub mod pipeline;
pub(crate) mod processor;
pub(crate) mod react_loop;
pub(crate) mod retry;
pub(crate) mod stream_channel;
pub(crate) mod types;
pub(crate) use types::StreamMode;
