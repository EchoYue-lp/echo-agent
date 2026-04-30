//! Multi-layer code execution sandbox.
//!
//! Three isolation levels, unified behind the [`SandboxExecutor`] trait.
//! The [`SandboxManager`] automatically selects the best available layer.
//!
//! # Isolation Levels
//!
//! | Level | Backend | Use Case |
//! |-------|---------|----------|
//! | [`LocalSandbox`] | Direct process spawn | Development, trusted code |
//! | [`DockerSandbox`] | Docker container | CI/CD, untrusted code |
//! | [`K8sSandbox`] | Kubernetes pod | Production, multi-tenant |
//!
//! # Quick Start
//!
//! ```rust,ignore
//! use echo_agent::prelude::*;
//!
//! # fn main() -> echo_agent::error::Result<()> {
//! let sandbox = LocalSandbox::new();
//! let result = sandbox.execute(SandboxCommand::shell("echo hello"))?;
//! println!("{}", result.stdout);
//! # Ok(())
//! # }
//! ```

pub use echo_execution::sandbox::*;
