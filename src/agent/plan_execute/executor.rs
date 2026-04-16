//! Executor — 负责执行计划中的单个步骤

use crate::agent::Agent;
use crate::error::Result;
use futures::future::BoxFuture;
use tracing::{debug, info};

/// Executor trait — 执行单个步骤
pub trait Executor: Send + Sync {
    /// 执行单个步骤
    ///
    /// - `step_description`: 步骤描述
    /// - `context`: 执行上下文（之前步骤的结果等）
    fn execute_step<'a>(
        &'a mut self,
        step_description: &'a str,
        context: &'a str,
    ) -> BoxFuture<'a, Result<String>>;
}

// ── ReactExecutor ────────────────────────────────────────────────────────────

/// 使用 ReactAgent 作为 Executor：每个步骤通过完整的 ReAct 循环执行
pub struct ReactExecutor {
    agent: Box<dyn Agent>,
}

impl ReactExecutor {
    pub fn new(agent: impl Agent + 'static) -> Self {
        Self {
            agent: Box::new(agent),
        }
    }

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
                "⚡ ReactExecutor 执行步骤"
            );

            let result = self.agent.execute(&prompt).await?;
            debug!(
                agent = %self.agent.name(),
                output_len = result.len(),
                "✅ 步骤执行完成"
            );

            Ok(result)
        })
    }
}

// ── SimpleExecutor ───────────────────────────────────────────────────────────

/// 简单 Executor：直接调用 LLM（无工具），适用于纯推理步骤
pub struct SimpleExecutor {
    agent: Box<dyn Agent>,
}

impl SimpleExecutor {
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
