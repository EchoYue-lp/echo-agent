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
    /// Content was safely transformed and must replace the original payload.
    Transform {
        /// Replacement content to pass to subsequent guards and consumers.
        content: String,
        /// Reasons describing the transformation.
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

impl Clone for GuardManager {
    fn clone(&self) -> Self {
        Self {
            guards: self.guards.clone(),
        }
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

    /// Run the guard chain and return its decision, including transformed content.
    ///
    /// Guards run in registration order because a transforming guard changes the
    /// content that every subsequent guard must inspect.
    pub async fn check_all(&self, content: &str, direction: GuardDirection) -> Result<GuardResult> {
        if self.guards.is_empty() {
            return Ok(GuardResult::Pass);
        }

        let mut current = content.to_string();
        let mut warnings = Vec::new();
        let mut transformed = false;

        for guard in &self.guards {
            let guard_name = guard.name();
            match guard.check(&current, direction).await {
                Ok(GuardResult::Block { reason }) => {
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
                Ok(GuardResult::Transform { content, reasons }) => {
                    current = content;
                    transformed = true;
                    warnings.extend(reasons);
                }
                Ok(GuardResult::Pass) => {}
                Err(e) => {
                    tracing::error!(guard = guard_name, error = %e, "Guard check error");
                    warnings.push(format!("{} error: {}", guard_name, e));
                }
            }
        }

        if transformed {
            Ok(GuardResult::Transform {
                content: current,
                reasons: warnings,
            })
        } else if !warnings.is_empty() {
            Ok(GuardResult::Warn { reasons: warnings })
        } else {
            Ok(GuardResult::Pass)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ReplaceGuard;

    impl Guard for ReplaceGuard {
        fn name(&self) -> &str {
            "replace"
        }

        fn check<'a>(
            &'a self,
            content: &'a str,
            _direction: GuardDirection,
        ) -> BoxFuture<'a, Result<GuardResult>> {
            Box::pin(async move {
                Ok(GuardResult::Transform {
                    content: content.replace("secret", "[redacted]"),
                    reasons: vec!["redacted secret".to_string()],
                })
            })
        }
    }

    struct RejectUnredactedGuard;

    impl Guard for RejectUnredactedGuard {
        fn name(&self) -> &str {
            "reject-unredacted"
        }

        fn check<'a>(
            &'a self,
            content: &'a str,
            _direction: GuardDirection,
        ) -> BoxFuture<'a, Result<GuardResult>> {
            Box::pin(async move {
                if content.contains("secret") {
                    Ok(GuardResult::Block {
                        reason: "unredacted secret".to_string(),
                    })
                } else {
                    Ok(GuardResult::Pass)
                }
            })
        }
    }

    #[tokio::test]
    async fn transformations_feed_subsequent_guards() -> Result<()> {
        let manager = GuardManager::from_guards(vec![
            Arc::new(ReplaceGuard),
            Arc::new(RejectUnredactedGuard),
        ]);

        let result = manager.check_all("a secret", GuardDirection::Input).await?;
        match result {
            GuardResult::Transform { content, reasons } => {
                assert_eq!(content, "a [redacted]");
                assert_eq!(reasons, vec!["redacted secret"]);
            }
            other => {
                return Err(crate::error::ReactError::Other(format!(
                    "expected transform, got {other:?}"
                )));
            }
        }
        Ok(())
    }
}
