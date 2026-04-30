//! # echo_execution
//!
//! Execution layer for the [echo-agent](https://crates.io/crates/echo_agent) framework.
//!
//! ## Modules
//!
//! | Module | Description |
//! |--------|-------------|
//! | [`sandbox`] | Multi-layer code execution: `LocalSandbox`, `DockerSandbox`, `K8sSandbox` |
//! | [`skills`] | File-based skill system: discover → activate → use, with hooks |
//! | [`tools`] | `ToolManager` — registry, execution, concurrency control |
//!
//! Most users should depend on `echo_agent` (the facade crate) instead of
//! depending on `echo_execution` directly.

pub mod sandbox;
pub mod skills;
pub mod tools;
