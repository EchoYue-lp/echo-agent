//! echo-agent：可组合的 Rust Agent 开发框架
//!
//! # 核心能力
//!
//! | 模块 | 能力 |
//! |------|------|
//! | [`agent`] | ReAct Agent 执行引擎（工具调用、规划、流式输出） |
//! | [`llm`] | LLM 客户端，支持 OpenAI 兼容 API |
//! | [`tools`] | 工具系统（Tool trait、执行管理、并发限流、超时重试） |
//! | [`memory`] | 双层记忆（Checkpointer 会话持久化 + Store 长期 KV） |
//! | [`compression`] | 上下文压缩（滑动窗口 / LLM 摘要 / 混合管道） |
//! | [`human_loop`] | 人工介入（审批 guard / 文本输入，支持命令行、Webhook、WebSocket） |
//! | [`skills`] | Skill 系统（Tool 集合 + Prompt 注入的能力包） |
//! | [`mcp`] | MCP 协议客户端，接入外部工具服务端 |
//! | [`tasks`] | DAG 任务管理（规划模式专用） |
//! | [`error`] | 统一错误类型树 |
//!
//! # 快速上手
//!
//! ```rust,no_run
//! use echo_agent::prelude::*;
//!
//! # async fn run() -> echo_agent::error::Result<()> {
//! let config = AgentConfig::new("gpt-4o", "assistant", "你是一个有帮助的助手")
//!     .enable_tool(true);
//!
//! let mut agent = ReactAgent::new(config);
//! let answer = agent.execute("你好").await?;
//! println!("{answer}");
//! # Ok(())
//! # }
//! ```

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

pub mod prelude {
    pub use crate::agent::react_agent::ReactAgent;
    pub use crate::agent::{Agent, AgentCallback, AgentConfig, AgentEvent, AgentRole};
    pub use crate::compression::compressor::{
        DefaultSummaryPrompt, FnSummaryPrompt, HybridCompressor, SlidingWindowCompressor,
        SummaryCompressor, SummaryPromptBuilder,
    };
    pub use crate::compression::{
        CompressionInput, CompressionOutput, ContextCompressor, ContextManager, ForceCompressStats,
    };
    pub use crate::error::Result;
    pub use crate::human_loop::{
        ConsoleHumanLoopProvider, HumanLoopProvider, HumanLoopRequest, HumanLoopResponse,
        WebSocketHumanLoopProvider, WebhookHumanLoopProvider,
    };
    pub use crate::llm::{JsonSchemaSpec, ResponseFormat};
    pub use crate::mcp::{McpManager, McpServerConfig, TransportConfig};
    pub use crate::memory::checkpointer::{Checkpointer, FileCheckpointer, InMemoryCheckpointer};
    pub use crate::memory::store::{FileStore, InMemoryStore, Store, StoreItem};
    pub use crate::skills::{
        Skill, SkillInfo, SkillManager,
        builtin::{CalculatorSkill, FileSystemSkill, ShellSkill, WeatherSkill},
        external::{LoadedSkill, ResourceRef, SkillLoader, SkillMeta},
    };
    pub use crate::testing::{FailingMockAgent, MockAgent, MockLlmClient, MockTool};
    pub use crate::tools::builtin::think::ThinkTool;
    pub use crate::tools::{Tool, ToolExecutionConfig, ToolParameters, ToolResult};
}
