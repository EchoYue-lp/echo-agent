//! 护栏系统
//!
//! 对用户输入、LLM 输出和工具调用进行安全过滤，支持基于规则和基于 LLM 的两种检查模式。
//!
//! 核心 trait、结果类型与内置具体实现都直接来自 `echo_core::guard`；
//! 根 crate 仅保留模块路径兼容层。
//! 如需直接依赖拆分后的 crate，可使用 [`crate::workspace::core::guard`]。
//!
//! # 核心类型
//!
//! - [`Guard`]: 护栏 trait，所有检查器必须实现
//! - [`GuardManager`]: 管理多个 Guard，按顺序执行并短路
//! - [`GuardResult`]: 检查结果（放行 / 阻断 / 警告）
//! - [`GuardDirection`]: 检查方向（输入 / 输出 / 工具输入 / 工具输出）

pub mod llm;
pub mod rule;

/// Direct re-exports from `echo_core::guard`.
pub mod core {
    pub use echo_core::guard::*;
}

pub use echo_core::guard::*;
