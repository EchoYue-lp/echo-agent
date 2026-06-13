//! # echo_orchestration
//!
//! Orchestration layer for the [echo-agent](https://crates.io/crates/echo_agent) framework.
//!
//! ## Modules
//!
//! | Module | Description |
//! |--------|-------------|
//! | [`workflow`] | Graph workflow engine — `GraphBuilder`, `Graph`, streaming events |
//! | [`human_loop`] | Human-in-the-loop approvals — console, webhook, WebSocket, permission pipeline |
//! | [`tasks`] | DAG task management — `TaskManager`, scheduling, hooks, events |
//! | [`scheduler`] | Cron-based task scheduling — `CronTask`, `SchedulerRunner` |
//! | [`planning`] | Structured task planning — `PlanSpec`, `PlanValidator` |
//!
//! ## Feature Flags
//!
//! - `websocket` — Enable `WebSocketHumanLoopProvider` for WebSocket-based approvals
//!
//! Most users should depend on `echo_agent` (the facade crate) instead of
//! depending on `echo_orchestration` directly.

pub mod human_loop;
pub mod planning;
pub mod scheduler;
pub mod tasks;
pub mod workflow;
