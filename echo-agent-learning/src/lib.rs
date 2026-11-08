// verify.sh与CI显式运行本crate的--doc；--all-targets本身不执行rustdoc。
// no_run只编译入门示例，不调用真实模型。
#![doc = include_str!("../../docs/zh/getting-started.md")]

//! Executable Rust lessons for contributors to `echo-agent`.
//!
//! The crate is a workspace-only teaching aid. Production crates never depend
//! on it; instead, its examples consume the same public APIs as downstream
//! users.

pub mod async_concurrency;
pub mod basics;
pub mod collections;
pub mod errors;
pub mod fundamentals;
pub mod ownership;
pub mod project_patterns;
pub mod serialization;
pub mod smart_pointers;
pub mod traits;
