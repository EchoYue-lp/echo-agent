//! Agent 核心 trait、事件和回调接口

use crate::error::{ReactError, Result};
use crate::llm::ToolDefinition;
use crate::llm::types::Message;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use serde_json::Value;
pub use tokio_util::sync::CancellationToken;

/// Agent 执行过程中产生的事件
///
/// 覆盖 Agent 生命周期的各个阶段，便于实现进度条、日志、UI 更新等。
#[derive(Debug)]
#[non_exhaustive]
#[allow(missing_docs)]
pub enum AgentEvent {
    // ── LLM 交互 ──────────────────────────────────────────────────────────
    /// LLM 正在生成 token（流式）
    Token(String),
    /// LLM 推理开始
    ThinkStart,
    /// LLM 推理结束
    #[allow(missing_docs)]
    ThinkEnd {
        prompt_tokens: usize,
        completion_tokens: usize,
    },

    // ── 工具调用 ──────────────────────────────────────────────────────────
    /// 准备调用工具
    ToolCall { name: String, args: Value },
    /// 工具执行完毕
    ToolResult { name: String, output: String },
    /// 工具执行出错
    ToolError { name: String, error: String },

    // ── 步骤级事件 ────────────────────────────────────────────────────────
    /// Plan-and-Execute 引擎生成了计划
    PlanGenerated { steps: Vec<String> },
    /// 计划步骤开始执行
    StepStart {
        step_index: usize,
        description: String,
    },
    /// 计划步骤执行结束
    StepEnd { step_index: usize, success: bool },

    // ── 护栏 & 安全 ──────────────────────────────────────────────────────
    /// 护栏被触发
    GuardTriggered { guard: String, blocked: bool },

    // ── 记忆 & 编排 ──────────────────────────────────────────────────────
    /// 长期记忆已召回
    MemoryRecalled { count: usize },
    /// Agent 间 Handoff 开始
    HandoffStart { from: String, to: String },
    /// Agent 间 Handoff 结束
    HandoffEnd { to: String },

    // ── 自省反思 ──────────────────────────────────────────────────────────
    /// 反思迭代开始
    ReflectionStart { iteration: usize },
    /// 反思迭代结束
    ReflectionEnd {
        iteration: usize,
        score: f64,
        passed: bool,
    },
    /// 评估者生成了评价结果
    CritiqueGenerated {
        score: f64,
        passed: bool,
        feedback: String,
    },
    /// 正在基于反思修正回答
    Refining { iteration: usize },

    // ── 终态 ──────────────────────────────────────────────────────────────
    /// 最终回答
    FinalAnswer(String),
    /// 被取消
    Cancelled,
}

impl AgentEvent {
    /// Return prompt token count for `ThinkEnd`.
    pub fn prompt_tokens(&self) -> Option<usize> {
        match self {
            AgentEvent::ThinkEnd { prompt_tokens, .. } => Some(*prompt_tokens),
            _ => None,
        }
    }

    /// Return completion token count for `ThinkEnd`.
    pub fn completion_tokens(&self) -> Option<usize> {
        match self {
            AgentEvent::ThinkEnd {
                completion_tokens, ..
            } => Some(*completion_tokens),
            _ => None,
        }
    }

    /// Return total token usage for `ThinkEnd`.
    pub fn total_tokens(&self) -> Option<usize> {
        match self {
            AgentEvent::ThinkEnd {
                prompt_tokens,
                completion_tokens,
            } => Some(prompt_tokens + completion_tokens),
            _ => None,
        }
    }

    /// Compatibility helper for older call sites that tracked a single token count.
    pub fn tokens_used(&self) -> Option<usize> {
        self.total_tokens()
    }
}

/// LLM 响应解析后的步骤类型
#[derive(Debug)]
#[allow(missing_docs)]
pub enum StepType {
    Thought(String),
    Call {
        tool_call_id: String,
        function_name: String,
        arguments: Value,
    },
}

/// Agent 统一执行接口
///
/// 约定一个可变借用驱动的执行模型，便于 Agent 在内部维护对话状态、
/// 工具缓存或连接句柄，同时让工作流层可以通过 `Mutex` 安全串行化访问。
pub trait Agent: Send + Sync {
    /// Human-readable agent name used in logs, events, and orchestration.
    fn name(&self) -> &str;
    /// Model identifier currently bound to the agent.
    fn model_name(&self) -> &str;
    /// System prompt that seeds the agent's behavior.
    fn system_prompt(&self) -> &str;

    /// Names of tools currently exposed to the model.
    fn tool_names(&self) -> Vec<String> {
        vec![]
    }

    /// Tool definitions serialized into LLM requests.
    fn tool_definitions(&self) -> Vec<ToolDefinition> {
        vec![]
    }

    /// Human-readable skill identifiers available to this agent.
    fn skill_names(&self) -> Vec<String> {
        vec![]
    }

    /// Configured MCP server identifiers available to this agent.
    fn mcp_server_names(&self) -> Vec<String> {
        vec![]
    }

    /// Release external resources before dropping the agent.
    fn close<'a>(&'a mut self) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }

    /// Execute a task and return the final answer.
    fn execute<'a>(&'a mut self, task: &'a str) -> BoxFuture<'a, Result<String>>;

    /// Execute a task and stream lifecycle events.
    fn execute_stream<'a>(
        &'a mut self,
        task: &'a str,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>>;

    /// Execute a task with cooperative cancellation support.
    fn execute_stream_with_cancel<'a>(
        &'a mut self,
        task: &'a str,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        let _ = cancel;
        self.execute_stream(task)
    }

    /// Alias of [`Self::execute`] for chat-centric call sites.
    fn chat<'a>(&'a mut self, message: &'a str) -> BoxFuture<'a, Result<String>> {
        self.execute(message)
    }

    /// Alias of [`Self::execute_stream`] for chat-centric call sites.
    fn chat_stream<'a>(
        &'a mut self,
        message: &'a str,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        self.execute_stream(message)
    }

    /// Chat streaming variant with cooperative cancellation support.
    fn chat_stream_with_cancel<'a>(
        &'a mut self,
        message: &'a str,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        let _ = cancel;
        self.chat_stream(message)
    }

    /// Reset in-memory conversational state.
    fn reset(&mut self) {}
}

/// Agent 生命周期回调接口
pub trait AgentCallback: Send + Sync {
    /// Called before the model starts a reasoning step.
    fn on_think_start<'a>(
        &'a self,
        _agent: &'a str,
        _messages: &'a [Message],
    ) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }

    /// Called after the model reasoning step is parsed into logical steps.
    fn on_think_end<'a>(&'a self, _agent: &'a str, _steps: &'a [StepType]) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }

    /// Called before a tool invocation begins.
    fn on_tool_start<'a>(
        &'a self,
        _agent: &'a str,
        _tool: &'a str,
        _args: &'a Value,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }

    /// Called after a tool invocation succeeds.
    fn on_tool_end<'a>(
        &'a self,
        _agent: &'a str,
        _tool: &'a str,
        _result: &'a str,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }

    /// Called when a tool invocation fails.
    fn on_tool_error<'a>(
        &'a self,
        _agent: &'a str,
        _tool: &'a str,
        _err: &'a ReactError,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }

    /// Called when the agent emits its final answer.
    fn on_final_answer<'a>(&'a self, _agent: &'a str, _answer: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }

    /// Called at the end of each outer control-loop iteration.
    fn on_iteration<'a>(&'a self, _agent: &'a str, _iteration: usize) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }
}
