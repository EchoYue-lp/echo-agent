#![doc = include_str!("../README.md")]
#![cfg_attr(docsrs, feature(doc_cfg))]

extern crate self as echo_agent;

// ── Core modules (always compiled) ──────────────────────────────────────────

pub mod agent;
pub mod audit;
pub mod compression;
pub mod config;
pub mod context;
pub mod error;
#[cfg(feature = "eval")]
#[cfg_attr(docsrs, doc(cfg(feature = "eval")))]
pub mod eval;
pub mod event_bus;
pub mod guard;
#[cfg(feature = "improve")]
#[cfg_attr(docsrs, doc(cfg(feature = "improve")))]
pub mod improve;
pub mod layered_compress;
pub mod llm;
pub mod memory;
pub mod notebook;
pub mod plugin;
pub mod retry;
pub mod sandbox;
pub mod security;
pub mod skills;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod tokenizer;
pub mod tools;
pub mod trace;
pub mod utils;
pub mod workflow;

// ── Optional modules (feature-gated) ────────────────────────────────────────

#[cfg(feature = "a2a")]
#[cfg_attr(docsrs, doc(cfg(feature = "a2a")))]
pub mod a2a;

#[cfg(feature = "channels")]
#[cfg_attr(docsrs, doc(cfg(feature = "channels")))]
pub mod channels;

#[cfg(feature = "handoff")]
#[cfg_attr(docsrs, doc(cfg(feature = "handoff")))]
pub mod handoff;

#[cfg(feature = "human-loop")]
#[cfg_attr(docsrs, doc(cfg(feature = "human-loop")))]
pub mod human_loop;

/// Unified hook bridge — connects task/subagent hooks to the central HookRegistry
pub mod hooks_bridge;

/// Agent types re-exported from echo-agents crate
pub use echo_agents;

#[cfg(feature = "mcp")]
#[cfg_attr(docsrs, doc(cfg(feature = "mcp")))]
pub mod mcp;

#[cfg(feature = "lsp")]
#[cfg_attr(docsrs, doc(cfg(feature = "lsp")))]
pub mod lsp;

#[cfg(feature = "tasks")]
#[cfg_attr(docsrs, doc(cfg(feature = "tasks")))]
pub mod tasks;

#[cfg(feature = "telemetry")]
#[cfg_attr(docsrs, doc(cfg(feature = "telemetry")))]
pub mod telemetry;

#[cfg(feature = "topology")]
#[cfg_attr(docsrs, doc(cfg(feature = "topology")))]
pub mod topology;

#[cfg(feature = "project-rules")]
pub use echo_core::project_rules;

// ── Declarative macros ──────────────────────────────────────────────────────

mod macros;

// ── Procedural macro re-exports ─────────────────────────────────────────────

pub use echo_macros::{
    Tool, audit_logger, callback, compressor, guard, handler, permission_policy, tool,
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

// ── Prelude ─────────────────────────────────────────────────────────────────

/// Common type re-exports.
///
/// Import everything with `use echo_agent::prelude::*`.
pub mod prelude {
    // Agent
    pub use crate::agent::{
        Agent, AgentCallback, AgentConfig, AgentEvent, AgentMode, AgentRole, CancellationToken,
        DefaultModeEngine, InterventionCallback, InterventionResult, LocalizedModeEngine,
        ModeConfig, ModeEngine, ReactAgent, ReactAgentBuilder, Runner, StepType, StructuredAgent,
    };
    // Prompt Template
    pub use echo_core::agent::PromptTemplateManager;
    // Config
    pub use crate::config::AppConfig;

    /// Convenience alias for [`ReactAgentBuilder`], the canonical builder type.
    pub type AgentBuilder = ReactAgentBuilder;

    // LLM
    pub use crate::llm::types::{ContentPart, ImageUrl, Message, MessageContent, Role, ToolCall};
    pub use crate::llm::{
        AnthropicClient, ChatChunk, ChatRequest, ChatResponse, JsonSchemaSpec, LlmClient,
        LlmConfig, LlmProvider, ModelProfile, OllamaClient, OpenAiClient, ProviderCapabilities,
        ProviderFactory, ResponseFormat, SimpleChatOptions, ToolDefinition,
    };

    // Tools
    pub use crate::tools::builtin::think::ThinkTool;
    pub use crate::tools::permission::{
        DefaultPermissionPolicy, PermissionDecision, PermissionPolicy, ToolPermission,
    };
    pub use crate::tools::{Tool, ToolExecutionConfig, ToolParameters, ToolResult, ToolRiskLevel, ToolStreamEvent};

    // Web Tools
    #[cfg(feature = "web")]
    #[cfg_attr(docsrs, doc(cfg(feature = "web")))]
    pub use crate::tools::web::{WebFetchTool, WebSearchTool};

    // Media Tools
    #[cfg(feature = "media")]
    #[cfg_attr(docsrs, doc(cfg(feature = "media")))]
    pub use crate::tools::media::{ImageFetchTool, WebFetchToolEnhanced};

    // Compression
    pub use crate::compression::compressor::{
        HybridCompressor, SlidingWindowCompressor, SummaryCompressor, default_summary_prompt,
    };
    pub use crate::compression::{
        CompressionInput, CompressionOutput, ContextCompressor, ContextManager, ForceCompressStats,
        PrepareResult,
    };

    // Tokenizer
    pub use crate::tokenizer::{HeuristicTokenizer, SimpleTokenizer, Tokenizer};

    // Memory
    #[cfg(feature = "sqlite")]
    #[cfg_attr(docsrs, doc(cfg(feature = "sqlite")))]
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
        hooks::{
            CompressHookStats, HookAction, HookContext, HookEvent, HookEventCategory, HookRegistry,
            HookResult, HookRule, HookSource, HooksDefinition, McpExecutorFn,
            UnifiedHookExecutorFn,
        },
    };

    // Guard
    #[cfg(feature = "content-guard")]
    pub use crate::guard::llm::LlmGuard;
    #[cfg(feature = "content-guard")]
    pub use crate::guard::rule::{RuleGuard, RuleGuardBuilder};
    pub use crate::guard::{Guard, GuardDirection, GuardManager, GuardResult};

    // Audit
    pub use crate::audit::{
        AuditCallback, AuditEvent, AuditEventType, AuditFilter, AuditLogger, FileAuditLogger,
        InMemoryAuditLogger,
    };

    // Workflow
    pub use crate::workflow::{
        ConcurrentWorkflow, DagWorkflow, DataPipelineConfig, Graph, GraphBuilder, GraphResult,
        SequentialWorkflow, SharedAgent, SharedState, StepOutput, Workflow, WorkflowDefinition,
        WorkflowEvent, WorkflowOutput, WritingPipelineConfig, run_data_pipeline,
        run_writing_pipeline, shared_agent,
    };

    // Sandbox
    pub use crate::sandbox::{
        DockerSandbox, ExecutionResult as SandboxResult, IsolationLevel, K8sSandbox, LocalSandbox,
        ResourceLimits, SandboxCommand, SandboxExecutor, SandboxManager, SandboxPolicy,
        SecurityLevel,
    };

    // Circuit Breaker
    pub use echo_core::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};

    // Retry
    pub use crate::retry::{RetryPolicy, with_retry, with_retry_if};

    // Error
    pub use crate::error::Result;

    // Trace
    pub use crate::trace::{
        ErrorPattern, InMemoryRunStore, JsonlRunStore, Run, RunEvent, RunStatus, RunStore,
        RunSummary, SessionSummary, TokenBreakdown, ToolUsageStats, TraceAnalyzer,
    };

    // Testing
    #[cfg(any(test, feature = "testing"))]
    pub use crate::testing::{FailingMockAgent, MockAgent, MockEmbedder, MockLlmClient, MockTool};
}

