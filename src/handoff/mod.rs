//! Handoff 模块 — Agent 间控制权转移
//!
//! 支持将当前 Agent 的执行控制权转移给另一个 Agent，并传递上下文信息。
//!
//! # 核心概念
//!
//! - [`HandoffTarget`]: 描述要转移到的目标 Agent
//! - [`HandoffContext`]: 携带的上下文数据（消息历史、元数据等）
//! - [`HandoffResult`]: 转移执行后的结果
//! - [`HandoffManager`]: 管理 Agent 注册和 Handoff 执行
//!
//! # 示例
//!
//! ```rust,no_run
//! use echo_agent::handoff::{HandoffManager, HandoffTarget, HandoffContext};
//! use echo_agent::prelude::*;
//! use std::sync::Arc;
//! use tokio::sync::Mutex;
//!
//! # #[tokio::main]
//! # async fn main() -> echo_agent::error::Result<()> {
//! let mut manager = HandoffManager::new();
//!
//! // 注册 Agent
//! let agent = ReactAgentBuilder::simple("qwen3-max", "我是翻译助手")?;
//! manager.register("translator", agent);
//!
//! // 执行 Handoff
//! let target = HandoffTarget::new("translator")
//!     .with_message("请将以下内容翻译为英文：你好世界");
//! let context = HandoffContext::new()
//!     .with_metadata("source_lang", "zh");
//! let result = manager.handoff(target, context).await?;
//! println!("翻译结果: {}", result.output);
//! # Ok(())
//! # }
//! ```

mod tool;

use crate::agent::Agent;
use crate::error::{AgentError, ReactError, Result};
use crate::llm::types::Message;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, info};

pub use tool::HandoffTool;

// ── HandoffTarget ────────────────────────────────────────────────────────────

/// 描述 Handoff 的目标 Agent 及要执行的任务
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandoffTarget {
    /// 目标 Agent 名称（需已在 HandoffManager 中注册）
    pub agent_name: String,
    /// 要传递给目标 Agent 的任务消息
    pub message: Option<String>,
    /// 是否传递完整的对话历史给目标 Agent
    pub transfer_history: bool,
}

impl HandoffTarget {
    pub fn new(agent_name: impl Into<String>) -> Self {
        Self {
            agent_name: agent_name.into(),
            message: None,
            transfer_history: false,
        }
    }

    /// 设置要传递给目标 Agent 的消息
    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }

    /// 开启对话历史传递
    pub fn with_history(mut self) -> Self {
        self.transfer_history = true;
        self
    }
}

// ── HandoffContext ───────────────────────────────────────────────────────────

/// Handoff 时携带的上下文信息
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HandoffContext {
    /// 源 Agent 名称
    pub source_agent: Option<String>,
    /// 对话历史（当 transfer_history 为 true 时填充）
    pub messages: Vec<Message>,
    /// 自定义元数据（键值对）
    pub metadata: HashMap<String, String>,
}

impl HandoffContext {
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置源 Agent 名称
    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source_agent = Some(source.into());
        self
    }

    /// 设置对话历史
    pub fn with_messages(mut self, messages: Vec<Message>) -> Self {
        self.messages = messages;
        self
    }

    /// 添加元数据
    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }
}

// ── HandoffResult ────────────────────────────────────────────────────────────

/// Handoff 执行结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandoffResult {
    /// 目标 Agent 名称
    pub target_agent: String,
    /// 源 Agent 名称（如果有）
    pub source_agent: Option<String>,
    /// 执行输出
    pub output: String,
    /// 是否建议将控制权交还给源 Agent
    pub return_to_source: bool,
}

// ── HandoffManager ───────────────────────────────────────────────────────────

/// Handoff 管理器：负责 Agent 注册和 Handoff 执行调度
pub struct HandoffManager {
    agents: HashMap<String, Arc<Mutex<Box<dyn Agent>>>>,
}

impl HandoffManager {
    pub fn new() -> Self {
        Self {
            agents: HashMap::new(),
        }
    }

    /// 注册一个 Agent（接受任意实现 Agent trait 的类型）
    pub fn register(&mut self, name: impl Into<String>, agent: impl Agent + 'static) {
        let name = name.into();
        self.agents
            .insert(name, Arc::new(Mutex::new(Box::new(agent))));
    }

    /// 注册一个已封装的 Agent
    pub fn register_boxed(&mut self, name: impl Into<String>, agent: Box<dyn Agent>) {
        let name = name.into();
        self.agents.insert(name, Arc::new(Mutex::new(agent)));
    }

    /// 注册一个已包装为 Arc<Mutex<Box<dyn Agent>>> 的 Agent
    pub fn register_shared(&mut self, name: impl Into<String>, agent: Arc<Mutex<Box<dyn Agent>>>) {
        self.agents.insert(name.into(), agent);
    }

