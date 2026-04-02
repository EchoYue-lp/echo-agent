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
pub enum AgentEvent {
    // ── LLM 交互 ──────────────────────────────────────────────────────────
    /// LLM 正在生成 token（流式）
    Token(String),
    /// LLM 推理开始
    ThinkStart,
    /// LLM 推理结束
    ThinkEnd { tokens_used: usize },

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

    // ── 终态 ──────────────────────────────────────────────────────────────
    /// 最终回答
    FinalAnswer(String),
    /// 被取消
    Cancelled,
}

/// LLM 响应解析后的步骤类型
#[derive(Debug)]
pub enum StepType {
    Thought(String),
    Call {
        tool_call_id: String,
        function_name: String,
        arguments: Value,
    },
}

/// Agent 统一执行接口
pub trait Agent: Send + Sync {
    fn name(&self) -> &str;
    fn model_name(&self) -> &str;
    fn system_prompt(&self) -> &str;

    fn tool_names(&self) -> Vec<String> {
        vec![]
    }

    fn tool_definitions(&self) -> Vec<ToolDefinition> {
        vec![]
    }

    fn skill_names(&self) -> Vec<String> {
        vec![]
    }

    fn mcp_server_names(&self) -> Vec<String> {
        vec![]
    }

    fn close(&mut self) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }

    fn execute<'a>(&'a mut self, task: &'a str) -> BoxFuture<'a, Result<String>>;

    fn execute_stream<'a>(
        &'a mut self,
        task: &'a str,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>>;

    fn execute_stream_with_cancel<'a>(
        &'a mut self,
        task: &'a str,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        let _ = cancel;
        self.execute_stream(task)
    }

    fn chat<'a>(&'a mut self, message: &'a str) -> BoxFuture<'a, Result<String>> {
        self.execute(message)
    }

    fn chat_stream<'a>(
        &'a mut self,
        message: &'a str,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        self.execute_stream(message)
    }

    fn chat_stream_with_cancel<'a>(
        &'a mut self,
        message: &'a str,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        let _ = cancel;
        self.chat_stream(message)
    }

    fn reset(&mut self) {}
}

/// Agent 生命周期回调接口
pub trait AgentCallback: Send + Sync {
    fn on_think_start<'a>(
        &'a self,
        _agent: &'a str,
        _messages: &'a [Message],
    ) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }

    fn on_think_end<'a>(&'a self, _agent: &'a str, _steps: &'a [StepType]) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }

    fn on_tool_start<'a>(
        &'a self,
        _agent: &'a str,
        _tool: &'a str,
        _args: &'a Value,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }

    fn on_tool_end<'a>(
        &'a self,
        _agent: &'a str,
        _tool: &'a str,
        _result: &'a str,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }

    fn on_tool_error<'a>(
        &'a self,
        _agent: &'a str,
        _tool: &'a str,
        _err: &'a ReactError,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }

    fn on_final_answer<'a>(&'a self, _agent: &'a str, _answer: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }

    fn on_iteration<'a>(&'a self, _agent: &'a str, _iteration: usize) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }
}
