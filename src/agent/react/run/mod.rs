//! ReactAgent 执行引擎
//!
//! 包含 ReAct 循环的所有内部实现：
//! - `retry` — LLM 重试逻辑 + 工具并发超时计算
//! - `context` — 上下文管理 + 长期记忆 + 持久化 + 审计
//! - `approval` — 工具执行审批（人工介入）
//! - `execution` — 工具执行（调用、护栏、截断）
//! - `react_loop` — ReAct 循环核心（think / process_steps / run_react_loop）
//! - `direct` — 直接执行入口（run_direct / run_chat_direct）
//! - `stream_loop` — 流式执行循环

pub(crate) mod approval;
pub(crate) mod context;
pub(crate) mod direct;
pub(crate) mod execution;
pub(crate) mod react_loop;
pub(crate) mod retry;
pub(crate) mod stream_loop;
pub(crate) use stream_loop::StreamMode;
