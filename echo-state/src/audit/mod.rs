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
use std::sync::atomic::{AtomicU64, Ordering};
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
    /// Monotonic counter for generating unique tool_call_ids.
    /// Using a global counter instead of a per-tool-name sequence avoids
    /// prefix-collision in the lookup when concurrent calls share a tool name.
    next_call_id: AtomicU64,
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
            next_call_id: AtomicU64::new(1),
        }
    }

    /// 为工具调用生成唯一的 tool_call_id（全局单调递增）。
    fn make_tool_call_id(&self, _tool: &str) -> String {
        let n = self.next_call_id.fetch_add(1, Ordering::Relaxed);
        format!("call_{}", n)
    }

    fn make_event(&self, event_type: AuditEventType) -> AuditEvent {
        AuditEvent::now(self.session_id.clone(), self.agent_name.clone(), event_type)
    }

    /// Look up and remove the oldest in-flight tool call by tool name.
    ///
    /// Uses an exact-prefix match on the tool-in-key pattern (not
    /// iteration-order-dependent), then removes and returns the entry
    /// with the smallest embedded sequence number.  This is deterministic
    /// even when concurrent calls share a tool name.
    fn pop_tool_call(&self, tool: &str) -> Option<ToolCallInfo> {
        let mut map = self.tool_calls.lock().ok()?;
        // Collect all keys matching this tool's tracking prefix.
        // Previous format: "tool#N" — fallback for entries still using that pattern.
        let candidates: Vec<String> = map
            .keys()
            .filter(|k| k.starts_with(&format!("{}#", tool)))
            .cloned()
            .collect();
        if candidates.is_empty() {
            return None;
        }
        // Remove the entry with the smallest sequence number (oldest).
        let key = candidates.into_iter().min_by_key(|k| {
            k.split('#')
                .nth(1)
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(u64::MAX)
        })?;
        map.remove(&key)
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
                .pop_tool_call(tool)
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
                .pop_tool_call(tool)
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
