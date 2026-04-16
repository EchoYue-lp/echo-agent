//! Workflow façade
//!
//! 此模块的核心运行时类型来自 `echo_orchestration::workflow`，
//! 根 crate 仅保留两类内容：
//! - 核心工作流类型的统一重导出
//! - 根 crate 特有的 `dsl` / `loader` 薄扩展入口
//!
//! 如需直接依赖拆分后的 crate，可使用 [`crate::workspace::orchestration::workflow`]。
//!
//! 提供两套编排能力：
//!
//! ## 1. Graph 工作流（对标 LangGraph）
//!
//! 将 Agent 执行建模为**有向图 + 共享状态**，支持：
//! - 线性管道、条件分支、循环、并行 fan-out/fan-in
//!
//! ## 2. Pipeline 工作流（Sequential / Concurrent / DAG）

/// Direct re-exports from `echo_orchestration::workflow`.
pub mod orchestration {
    pub use echo_orchestration::workflow::*;
}

pub use echo_orchestration::workflow::*;

/// Root-crate declarative DSL built on top of the orchestration workflow types.
pub mod dsl;
/// Root-crate file/YAML loader built on top of the orchestration workflow types.
pub mod loader;

pub use loader::WorkflowDefinition;
