//! # echo-core
//!
//! Core traits, types, and error definitions for the [echo-agent](https://crates.io/crates/echo_agent) framework.
//!
//! This crate provides the shared interfaces that all other workspace crates build on:
//!
//! | Module | Contents |
//! |--------|----------|
//! | [`agent`] | `Agent` trait, `AgentEvent`, `AgentCallback`, `CancellationToken`, `Critic` trait with built-in implementations, `Executor` trait with built-in implementations, reflection types and store |
//! | [`llm`] | `LlmClient` trait, `ChatRequest`/`ChatResponse`/`ChatChunk` types |
//! | [`tools`] | `Tool` trait, `ToolResult`, `ToolPermission`, permission model |
//! | [`error`] | Unified `Error` enum covering all failure modes |
//! | [`guard`] | `Guard` trait, `GuardResult`, `GuardManager` |
//! | [`audit`] | `AuditEvent`, `AuditLogger` trait |
//! | [`retry`] | `RetryPolicy` with exponential backoff |
//! | [`tokenizer`] | Token counting interface |
//!
//! Most users should depend on `echo_agent` (the facade crate) instead of
//! depending on `echo_core` directly.

pub mod agent;
pub mod audit;
pub mod budget;
pub mod circuit_breaker;
pub mod compression;
pub mod error;
pub mod guard;
pub mod hooks;
pub mod llm;
pub mod memory;
pub mod project_rules;
pub mod retry;
pub mod sandbox;
pub mod tokenizer;
pub mod tools;
pub mod utils;
