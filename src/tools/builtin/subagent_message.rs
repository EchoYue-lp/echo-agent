//! In-tree Subagent communication tools.
//!
//! `subagent_message` sends one bounded message from a running Subagent to
//! its parent (informational report or blocking clarification request) or to
//! a sibling attempt (queue-only steer), through the uplink channel the
//! dispatcher installed into the invocation's runtime context. Delivery is
//! fire-and-forget: the sender never waits for a reply, so parent/child
//! mutual waiting cannot deadlock a dispatch tree.
//!
//! `subagent_list` returns the bounded set of currently active attempts in
//! the shared control plane so the model can discover sibling execution ids
//! to address.
//!
//! Both tools are no-ops-with-explanation outside a dispatched Subagent
//! invocation (no lineage / no uplink in the ToolContext) instead of failing
//! hard, so a mis-registered agent still produces model-readable feedback.

use crate::agent::subagent::registry::SubagentRegistry;
use crate::tools::{Tool, ToolParameters, ToolResult};
use echo_core::tools::{
    SubagentUplinkKind, SubagentUplinkMessage, SubagentUplinkTarget, ToolContext,
};
use futures::future::BoxFuture;
use serde_json::{Value, json};

/// Upper bound for one message body, in Unicode scalar values.
const MAX_MESSAGE_CHARS: usize = 8_000;
/// Default (and upper) bound for `subagent_list` output size.
const DEFAULT_LIST_LIMIT: usize = 20;

fn bounded_text(value: &str) -> String {
    value.chars().take(MAX_MESSAGE_CHARS).collect()
}

pub struct SubagentMessageTool;

impl SubagentMessageTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SubagentMessageTool {
    fn default() -> Self {
        Self
    }
}

impl Tool for SubagentMessageTool {
    fn name(&self) -> &str {
        "subagent_message"
    }

    fn description(&self) -> &str {
        "Send one message from this Subagent to its dispatching parent or to a \
         sibling attempt. Parent direction supports `report` (informational, \
         keep working) and `escalate` (request clarification; you still keep \
         working — do not wait for the answer). Sibling direction delivers a \
         queue-only note into the sibling's active turn; use `subagent_list` \
         to discover sibling execution ids. Messages are claims, not verified \
         evidence."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "direction": {
                    "type": "string",
                    "enum": ["parent", "sibling"],
                    "description": "Message target: the dispatching parent, or a sibling attempt."
                },
                "text": {
                    "type": "string",
                    "description": "Message body (non-empty, bounded)."
                },
                "intent": {
                    "type": "string",
                    "enum": ["report", "escalate"],
                    "description": "Parent direction only: informational report (default) or blocking clarification request."
                },
                "execution_id": {
                    "type": "string",
                    "description": "Sibling direction only: target a LIVE attempt by its execution id (from subagent_list)."
                },
                "task_id": {
                    "type": "string",
                    "description": "Sibling direction only: target a task in the same plan that has no live attempt yet (exactly one of execution_id / task_id)."
                }
            },
            "required": ["direction", "text"],
            "additionalProperties": false
        })
    }

    fn execute_with_context<'a>(
        &'a self,
        parameters: ToolParameters,
        ctx: &'a ToolContext,
    ) -> BoxFuture<'a, crate::error::Result<ToolResult>> {
        Box::pin(async move {
            let Some(lineage) = ctx.subagent_lineage.clone() else {
                return Ok(ToolResult::error(
                    "subagent_message is only available inside a dispatched Subagent",
                ));
            };
            let Some(uplink) = ctx.uplink.clone() else {
                return Ok(ToolResult::error(
                    "no uplink channel is wired for this dispatch; messaging is unavailable",
                ));
            };

            let direction = parameters
                .get("direction")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let text = parameters
                .get("text")
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or_default()
                .to_string();
            if text.is_empty() {
                return Ok(ToolResult::error("text must be a non-empty string"));
            }
            if text.chars().count() > MAX_MESSAGE_CHARS {
                return Ok(ToolResult::error(format!(
                    "text exceeds {MAX_MESSAGE_CHARS} characters"
                )));
            }
            let text = bounded_text(&text);

            let target = match direction.as_str() {
                "parent" => {
                    let intent = match parameters.get("intent").and_then(Value::as_str) {
                        None | Some("report") => SubagentUplinkKind::Report,
                        Some("escalate") => SubagentUplinkKind::Escalate,
                        Some(other) => {
                            return Ok(ToolResult::error(format!(
                                "invalid intent '{other}' (expected report|escalate)"
                            )));
                        }
                    };
                    SubagentUplinkTarget::Parent { kind: intent, text }
                }
                "sibling" => {
                    let execution_id = parameters
                        .get("execution_id")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(str::to_string);
                    let task_id = parameters
                        .get("task_id")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(str::to_string);
                    let to = match (execution_id, task_id) {
                        (Some(execution_id), None) => {
                            echo_core::tools::SubagentPeerAddress::ByExecutionId(execution_id)
                        }
                        (None, Some(task_id)) => {
                            echo_core::tools::SubagentPeerAddress::ByTaskId(task_id)
                        }
                        _ => {
                            return Ok(ToolResult::error(
                                "sibling direction requires exactly one of execution_id / task_id",
                            ));
                        }
                    };
                    SubagentUplinkTarget::Sibling { to, text }
                }
                other => {
                    return Ok(ToolResult::error(format!(
                        "invalid direction '{other}' (expected parent|sibling)"
                    )));
                }
            };

            let receipt = uplink(SubagentUplinkMessage {
                from: lineage,
                target,
            })
            .await;

            tracing::debug!(
                tool = "subagent_message",
                status = %receipt.status,
                accepted = receipt.accepted,
                "subagent uplink settled"
            );

            Ok(ToolResult::success(
                json!({
                    "accepted": receipt.accepted,
                    "status": receipt.status,
                    "detail": receipt.detail,
                })
                .to_string(),
            ))
        })
    }
}