    /// 获取已注册的 Agent 名称列表
    pub fn registered_agents(&self) -> Vec<&str> {
        self.agents.keys().map(|s| s.as_str()).collect()
    }

    /// 检查 Agent 是否已注册
    pub fn has_agent(&self, name: &str) -> bool {
        self.agents.contains_key(name)
    }

    /// 执行 Handoff：将控制权转移到目标 Agent
    ///
    /// 流程：
    /// 1. 查找目标 Agent
    /// 2. 构建上下文提示（包含元数据和历史）
    /// 3. 调用目标 Agent 执行
    /// 4. 返回结果
    pub async fn handoff(
        &self,
        target: HandoffTarget,
        context: HandoffContext,
    ) -> Result<HandoffResult> {
        let agent_arc = self.agents.get(&target.agent_name).ok_or_else(|| {
            ReactError::Agent(AgentError::InitializationFailed(format!(
                "Handoff 目标 Agent '{}' 未注册。可用 Agent: {:?}",
                target.agent_name,
                self.registered_agents()
            )))
        })?;

        info!(
            source = ?context.source_agent,
            target = %target.agent_name,
            transfer_history = %target.transfer_history,
            metadata_keys = ?context.metadata.keys().collect::<Vec<_>>(),
            "🤝 执行 Handoff"
        );

        let mut agent = agent_arc.lock().await;

        // 构建发送给目标 Agent 的完整提示
        let mut prompt_parts = Vec::new();

        // 添加来源信息
        if let Some(source) = &context.source_agent {
            prompt_parts.push(format!("[Handoff 来源: Agent '{}']", source));
        }

        // 添加元数据
        if !context.metadata.is_empty() {
            let meta_lines: Vec<String> = context
                .metadata
                .iter()
                .map(|(k, v)| format!("  - {}: {}", k, v))
                .collect();
            prompt_parts.push(format!("[上下文元数据]\n{}", meta_lines.join("\n")));
        }

        // 添加历史摘要
        if target.transfer_history && !context.messages.is_empty() {
            let history_summary: Vec<String> = context
                .messages
                .iter()
                .filter_map(|msg| msg.content.as_ref().map(|c| format!("{}: {}", msg.role, c)))
                .collect();
            prompt_parts.push(format!("[对话历史]\n{}", history_summary.join("\n")));
        }

        // 添加任务消息
        if let Some(message) = &target.message {
            prompt_parts.push(format!("[任务]\n{}", message));
        }

        let full_prompt = prompt_parts.join("\n\n");
        debug!(
            target = %target.agent_name,
            prompt_len = full_prompt.len(),
            "📨 发送 Handoff 提示"
        );

        let output = agent.execute(&full_prompt).await?;

        info!(
            target = %target.agent_name,
            output_len = output.len(),
            "✅ Handoff 执行完成"
        );

        Ok(HandoffResult {
            target_agent: target.agent_name,
            source_agent: context.source_agent,
            output,
            return_to_source: false,
        })
    }

    /// 执行 Handoff 链：依次将控制权传递给多个 Agent
    ///
    /// 每个 Agent 的输出会作为下一个 Agent 的输入。
    pub async fn handoff_chain(
        &self,
        targets: Vec<HandoffTarget>,
        initial_context: HandoffContext,
    ) -> Result<Vec<HandoffResult>> {
        let mut results = Vec::new();
        let mut current_context = initial_context;

        for target in targets {
            let result = self.handoff(target, current_context.clone()).await?;

            // 为下一个 Agent 更新上下文
            current_context = HandoffContext::new()
                .with_source(result.target_agent.clone())
                .with_metadata("previous_output", result.output.clone());

            results.push(result);
        }

        Ok(results)
    }
}

impl Default for HandoffManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_handoff_target() {
        let target = HandoffTarget::new("agent_b")
            .with_message("请处理此任务")
            .with_history();

        assert_eq!(target.agent_name, "agent_b");
        assert_eq!(target.message.as_deref(), Some("请处理此任务"));
        assert!(target.transfer_history);
    }

    #[test]
    fn test_handoff_context() {
        let ctx = HandoffContext::new()
            .with_source("agent_a")
            .with_metadata("key1", "value1")
            .with_metadata("key2", "value2");

        assert_eq!(ctx.source_agent.as_deref(), Some("agent_a"));
        assert_eq!(ctx.metadata.len(), 2);
        assert_eq!(ctx.metadata.get("key1").unwrap(), "value1");
    }

    #[test]
    fn test_handoff_manager_register() {
        let manager = HandoffManager::new();
        assert!(manager.registered_agents().is_empty());
        assert!(!manager.has_agent("test"));
    }

    #[tokio::test]
    async fn test_handoff_agent_not_found() {
        let manager = HandoffManager::new();
        let target = HandoffTarget::new("nonexistent");
        let ctx = HandoffContext::new();

        let result = manager.handoff(target, ctx).await;
        assert!(result.is_err());
    }
}
