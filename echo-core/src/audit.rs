//! Audit logging core trait and event types

use crate::error::Result;
use crate::guard::GuardDirection;
use crate::tools::permission::ToolPermission;
use chrono::{DateTime, Utc};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Audit event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    /// Time when the event was recorded.
    #[serde(with = "crate::utils::time::local_rfc3339")]
    pub timestamp: DateTime<Utc>,
    /// Optional conversation or session identifier.
    pub session_id: Option<String>,
    /// Agent name associated with the event.
    pub agent_name: String,
    /// Structured payload describing the event kind.
    pub event_type: AuditEventType,
    /// Optional distributed tracing identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
}

impl AuditEvent {
    /// Construct a new event stamped with the current UTC time.
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

/// Audit event types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuditEventType {
    /// Raw user input captured before the agent starts processing.
    UserInput {
        /// User-provided content.
        content: String,
    },
    /// A model invocation with optional token accounting.
    LlmCall {
        /// Model name used for the request.
        model: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        /// Prompt token count if the provider reported it.
        prompt_tokens: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        /// Completion token count if the provider reported it.
        completion_tokens: Option<u64>,
    },
    /// A tool invocation and its observed outcome.
    ToolCall {
        /// Tool identifier.
        tool: String,
        /// Serialized tool input.
        input: Value,
        /// Tool output text.
        output: String,
        /// Whether the tool succeeded.
        success: bool,
        /// Execution duration in milliseconds.
        duration_ms: u64,
    },
    /// A guard rejected input or output.
    GuardBlock {
        /// Guard name.
        guard: String,
        /// Whether the guard acted on input or output.
        direction: GuardDirection,
        /// Human-readable reason.
        reason: String,
    },
    /// A permission policy denied a tool invocation.
    PermissionDenied {
        /// Tool identifier.
        tool: String,
        /// Permissions that triggered the denial.
        required: Vec<ToolPermission>,
        /// Human-readable reason.
        reason: String,
    },
    /// Final answer returned to the caller.
    FinalAnswer {
        /// Final response content.
        content: String,
    },
    /// Approval request was initiated
    ApprovalRequested {
        /// Tool name
        tool: String,
        /// Argument hash (for cache matching)
        args_hash: String,
        /// Risk level
        risk_level: String,
    },
    /// Approval decision was returned
    ApprovalCompleted {
        /// Tool name
        tool: String,
        /// Decision result
        decision: String,
        /// Approval scope
        scope: String,
        /// Rejection reason (if any)
        reason: Option<String>,
        /// Duration from request to decision (milliseconds)
        duration_ms: u64,
    },
}

/// 审计查询过滤器
#[derive(Debug, Default, Clone)]
pub struct AuditFilter {
    /// Restrict results to a specific session.
    pub session_id: Option<String>,
    /// Restrict results to a specific agent name.
    pub agent_name: Option<String>,
    /// Inclusive lower timestamp bound.
    pub from: Option<DateTime<Utc>>,
    /// Inclusive upper timestamp bound.
    pub to: Option<DateTime<Utc>>,
    /// Maximum number of results to return.
    pub limit: Option<usize>,
}

/// 审计日志记录器 trait
pub trait AuditLogger: Send + Sync {
    /// Persist one audit event.
    fn log<'a>(&'a self, event: AuditEvent) -> BoxFuture<'a, Result<()>>;
    /// Query stored events with a filter.
    fn query<'a>(&'a self, filter: AuditFilter) -> BoxFuture<'a, Result<Vec<AuditEvent>>>;
}
