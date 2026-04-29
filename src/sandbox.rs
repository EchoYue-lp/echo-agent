//! 三层沙箱执行系统（re-export from echo_execution::sandbox）
//!
//! 提供 Local / Docker / K8s 三种隔离级别的代码执行环境，
//! 统一通过 [`SandboxExecutor`] trait 抽象，由 [`SandboxManager`] 自动选择最佳执行层。

pub use echo_execution::sandbox::*;
