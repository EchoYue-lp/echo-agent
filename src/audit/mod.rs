//! 审计日志
//!
//! 完整记录 tool 调用链、护栏阻断、权限拒绝等事件，支持合规审查。
//!
//! # 核心类型
//!
//! - [`AuditEvent`][]: 审计事件
//! - [`AuditLogger`]: 日志记录器 trait
//! - [`AuditCallback`]: 基于 `AgentCallback` 的自动审计

pub mod file;
pub mod memory;

pub use echo_core::audit::*;

use futures::future::BoxFuture;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// 基于 `AgentCallback` 的审计日志自动记录器
///
/// 实现 `AgentCallback`，将所有回调事件自动写入 `AuditLogger`。
///
/// # 示例
///
/// ```rust,no_run
/// use echo_agent::audit::{AuditCallback, memory::InMemoryAuditLogger};
/// use echo_agent::prelude::*;
/// use std::sync::Arc;
///
/// let logger = Arc::new(InMemoryAuditLogger::new());
/// let audit_cb = Arc::new(AuditCallback::new(logger, "my-agent", None));
///
/// let agent = ReactAgentBuilder::new()
///     .model("qwen3-max")
///     .callback(audit_cb)
///     .build();
/// ```
pub struct AuditCallback {
    logger: Arc<dyn AuditLogger>,
    agent_name: String,
    session_id: Option<String>,
    tool_start_times: Mutex<HashMap<String, std::time::Instant>>,
}

impl AuditCallback {
    pub fn new(
        logger: Arc<dyn AuditLogger>,
        agent_name: impl Into<String>,
        session_id: Option<String>,
    ) -> Self {
        Self {
            logger,
            agent_name: agent_name.into(),
            session_id,
            tool_start_times: Mutex::new(HashMap::new()),
        }
    }

    fn make_event(&self, event_type: AuditEventType) -> AuditEvent {
        AuditEvent::now(self.session_id.clone(), self.agent_name.clone(), event_type)
    }
}

impl crate::agent::AgentCallback for AuditCallback {
    fn on_tool_start<'a>(
        &'a self,
        _agent: &'a str,
        tool: &'a str,
        _args: &'a Value,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            if let Ok(mut times) = self.tool_start_times.lock() {
                times.insert(tool.to_string(), std::time::Instant::now());
            }
        })
    }

    fn on_tool_end<'a>(
        &'a self,
        _agent: &'a str,
        tool: &'a str,
        result: &'a str,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let duration_ms = self
                .tool_start_times
                .lock()
                .ok()
                .and_then(|mut m| m.remove(tool))
                .map(|t| t.elapsed().as_millis() as u64)
                .unwrap_or(0);

            let event = self.make_event(AuditEventType::ToolCall {
                tool: tool.to_string(),
                input: Value::Null,
                output: result.to_string(),
                success: true,
                duration_ms,
            });
            let _ = self.logger.log(event).await;
        })
    }

    fn on_tool_error<'a>(
        &'a self,
        _agent: &'a str,
        tool: &'a str,
        err: &'a crate::error::ReactError,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let duration_ms = self
                .tool_start_times
                .lock()
                .ok()
                .and_then(|mut m| m.remove(tool))
                .map(|t| t.elapsed().as_millis() as u64)
                .unwrap_or(0);

            let event = self.make_event(AuditEventType::ToolCall {
                tool: tool.to_string(),
                input: Value::Null,
                output: err.to_string(),
                success: false,
                duration_ms,
            });
            let _ = self.logger.log(event).await;
        })
    }

    fn on_final_answer<'a>(&'a self, _agent: &'a str, answer: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let event = self.make_event(AuditEventType::FinalAnswer {
                content: answer.to_string(),
            });
            let _ = self.logger.log(event).await;
        })
    }
}
