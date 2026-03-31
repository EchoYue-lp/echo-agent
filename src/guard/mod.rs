//! 护栏系统
//!
//! 对用户输入、LLM 输出和工具调用进行安全过滤，支持基于规则和基于 LLM 的两种检查模式。
//!
//! # 核心类型
//!
//! - [`Guard`]: 护栏 trait，所有检查器必须实现
//! - [`GuardManager`]: 管理多个 Guard，按顺序执行并短路
//! - [`GuardResult`]: 检查结果（放行 / 阻断 / 警告）
//! - [`GuardDirection`]: 检查方向（输入 / 输出 / 工具输入 / 工具输出）

pub mod llm;
pub mod rule;

pub use echo_core::guard::*;
