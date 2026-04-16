//! 护栏系统核心 trait 和类型

#![allow(missing_docs)]

#[cfg(feature = "guard")]
pub mod llm;
#[cfg(feature = "guard")]
pub mod rule;

use crate::error::Result;
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

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
    Block {
        reason: String,
    },
    /// Multiple warnings collected from all guards.
    Warn {
        reasons: Vec<String>,
    },
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

    /// 并行执行所有护栏检查。
    ///
    /// - 所有护栏同时启动（`join_all`），而非串行执行。
    /// - 一旦发现 `Block` 结果，取消其他仍在运行的检查（通过 `CancellationToken`）。
    /// - 收集所有 `Warn` 理由到 `Vec<String>`。
    pub async fn check_all(&self, content: &str, direction: GuardDirection) -> Result<GuardResult> {
        if self.guards.is_empty() {
            return Ok(GuardResult::Pass);
        }

        let cancel = CancellationToken::new();
        let mut handles = Vec::with_capacity(self.guards.len());

        for guard in &self.guards {
            let guard = guard.clone();
            let content = content.to_string();
            let cancel_child = cancel.clone();
            handles.push(tokio::spawn(async move {
                let result = tokio::select! {
                    _ = cancel_child.cancelled() => {
                        return (guard.name().to_string(), Ok(GuardResult::Pass));
                    }
                    r = guard.check(&content, direction) => r,
                };
                (guard.name().to_string(), result)
            }));
        }

        let mut warnings = Vec::new();

        for (i, handle) in handles.into_iter().enumerate() {
            let (guard_name, result) = handle.await.map_err(|e| {
                crate::error::ReactError::Other(format!("Guard task {} panicked: {}", i, e))
            })?;

            match result {
                Ok(GuardResult::Block { reason }) => {
                    cancel.cancel(); // 取消其他仍在运行的检查
                    tracing::warn!(
                        guard = guard_name,
                        direction = %direction,
                        reason = %reason,
                        "护栏阻断"
                    );
                    return Ok(GuardResult::Block { reason });
                }
                Ok(GuardResult::Warn { reasons }) => {
                    warnings.extend(reasons);
                }
                Ok(GuardResult::Pass) => {}
                Err(e) => {
                    tracing::error!(guard = guard_name, error = %e, "护栏检查出错");
                    warnings.push(format!("{} error: {}", guard_name, e));
                }
            }
        }

        if !warnings.is_empty() {
            Ok(GuardResult::Warn { reasons: warnings })
        } else {
            Ok(GuardResult::Pass)
        }
    }
}
