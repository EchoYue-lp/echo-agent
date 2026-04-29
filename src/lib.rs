//! # echo-agent
//!
//! 一个通用、易用、高性能的 Rust AI Agent 开发框架。
//!
//! ## Workspace 结构
//!
//! | Crate | 职责 |
//! |-------|------|
//! | **echo-core** | 核心 trait 与类型（Tool、LlmClient、Agent、Guard、Error） |
//! | **echo-macros** | 过程宏（`#[tool]`、`#[callback]`、`#[guard]`、`#[handler]` 等） |
//! | **echo-execution** | 执行层：沙箱、技能、工具管理 |
//! | **echo-integration** | 外部集成：LLM 客户端、MCP 协议、IM 通道 |
//! | **echo-state** | 状态层：记忆、压缩、审计 |
//! | **echo-orchestration** | 编排层：工作流、人工审批、任务规划 |
//! | **echo-agent** | 主 crate —— 重新导出所有公共 API，提供 builder / runner |
//!
//! ## 快速开始
//!
//! ```rust,no_run
//! use echo_agent::prelude::*;
//!
//! # #[tokio::main]
//! # async fn main() -> echo_agent::error::Result<()> {
//! let mut agent = ReactAgentBuilder::simple("qwen3-max", "你是一个有帮助的助手")?;
//! let answer = agent.chat("你好！").await?;
//! println!("Agent: {}", answer);
//! # Ok(())
//! # }
//! ```
//!
//! ## 使用宏快速构建
//!
//! ```rust,no_run
//! use echo_agent::prelude::*;
//! use echo_agent::{agent, tool};
//!
//! #[tool(name = "add", description = "两数相加")]
//! async fn add(a: f64, b: f64) -> Result<ToolResult> {
//!     Ok(ToolResult::success(format!("{}", a + b)))
//! }
//!
//! # #[tokio::main]
//! # async fn main() -> echo_agent::error::Result<()> {
//! let mut agent = agent! {
//!     model: "qwen3-max",
//!     system_prompt: "你是计算助手",
//!     tools: [AddTool],
//! }?;
//! let answer = agent.execute("3 + 7 = ?").await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## 模块概览
//!
//! | 模块 | 能力 |
//! |------|------|
//! | [`agent`] | Agent 引擎（ReAct / Plan-Execute / Self-Reflection / Subagent） |
//! | [`llm`] | LLM 客户端（OpenAI / Anthropic / Ollama） |
//! | [`tools`] | 工具系统（Tool trait、并发限流、超时重试） |
//! | [`memory`] | 双层记忆（Checkpointer + Store） |
//! | [`compression`] | 上下文压缩 |
//! | [`human_loop`] | 人工介入（审批/输入） |
//! | [`guard`] | 护栏系统（规则 / LLM 内容过滤） |
//! | [`audit`] | 审计日志 |
//! | [`skills`] | Skill 系统（Tool + Prompt 包） |
//! | [`mcp`] | MCP 协议客户端 |
//! | [`tasks`] | DAG 任务管理 |
//! | [`handoff`] | Agent 间 Handoff |
//! | [`agent::plan_execute`] | Plan-and-Execute 引擎 |
//! | [`a2a`] | A2A 协议 |
//! | [`topology`] | Agent 拓扑可视化 |
//! | [`workflow`] | 图工作流引擎（Graph + SharedState + Sequential/Concurrent/DAG） |
//! | [`telemetry`] | OpenTelemetry 追踪与指标 |
//! | [`error`] | 统一错误类型 |

extern crate self as echo_agent;

// ── 核心模块（始终编译） ─────────────────────────────────────────────────────

pub mod agent;
pub mod audit;
pub mod compression;
pub mod config;
pub mod error;
pub mod guard;
pub mod llm;
pub mod memory;
pub mod retry;
pub mod sandbox;
pub mod skills;
pub mod testing;
pub mod tokenizer;
pub mod tools;
pub mod utils;
pub mod workflow;

// ── 可选模块（feature-gated） ────────────────────────────────────────────────

