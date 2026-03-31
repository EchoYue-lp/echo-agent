//! 护栏系统核心 trait 和类型

use crate::error::Result;
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// 护栏检查方向
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GuardDirection {
    Input,
    Output,
    ToolInput,
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
    Pass,
    Block { reason: String },
    Warn { reason: String },
}

impl GuardResult {
    pub fn is_blocked(&self) -> bool {
        matches!(self, GuardResult::Block { .. })
    }
}

/// 护栏 trait
pub trait Guard: Send + Sync {
    fn name(&self) -> &str;

    fn check<'a>(
        &'a self,
        content: &'a str,
        direction: GuardDirection,
    ) -> BoxFuture<'a, Result<GuardResult>>;
}

/// 护栏管理器
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

    pub fn add(&mut self, guard: Arc<dyn Guard>) {
        self.guards.push(guard);
    }

    pub fn from_guards(guards: Vec<Arc<dyn Guard>>) -> Self {
        Self { guards }
    }

    pub fn is_empty(&self) -> bool {
        self.guards.is_empty()
    }

    pub async fn check_all(&self, content: &str, direction: GuardDirection) -> Result<GuardResult> {
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
