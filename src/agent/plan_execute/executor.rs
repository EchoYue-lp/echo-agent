//! Executor — responsible for executing individual steps in a plan

use crate::agent::Agent;
use crate::error::Result;
use futures::future::BoxFuture;
use tracing::{debug, info};

/// Executor trait — executes a single step
pub trait Executor: Send + Sync {
    /// Execute a single step
    ///
    /// - `step_description`: Step description
    /// - `context`: Execution context (results from previous steps, etc.)
    fn execute_step<'a>(
        &'a mut self,
        step_description: &'a str,
        context: &'a str,
    ) -> BoxFuture<'a, Result<String>>;
}

// ── ReactExecutor ────────────────────────────────────────────────────────────

/// Uses ReactAgent as the Executor: each step is executed through a full ReAct loop
pub struct ReactExecutor {
    agent: Box<dyn Agent>,
}

impl ReactExecutor {
    /// Create a ReactExecutor from a type implementing the Agent trait
    ///
    /// # Parameters
    /// * `agent` - Agent instance for executing steps
    pub fn new(agent: impl Agent + 'static) -> Self {
        Self {
            agent: Box::new(agent),
        }
    }

    /// Create a ReactExecutor from an already-boxed Agent trait object
    ///
    /// # Parameters
    /// * `agent` - Already-boxed Agent trait object
    pub fn from_boxed(agent: Box<dyn Agent>) -> Self {
        Self { agent }
    }
}

impl Executor for ReactExecutor {
    fn execute_step<'a>(
        &'a mut self,
        step_description: &'a str,
        context: &'a str,
    ) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            let prompt = if context.is_empty() {
                step_description.to_string()
            } else {
                format!("{}\n\n{}", context, step_description)
            };

            info!(
                agent = %self.agent.name(),
                step = %step_description,
                "⚡ ReactExecutor executing step"
            );

            let result = self.agent.execute(&prompt).await?;
            debug!(
                agent = %self.agent.name(),
                output_len = result.len(),
                "✅ Step execution complete"
            );

            Ok(result)
        })
    }
}

// ── SimpleExecutor ───────────────────────────────────────────────────────────

/// Simple Executor: calls LLM directly (no tools), suitable for pure reasoning steps
pub struct SimpleExecutor {
    agent: Box<dyn Agent>,
}

impl SimpleExecutor {
    /// Create a SimpleExecutor from a type implementing the Agent trait
    ///
    /// # Parameters
    /// * `agent` - Agent instance for executing steps
    pub fn new(agent: impl Agent + 'static) -> Self {
        Self {
            agent: Box::new(agent),
        }
    }
}

impl Executor for SimpleExecutor {
    fn execute_step<'a>(
        &'a mut self,
        step_description: &'a str,
        context: &'a str,
    ) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            let prompt = if context.is_empty() {
                step_description.to_string()
            } else {
                format!("{}\n\n{}", context, step_description)
            };

            self.agent.execute(&prompt).await
        })
    }
}