/// Advanced type re-exports for optional modules (requires corresponding features).
pub mod advanced {
    #[cfg(feature = "human-loop")]
    #[cfg_attr(docsrs, doc(cfg(feature = "human-loop")))]
    pub use crate::human_loop::{
        ApprovalDecision, ApprovalResponder, ConsoleHumanLoopProvider, HumanLoopEvent,
        HumanLoopHandler, HumanLoopManager, HumanLoopProvider, HumanLoopRequest, HumanLoopResponse,
        InputResponder, WebSocketHumanLoopProvider, WebhookHumanLoopProvider, dispatch_event,
    };

    #[cfg(feature = "mcp")]
    #[cfg_attr(docsrs, doc(cfg(feature = "mcp")))]
    pub use crate::mcp::{McpManager, McpServerConfig, McpTool, TransportConfig};

    #[cfg(feature = "channels")]
    #[cfg_attr(docsrs, doc(cfg(feature = "channels")))]
    pub use crate::channels::AgentChannelHandler;

    #[cfg(feature = "telemetry")]
    #[cfg_attr(docsrs, doc(cfg(feature = "telemetry")))]
    pub use crate::telemetry::{Metrics, TelemetryConfig, init_telemetry, shutdown_telemetry};

    #[cfg(feature = "handoff")]
    #[cfg_attr(docsrs, doc(cfg(feature = "handoff")))]
    pub use crate::handoff::{
        HandoffContext, HandoffManager, HandoffResult, HandoffTarget, HandoffTool,
    };

    #[cfg(feature = "plan-execute")]
    #[cfg_attr(docsrs, doc(cfg(feature = "plan-execute")))]
    pub use crate::agent::plan_execute::{
        ExecutionMode, Executor, LlmPlanner, Plan, PlanExecuteAgent, PlanStep, Planner,
        ReactExecutor, SimpleExecutor, StaticPlanner, StepResult, StepStatus,
    };

    #[cfg(feature = "a2a")]
    #[cfg_attr(docsrs, doc(cfg(feature = "a2a")))]
    pub use crate::a2a::{
        A2AClient, A2AServer, A2AStreamEvent, AgentCapabilities, AgentCard, AgentProvider,
        AgentSkill, JwtClaims, JwtConfig, TaskState, get_claims, serve, serve_from_config,
        serve_from_config_with_auth, serve_with_auth,
    };

    #[cfg(feature = "topology")]
    #[cfg_attr(docsrs, doc(cfg(feature = "topology")))]
    pub use crate::topology::{
        NodeType, TopologyCallback, TopologyData, TopologyEdge, TopologyNode, TopologyStats,
        TopologyTracker,
    };

    #[cfg(feature = "tasks")]
    #[cfg_attr(docsrs, doc(cfg(feature = "tasks")))]
    pub use crate::tasks::{Task, TaskManager, TaskStatus};

    #[cfg(feature = "self-reflection")]
    #[cfg_attr(docsrs, doc(cfg(feature = "self-reflection")))]
    pub use crate::agent::self_reflection::{
        CompositeCritic, CompositeStrategy, Critic, Critique, CritiqueOutput,
        InMemoryReflectionStore, LlmCritic, ReflectionExperience, ReflectionRecord,
        ReflectionStore, SelfReflectionAgent, StaticCritic, ThresholdCritic,
        critique_output_schema, default_refinement_prompt, default_reflection_prompt,
    };

    #[cfg(all(feature = "self-reflection", feature = "plan-execute"))]
    pub use crate::agent::self_reflection::ReflectiveExecutor;
}
