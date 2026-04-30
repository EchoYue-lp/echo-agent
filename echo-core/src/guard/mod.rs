//! Guard system core trait and types

#[cfg(feature = "guard")]
pub mod content;
#[cfg(feature = "guard")]
pub mod llm;
#[cfg(feature = "guard")]
pub mod rule;

use crate::error::Result;
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Guard check direction
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GuardDirection {
    /// User input direction check
    Input,
    /// Model output direction check
    Output,
    /// Tool input parameter check
    ToolInput,
    /// Tool output result check
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

/// Guard check result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GuardResult {
    Pass,
    Block {
        /// Block reason
        reason: String,
    },
    /// Multiple warnings collected from all guards.
    Warn {
        /// Warning reason list
        reasons: Vec<String>,
    },
}

impl GuardResult {
    pub fn is_blocked(&self) -> bool {
        matches!(self, GuardResult::Block { .. })
    }
}

/// Guard trait
pub trait Guard: Send + Sync {
    /// Get the guard name
    fn name(&self) -> &str;

    /// Check content
    ///
    /// # Parameters
    /// * `content` - Content to check
    /// * `direction` - Check direction
    fn check<'a>(
        &'a self,
        content: &'a str,
        direction: GuardDirection,
    ) -> BoxFuture<'a, Result<GuardResult>>;
}

/// Guard manager
pub struct GuardManager {
    guards: Vec<Arc<dyn Guard>>,
}

impl Default for GuardManager {
    fn default() -> Self {
        Self::new()
    }
}

impl GuardManager {
    /// Create an empty guard manager
    pub fn new() -> Self {
        Self { guards: Vec::new() }
    }

    /// Add a guard
    pub fn add(&mut self, guard: Arc<dyn Guard>) {
        self.guards.push(guard);
    }

    /// Create a manager from a list of guards
    pub fn from_guards(guards: Vec<Arc<dyn Guard>>) -> Self {
        Self { guards }
    }

    /// Check if empty (no guards added)
    pub fn is_empty(&self) -> bool {
        self.guards.is_empty()
    }

    /// Run all guard checks in parallel.
    ///
    /// - All guards start simultaneously (subject to concurrency cap), rather than running serially.
    /// - Once a `Block` result is detected, cancel other in-flight checks (via `CancellationToken`).
    /// - Collect all `Warn` reasons into `Vec<String>`.
    ///
    /// The concurrency cap is 16 to prevent spawning an excessive number of tasks
    /// when many guards are registered.
    pub async fn check_all(&self, content: &str, direction: GuardDirection) -> Result<GuardResult> {
        if self.guards.is_empty() {
            return Ok(GuardResult::Pass);
        }

        // Concurrency limit to avoid spawning unbounded tasks
        let semaphore = Arc::new(tokio::sync::Semaphore::new(16));
        let cancel = CancellationToken::new();
        let mut handles = Vec::with_capacity(self.guards.len());

        for guard in &self.guards {
            let guard = guard.clone();
            let content = content.to_string();
            let cancel_child = cancel.clone();
            let permit = semaphore.clone().acquire_owned().await;
            handles.push(tokio::spawn(async move {
                let _permit = permit; // hold until task completes
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
                    cancel.cancel(); // cancel other in-flight checks
                    tracing::warn!(
                        guard = guard_name,
                        direction = %direction,
                        reason = %reason,
                        "Guard blocked content"
                    );
                    return Ok(GuardResult::Block { reason });
                }
                Ok(GuardResult::Warn { reasons }) => {
                    warnings.extend(reasons);
                }
                Ok(GuardResult::Pass) => {}
                Err(e) => {
                    tracing::error!(guard = guard_name, error = %e, "Guard check error");
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
