//! Streaming execution loop — modular implementation.
//!
//! - `processor` — SSE chunk → AgentEvent conversion
//! - `llm_stream` — LLM HTTP stream creation with retry
//! - `entry` — Public entry points (`run_stream`, `run_stream_with_message`)
//! - `runner` — Core ReAct loop (`run_stream_inner`)

pub(crate) mod entry;
pub(crate) mod llm_stream;
pub(crate) mod processor;
