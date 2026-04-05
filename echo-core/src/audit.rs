//! 审计日志核心 trait 和事件类型

use crate::error::Result;
use crate::guard::GuardDirection;
use crate::tools::permission::ToolPermission;
use chrono::{DateTime, Utc};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 审计事件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    pub timestamp: DateTime<Utc>,
    pub session_id: Option<String>,
    pub agent_name: String,
    pub event_type: AuditEventType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
}

impl AuditEvent {
    pub fn now(session_id: Option<String>, agent_name: String, event_type: AuditEventType) -> Self {
        Self {
            timestamp: Utc::now(),
            session_id,
            agent_name,
            event_type,
            trace_id: None,
        }
    }
}

/// 审计事件类型
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuditEventType {
    UserInput {
        content: String,
    },
    LlmCall {
        model: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        prompt_tokens: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        completion_tokens: Option<u64>,
    },
    ToolCall {
        tool: String,
        input: Value,
        output: String,
        success: bool,
        duration_ms: u64,
    },
    GuardBlock {
        guard: String,
        direction: GuardDirection,
        reason: String,
    },
    PermissionDenied {
        tool: String,
        required: Vec<ToolPermission>,
        reason: String,
    },
    FinalAnswer {
        content: String,
    },
    /// 审批请求已发起
    ApprovalRequested {
        /// 工具名称
        tool: String,
        /// 参数哈希（用于匹配缓存）
        args_hash: String,
        /// 风险等级
        risk_level: String,
    },
    /// 审批决策已返回
    ApprovalCompleted {
        /// 工具名称
        tool: String,
        /// 决策结果
        decision: String,
        /// 审批范围
        scope: String,
        /// 拒绝原因（如果有）
        reason: Option<String>,
        /// 从请求到决策的耗时（毫秒）
        duration_ms: u64,
    },
}

/// 审计查询过滤器
#[derive(Debug, Default, Clone)]
pub struct AuditFilter {
    pub session_id: Option<String>,
    pub agent_name: Option<String>,
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
    pub limit: Option<usize>,
}

/// 审计日志记录器 trait
pub trait AuditLogger: Send + Sync {
    fn log<'a>(&'a self, event: AuditEvent) -> BoxFuture<'a, Result<()>>;
    fn query<'a>(&'a self, filter: AuditFilter) -> BoxFuture<'a, Result<Vec<AuditEvent>>>;
}
