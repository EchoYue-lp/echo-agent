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

/// 存储工具调用开始时的信息
struct ToolCallInfo {
    args: Value,
    started_at: std::time::Instant,
}

/// 基于 `AgentCallback` 的审计日志自动记录器
///
/// 实现 `AgentCallback`，将所有回调事件自动写入 `AuditLogger`。
///
/// # 示例
///
/// ```rust
/// use echo_state::audit::{memory::InMemoryAuditLogger, AuditCallback};
/// use std::sync::Arc;
///
/// let logger = Arc::new(InMemoryAuditLogger::new());
/// let audit_cb = Arc::new(AuditCallback::new(logger, "my-agent", None));
/// // 将 `audit_cb` 接入你自己的 agent/runtime 层，或通过 `echo_agent` façade 使用。
/// let _ = audit_cb;
/// ```
pub struct AuditCallback {
    logger: Arc<dyn AuditLogger>,
    agent_name: String,
    session_id: Option<String>,
    /// tool_call_id → ToolCallInfo（args + start time）
    tool_calls: Mutex<HashMap<String, ToolCallInfo>>,
    /// 按 tool_name 维护调用序列号，用于生成 tool_call_id
    tool_seq: Mutex<HashMap<String, u64>>,
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
            tool_calls: Mutex::new(HashMap::new()),
            tool_seq: Mutex::new(HashMap::new()),
        }
    }

    /// 为工具调用生成唯一的 tool_call_id
    fn make_tool_call_id(&self, tool: &str) -> String {
        let mut seq = self.tool_seq.lock().unwrap_or_else(|e| e.into_inner());
        let n = seq.entry(tool.to_string()).or_insert(0);
        *n += 1;
        format!("{}#{}", tool, n)
    }

    fn make_event(&self, event_type: AuditEventType) -> AuditEvent {
        AuditEvent::now(self.session_id.clone(), self.agent_name.clone(), event_type)
    }
}

impl echo_core::agent::AgentCallback for AuditCallback {
    fn on_tool_start<'a>(
        &'a self,
        _agent: &'a str,
        tool: &'a str,
        args: &'a Value,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let tool_call_id = self.make_tool_call_id(tool);
            if let Ok(mut calls) = self.tool_calls.lock() {
                calls.insert(
                    tool_call_id,
                    ToolCallInfo {
                        args: args.clone(),
                        started_at: std::time::Instant::now(),
                    },
                );
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
            let (duration_ms, input) = self
                .tool_calls
                .lock()
                .ok()
                .and_then(|mut m| {
                    // Try to find by tool_call_id pattern first (tool#N)
                    let key = m
                        .keys()
                        .find(|k| k.starts_with(&format!("{}#", tool)))
                        .cloned();
                    key.and_then(|k| m.remove(&k))
                })
                .map(|info| (info.started_at.elapsed().as_millis() as u64, info.args))
                .unwrap_or((0, Value::Null));

            let event = self.make_event(AuditEventType::ToolCall {
                tool: tool.to_string(),
                input,
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
        err: &'a echo_core::error::ReactError,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let (duration_ms, input) = self
                .tool_calls
                .lock()
                .ok()
                .and_then(|mut m| {
                    let key = m
                        .keys()
                        .find(|k| k.starts_with(&format!("{}#", tool)))
                        .cloned();
                    key.and_then(|k| m.remove(&k))
                })
                .map(|info| (info.started_at.elapsed().as_millis() as u64, info.args))
                .unwrap_or((0, Value::Null));

            let event = self.make_event(AuditEventType::ToolCall {
                tool: tool.to_string(),
                input,
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
