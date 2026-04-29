//! IM 通道集成模块
//!
//! 此模块分成两层：
//! - `echo_integration::channels` 的直接 façade 重导出
//! - 根 crate 独有的 `AgentChannelHandler` 薄适配层，用于把 `ReactAgent`
//!   接到 IM channel 会话模型上
//!
//! 如需直接依赖拆分后的 crate，可使用 [`crate::workspace::integration::channels`]。
//!
//! # 能力继承
//!
//! 通过 `AgentChannelHandler` 创建的 Agent 自动继承框架的所有能力：
//! - 内置工具（think、memory、answer）
//! - 外部工具（MCP、Skill、web、media、data）
//! - 长期记忆（remember/recall/forget）
//! - 上下文压缩
//! - 护栏（Guard）
//! - 权限策略
//!
//! # 快速开始
//!
//! ```rust,no_run
//! use echo_agent::prelude::*;
//! use echo_agent::channels::*;
//! use std::sync::Arc;
//!
//! # async fn example() -> echo_agent::error::Result<()> {
//! // 1. 创建 ChannelManager
//! let mut manager = ChannelManager::new();
//!
//! // 2. 注册通道
//! manager.register(Box::new(QqChannel::new(QqConfig {
//!     app_id: "your-app-id".into(),
//!     client_secret: "your-secret".into(),
//! })?));
//!
//! // 3. 使用 AgentChannelHandler 自动桥接
//! let session_config = SessionConfig::default();
//! let handler_factory = |_channel_id: &str| -> Arc<dyn MessageHandler> {
//!     Arc::new(SessionHandler::new(
//!         session_config.clone(),
//!         || -> Box<dyn MessageHandler> {
//!             Box::new(AgentChannelHandler::from_config(
//!                 AgentConfig::standard("qwen3-max", "im-assistant", "你是一个友好的助手")
//!                     .enable_tool(true)
//!                     .enable_memory(true)
//!             ))
//!         },
//!     ))
//! };
//!
//! // 4. 启动
//! for result in manager.start_all(handler_factory).await {
//!     result?;
//! }
//! # Ok(())
//! # }
//! ```

/// Direct re-exports from `echo_integration::channels`.
pub mod integration {
    pub use echo_integration::channels::*;
}

pub use echo_integration::channels::prelude::*;

use crate::agent::Agent;
use crate::agent::react::ReactAgent;
use crate::prelude::AgentConfig;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::Mutex;

/// 基于 ReactAgent 的 IM 消息处理器
///
/// 将 IM 通道的消息转发给 ReactAgent 处理，自动继承框架的全部能力
/// （工具、记忆、MCP、Skills、压缩、护栏等）。
///
/// 每个用户会话（由 `SessionHandler` 管理）持有独立的 `AgentChannelHandler`，
/// 确保对话隔离。
pub struct AgentChannelHandler {
    agent: Arc<Mutex<ReactAgent>>,
}

impl AgentChannelHandler {
    /// 从已有的 ReactAgent 创建
    ///
    /// 适用于需要预先配置 Agent（如注入自定义工具、LlmConfig、Memory Store 等）的场景。
    pub fn new(agent: ReactAgent) -> Self {
        Self {
            agent: Arc::new(Mutex::new(agent)),
        }
    }

    /// 从 AgentConfig 创建
    ///
    /// 便捷构造器，自动创建 ReactAgent 并应用配置。
    /// 使用框架默认的 LLM 配置（环境变量）。
    pub fn from_config(config: AgentConfig) -> Self {
        Self::new(ReactAgent::new(config))
    }

    /// 使用标准预设创建
    ///
    /// 等价于 `from_config(AgentConfig::standard(model, name, prompt))`
    pub fn standard(model: &str, agent_name: &str, system_prompt: &str) -> Self {
        Self::from_config(
            AgentConfig::standard(model, agent_name, system_prompt)
                .enable_tool(true)
                .enable_memory(true),
        )
    }
}

#[async_trait]
impl MessageHandler for AgentChannelHandler {
    async fn handle(&self, msg: InboundMessage) -> echo_core::error::Result<OutboundMessage> {
        let agent = self.agent.lock().await;
        let reply = agent.chat(&msg.text).await?;

        Ok(OutboundMessage::new(
            &msg.channel_id,
            &msg.sender_id,
            msg.chat_type,
            &reply,
        ))
    }

    async fn reply(&self, _msg: OutboundMessage) -> echo_core::error::Result<()> {
        // reply 由 channel 自身处理
        Ok(())
    }
}
