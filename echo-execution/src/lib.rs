//! # echo_execution
//!
//! Execution layer: sandbox, skills, and tools.
//!
//! This crate contains the low-level runtime for:
//! - multi-layer sandbox execution (Local / Docker / K8s)
//! - file-based skill loading, activation, hooks, and script execution
//! - tool registry / execution primitives shared by higher-level agent crates

pub mod sandbox;
pub mod skills;
pub mod tools;