pub struct SubagentListTool {
    registry: std::sync::Arc<SubagentRegistry>,
}

impl SubagentListTool {
    pub fn new(registry: std::sync::Arc<SubagentRegistry>) -> Self {
        Self { registry }
    }
}

impl Tool for SubagentListTool {
    fn name(&self) -> &str {
        "subagent_list"
    }

    fn description(&self) -> &str {
        "List the currently active Subagent attempts visible in the shared \
         control plane (bounded). Use the execution ids as `subagent_message` \
         sibling targets."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "limit": {
                    "type": "integer",
                    "description": "Maximum entries to return (default 20, max 64)."
                }
            },
            "additionalProperties": false
        })
    }

    fn execute_with_context<'a>(
        &'a self,
        parameters: ToolParameters,
        _ctx: &'a ToolContext,
    ) -> BoxFuture<'a, crate::error::Result<ToolResult>> {
        let registry = self.registry.clone();
        Box::pin(async move {
            let limit = parameters
                .get("limit")
                .and_then(Value::as_u64)
                .map(|value| value as usize)
                .unwrap_or(DEFAULT_LIST_LIMIT);
            let limit = limit.clamp(1, 64);
            let attempts = registry.control_registry().active_snapshot(limit);
            Ok(ToolResult::success(
                json!({
                    "active": attempts
                        .iter()
                        .map(|summary| json!({
                            "execution_id": summary.execution_id,
                            "task_id": summary.task_id,
                            "attempt": summary.attempt,
                            "phase": format!("{:?}", summary.phase),
                        }))
                        .collect::<Vec<_>>(),
                    "count": attempts.len(),
                })
                .to_string(),
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_schema_is_complete() {
        let tool = SubagentMessageTool::new();
        let schema = tool.parameters();
        assert!(schema["properties"]["direction"].is_object());
        assert_eq!(schema["required"][0], "direction");
    }

    #[test]
    fn list_schema_has_no_required_fields() {
        let tool = SubagentListTool::new(std::sync::Arc::new(SubagentRegistry::new()));
        let schema = tool.parameters();
        assert!(schema.get("required").is_none());
    }
}
