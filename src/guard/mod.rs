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

use crate::error::Result;
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// 护栏检查方向
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GuardDirection {
    /// 用户输入（进入 LLM 前）
    Input,
    /// LLM 输出（返回给用户前）
    Output,
    /// 工具输入参数
    ToolInput,
    /// 工具输出结果
    ToolOutput,
}

impl std::fmt::Display for GuardDirection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GuardDirection::Input => write!(f, "input"),
            GuardDirection::Output => write!(f, "output"),
            GuardDirection::ToolInput => write!(f, "tool_input"),
            GuardDirection::ToolOutput => write!(f, "tool_output"),
        }
    }
}

/// 护栏检查结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GuardResult {
    /// 内容安全，允许通过
    Pass,
    /// 内容被阻断，附带原因
    Block { reason: String },
    /// 内容存在风险但允许通过，附带警告原因
    Warn { reason: String },
}

impl GuardResult {
    pub fn is_blocked(&self) -> bool {
        matches!(self, GuardResult::Block { .. })
    }
}

/// 护栏 trait
///
/// 所有护栏检查器（规则引擎、LLM 审查等）必须实现此 trait。
pub trait Guard: Send + Sync {
    /// 护栏名称
    fn name(&self) -> &str;

    /// 对内容执行安全检查
    fn check<'a>(
        &'a self,
        content: &'a str,
        direction: GuardDirection,
    ) -> BoxFuture<'a, Result<GuardResult>>;
}

/// 护栏管理器
///
/// 持有多个 Guard 实例，按注册顺序依次执行检查。
/// 遇到 `Block` 立即短路返回。
pub struct GuardManager {
    guards: Vec<Arc<dyn Guard>>,
}

impl Default for GuardManager {
    fn default() -> Self {
        Self::new()
    }
}

impl GuardManager {
    pub fn new() -> Self {
        Self { guards: Vec::new() }
    }

    /// 添加护栏
    pub fn add(&mut self, guard: Arc<dyn Guard>) {
        self.guards.push(guard);
    }

    /// 从护栏列表构建
    pub fn from_guards(guards: Vec<Arc<dyn Guard>>) -> Self {
        Self { guards }
    }

    /// 是否为空（无护栏注册）
    pub fn is_empty(&self) -> bool {
        self.guards.is_empty()
    }

    /// 依次执行所有护栏检查，遇到 Block 立即短路。
    ///
    /// 返回第一个 `Block` 结果，或收集所有 `Warn`，或 `Pass`。
    pub async fn check_all(
        &self,
        content: &str,
        direction: GuardDirection,
    ) -> Result<GuardResult> {
        let mut warnings = Vec::new();

        for guard in &self.guards {
            let result = guard.check(content, direction).await?;
            match result {
                GuardResult::Block { reason } => {
                    tracing::warn!(
                        guard = guard.name(),
                        direction = %direction,
                        reason = %reason,
                        "🛡️ 护栏阻断"
                    );
                    return Ok(GuardResult::Block { reason });
                }
                GuardResult::Warn { reason } => {
                    tracing::warn!(
                        guard = guard.name(),
                        direction = %direction,
                        reason = %reason,
                        "⚠️ 护栏警告"
                    );
                    warnings.push(reason);
                }
                GuardResult::Pass => {}
            }
        }

        if !warnings.is_empty() {
            Ok(GuardResult::Warn {
                reason: warnings.join("; "),
            })
        } else {
            Ok(GuardResult::Pass)
        }
    }
}
