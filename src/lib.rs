//! # echo-agent
//!
//! 一个通用、易用、高性能的 Rust AI Agent 开发框架。
//!
//! ## 核心特性
//!
//! - **ReAct 执行引擎**: 自动工具调用、多轮推理、流式输出
//! - **工具系统**: 内置工具 + MCP 协议 + 自定义扩展
//! - **双层记忆**: 会话持久化 + 长期 KV 存储
//! - **上下文压缩**: 滑动窗口 / LLM 摘要 / 混合管道
//! - **人工介入**: 审批 guard / 文本输入，支持多渠道
//!
//! ## 快速开始
//!
//! ```rust,no_run
//! use echo_agent::prelude::*;
//!
//! # #[tokio::main]
//! # async fn main() -> echo_agent::error::Result<()> {
//! // 创建 Agent
//! let mut agent = ReactAgentBuilder::simple("qwen3-max", "你是一个有帮助的助手")?;
//!
//! // 执行对话
//! let answer = agent.chat("你好！").await?;
//! println!("Agent: {}", answer);
//!
//! // 重置对话
//! agent.reset();
//! # Ok(())
//! # }
//! ```
//!
//! ## 带工具的 Agent
//!
//! ```rust,no_run
//! use echo_agent::prelude::*;
//!
//! # #[tokio::main]
//! # async fn main() -> echo_agent::error::Result<()> {
//! let mut agent = ReactAgentBuilder::new()
//!     .model("qwen3-max")
//!     .system_prompt("你是一个助手，可以使用工具完成任务")
//!     .enable_tools()
//!     .build()?;
//!
//! // agent 会自动使用 final_answer 工具返回结果
//! let answer = agent.chat("今天天气如何？").await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## 模块概览
//!
//! | 模块 | 能力 |
//! |------|------|
//! | [`agent`] | ReAct Agent 执行引擎 |
//! | [`llm`] | LLM 客户端（OpenAI 兼容） |
//! | [`tools`] | 工具系统（Tool trait、并发限流、超时重试） |
//! | [`memory`] | 双层记忆（Checkpointer + Store） |
//! | [`compression`] | 上下文压缩 |
//! | [`human_loop`] | 人工介入（审批/输入） |
//! | [`skills`] | Skill 系统（Tool + Prompt 包） |
//! | [`mcp`] | MCP 协议客户端 |
//! | [`tasks`] | DAG 任务管理 |
//! | [`error`] | 统一错误类型 |
//!

pub mod agent;
pub mod compression;
pub mod error;
pub mod human_loop;
pub mod llm;
pub mod mcp;
pub mod memory;
pub mod skills;
pub mod tasks;
pub mod testing;
pub mod tools;

/// 常用类型导出
///
/// 包含最常用的类型，通过 `use echo_agent::prelude::*` 导入。
pub mod prelude {
    pub use crate::agent::react_agent::ReactAgent;
    pub use crate::agent::react_agent::StepType;
    pub use crate::agent::{
        Agent, AgentBuilder, AgentCallback, AgentConfig, AgentEvent, AgentRole, CancellationToken,
        ReactAgentBuilder,
    };
    pub use crate::compression::compressor::{
        DefaultSummaryPrompt, FnSummaryPrompt, HybridCompressor, SlidingWindowCompressor,
        SummaryCompressor, SummaryPromptBuilder,
    };
    pub use crate::compression::{
        CompressionInput, CompressionOutput, ContextCompressor, ContextManager, ForceCompressStats,
    };
    pub use crate::error::Result;
    pub use crate::human_loop::{
        ApprovalDecision, ApprovalResponder, ConsoleHumanLoopProvider, HumanLoopEvent,
        HumanLoopHandler, HumanLoopManager, HumanLoopProvider, HumanLoopRequest, HumanLoopResponse,
        InputResponder, WebSocketHumanLoopProvider, WebhookHumanLoopProvider, dispatch_event,
    };
    pub use crate::llm::types::{Message, ToolCall};
    pub use crate::llm::{
        ChatChunk, ChatRequest, ChatResponse, JsonSchemaSpec, LlmClient, LlmConfig, OpenAiClient,
        ResponseFormat, ToolDefinition,
    };
    pub use crate::mcp::types::McpTool;
    pub use crate::mcp::{McpManager, McpServerConfig, TransportConfig};
    pub use crate::memory::checkpointer::{Checkpointer, FileCheckpointer, InMemoryCheckpointer};
    pub use crate::memory::embedder::{Embedder, HttpEmbedder};
    pub use crate::memory::embedding_store::EmbeddingStore;
    pub use crate::memory::store::{FileStore, InMemoryStore, Store, StoreItem};
    pub use crate::skills::{
        Skill, SkillInfo, SkillManager,
        builtin::{CalculatorSkill, FileSystemSkill, ShellSkill, WeatherSkill},
        external::{LoadedSkill, ResourceRef, SkillLoader, SkillMeta},
    };
    pub use crate::testing::{FailingMockAgent, MockAgent, MockEmbedder, MockLlmClient, MockTool};
    pub use crate::tools::builtin::think::ThinkTool;
    pub use crate::tools::{Tool, ToolExecutionConfig, ToolParameters, ToolResult};
}