#[cfg(feature = "a2a")]
pub mod a2a;
#[cfg(feature = "channels")]
pub mod channels;
#[cfg(feature = "handoff")]
pub mod handoff;
#[cfg(feature = "human-loop")]
pub mod human_loop;
#[cfg(feature = "mcp")]
pub mod mcp;
#[cfg(feature = "tasks")]
pub mod tasks;
#[cfg(feature = "telemetry")]
pub mod telemetry;
#[cfg(feature = "topology")]
pub mod topology;

// Re-export project_rules from echo_core when feature enabled
#[cfg(feature = "project-rules")]
pub use echo_core::project_rules;

// ── 声明式宏 ─────────────────────────────────────────────────────────────────

mod macros;

// ── 过程宏 re-export ─────────────────────────────────────────────────────────

pub use echo_macros::{
    audit_logger, callback, compressor, guard, handler, permission_policy, tool,
};

/// Direct access to split workspace crates during migration.
///
/// This keeps `echo_agent` usable as a facade while still giving callers an
/// explicit path to the underlying crate APIs when they need to avoid facade
/// drift or migrate imports incrementally.
pub mod workspace {
    pub use echo_core as core;
    pub use echo_execution as execution;
    pub use echo_integration as integration;
    pub use echo_orchestration as orchestration;
    pub use echo_state as state;
}

// ── Prelude ──────────────────────────────────────────────────────────────────

/// 常用类型导出
///
/// 包含最常用的核心类型，通过 `use echo_agent::prelude::*` 导入。
pub mod prelude {
    // Agent
    pub use crate::agent::{
        Agent, AgentCallback, AgentConfig, AgentEvent, AgentRole, CancellationToken, ReactAgent,
        ReactAgentBuilder, Runner, StepType, StructuredAgent,
    };
    // Config
    pub use crate::config::AppConfig;

    /// AgentBuilder 是 ReactAgentBuilder 的别名（向后兼容）
    #[allow(deprecated)]
    pub type AgentBuilder = ReactAgentBuilder;

    // LLM
    pub use crate::llm::types::{ContentPart, ImageUrl, Message, MessageContent, ToolCall};
    pub use crate::llm::{
        AnthropicClient, ChatChunk, ChatRequest, ChatResponse, JsonSchemaSpec, LlmClient,
        LlmConfig, LlmProvider, OllamaClient, OpenAiClient, ProviderFactory, ResponseFormat,
        ToolDefinition,
    };

    // Tools
    pub use crate::tools::builtin::think::ThinkTool;
    pub use crate::tools::permission::{
        DefaultPermissionPolicy, PermissionDecision, PermissionPolicy, ToolPermission,
    };
    pub use crate::tools::{Tool, ToolExecutionConfig, ToolParameters, ToolResult};

    // Web Tools
    #[cfg(feature = "web")]
    pub use crate::tools::web::{WebFetchTool, WebSearchTool};

    // Media Tools
    #[cfg(feature = "media")]
    pub use crate::tools::media::{ImageFetchTool, WebFetchToolEnhanced};

    // Compression
    pub use crate::compression::compressor::{
        DefaultSummaryPrompt, FnSummaryPrompt, HybridCompressor, SlidingWindowCompressor,
        SummaryCompressor, SummaryPromptBuilder,
    };
    pub use crate::compression::{
        CompressionInput, CompressionOutput, ContextCompressor, ContextManager, ForceCompressStats,
    };

    // Tokenizer
    pub use crate::tokenizer::{HeuristicTokenizer, SimpleTokenizer, Tokenizer};

    // Memory
    #[cfg(feature = "sqlite")]
    pub use crate::memory::SqliteStore;
    pub use crate::memory::{
        Checkpointer, Embedder, EmbeddingStore, FileCheckpointer, FileStore, HttpEmbedder,
        InMemoryCheckpointer, InMemoryStore, SnapshotManager, SnapshotPolicy, StateSnapshot, Store,
        StoreItem,
    };

    // Skills
    pub use crate::skills::{
        Skill, SkillInfo, SkillRegistry,
        builtin::{FileSystemSkill, ShellSkill},
        external::{
            ActivateSkillTool, DiscoveryScope, PromptContext, ReadSkillResourceTool,
            RunSkillScriptTool, SkillContent, SkillDescriptor, SkillLoader, SkillResourceEntry,
            SkillResourceKind, SkillSource,
        },
        hooks::{HookAction, HookEvent, HookRegistry, HookResult, HookRule, HooksDefinition},
    };

