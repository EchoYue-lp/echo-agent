//! ReactAgent execution engine
//!
//! Contains all internal implementations of the ReAct loop:
//! - `retry` — LLM retry logic + tool concurrent timeout calculation
//! - `context` — Context management + long-term memory + persistence + auditing
//! - `react_loop` — ReAct loop entry and context preparation
//! - `direct` — Direct execution entry (run_direct / run_chat_direct)
//! - `stream_channel` — Channel-based streaming execution (primary)
//! - `processor` — SSE chunk → AgentEvent conversion
//! - `types` — Shared types (StreamMode, StreamInit)

use std::time::Duration;

pub(super) const TOOL_CANCELLATION_GRACE_PERIOD: Duration = Duration::from_secs(5);
const STREAM_CANCELLATION_SETTLE_PERIOD: Duration = Duration::from_secs(6);

pub(crate) mod context;
pub(crate) mod direct;
pub(crate) mod phases;
/// Tool execution pipeline — composable 13-stage middleware for tool calls.
pub mod pipeline;
pub(crate) mod processor;
pub(crate) mod react_loop;
pub(crate) mod retry;
pub(crate) mod stream_channel;
pub(crate) mod stream_macros;
pub(crate) mod types;
pub(crate) use types::StreamMode;