    // Skills — backward compatibility
    #[allow(deprecated)]
    pub use crate::skills::SkillManager;

    // Guard
    pub use crate::guard::llm::LlmGuard;
    pub use crate::guard::rule::{RuleGuard, RuleGuardBuilder};
    pub use crate::guard::{Guard, GuardDirection, GuardManager, GuardResult};

    // Audit
    pub use crate::audit::{
        AuditCallback, AuditEvent, AuditEventType, AuditFilter, AuditLogger, FileAuditLogger,
        InMemoryAuditLogger,
    };

    // Workflow
    pub use crate::workflow::{
        ConcurrentWorkflow, DagWorkflow, Graph, GraphBuilder, GraphResult, SequentialWorkflow,
        SharedAgent, SharedState, StepOutput, Workflow, WorkflowDefinition, WorkflowEvent,
        WorkflowOutput, shared_agent,
    };

    // Sandbox
    pub use crate::sandbox::{
        DockerSandbox, ExecutionResult as SandboxResult, IsolationLevel, K8sSandbox, LocalSandbox,
        ResourceLimits, SandboxCommand, SandboxExecutor, SandboxManager, SandboxPolicy,
        SecurityLevel,
    };

    // Retry
    pub use crate::retry::{RetryPolicy, with_retry, with_retry_if};

    // Error
    pub use crate::error::Result;

    // Testing
    pub use crate::testing::{FailingMockAgent, MockAgent, MockEmbedder, MockLlmClient, MockTool};
}

/// 进阶类型导出（可选模块的类型，需启用对应 feature）
pub mod advanced {
    #[cfg(feature = "human-loop")]
    pub use crate::human_loop::{
        ApprovalDecision, ApprovalResponder, ConsoleHumanLoopProvider, HumanLoopEvent,
        HumanLoopHandler, HumanLoopManager, HumanLoopProvider, HumanLoopRequest, HumanLoopResponse,
        InputResponder, WebSocketHumanLoopProvider, WebhookHumanLoopProvider, dispatch_event,
    };

    #[cfg(feature = "mcp")]
    pub use crate::mcp::{McpManager, McpServerConfig, McpTool, TransportConfig};

    #[cfg(feature = "channels")]
    pub use crate::channels::AgentChannelHandler;

    #[cfg(feature = "telemetry")]
    pub use crate::telemetry::{Metrics, TelemetryConfig, init_telemetry, shutdown_telemetry};

    #[cfg(feature = "handoff")]
    pub use crate::handoff::{
        HandoffContext, HandoffManager, HandoffResult, HandoffTarget, HandoffTool,
    };

    #[cfg(feature = "plan-execute")]
    pub use crate::agent::plan_execute::{
        ExecutionMode, Executor, LlmPlanner, Plan, PlanExecuteAgent, PlanStep, Planner,
        ReactExecutor, SimpleExecutor, StaticPlanner, StepResult, StepStatus,
    };

    #[cfg(feature = "a2a")]
    pub use crate::a2a::{
        A2AClient, A2AServer, A2AStreamEvent, AgentCapabilities, AgentCard, AgentProvider,
        AgentSkill, JwtClaims, JwtConfig, TaskState, get_claims, serve, serve_from_config,
        serve_from_config_with_auth, serve_with_auth,
    };

    #[cfg(feature = "topology")]
    pub use crate::topology::{
        NodeType, TopologyCallback, TopologyData, TopologyEdge, TopologyNode, TopologyStats,
        TopologyTracker,
    };

    #[cfg(feature = "tasks")]
    pub use crate::tasks::{Task, TaskManager, TaskStatus};

    #[cfg(feature = "self-reflection")]
    pub use crate::agent::self_reflection::{
        CompositeCritic, CompositeStrategy, Critic, DefaultRefinementPromptBuilder,
        DefaultReflectionPromptBuilder, InMemoryReflectionStore, LlmCritic,
        RefinementPromptBuilder, ReflectionExperience, ReflectionPromptBuilder, ReflectionRecord,
        ReflectionStore, SelfReflectionAgent, StaticCritic, ThresholdCritic,
        critique_output_schema,
    };

    #[cfg(all(feature = "self-reflection", feature = "plan-execute"))]
    pub use crate::agent::self_reflection::ReflectiveExecutor;
}
