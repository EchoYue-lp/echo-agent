//! TodoWrite tool — exposes task tracking to the LLM.
//!
//! Allows the agent to create, update, and list tasks during execution.
//! This gives the agent a "scratchpad" for planning multi-step work.

use async_trait::async_trait;
use echo_core::error::Result;
use echo_core::tools::{Tool, ToolParameters, ToolResult};
use futures::future::BoxFuture;
use std::sync::Mutex;

/// In-memory task state shared across tool calls.
/// Capped at MAX_TASKS to prevent unbounded memory growth.
static TASKS: std::sync::LazyLock<Mutex<Vec<TodoEntry>>> =
    std::sync::LazyLock::new(|| Mutex::new(Vec::new()));

const MAX_TASKS: usize = 100;

#[derive(Debug, Clone, serde::Serialize)]
struct TodoEntry {
    id: String,
    content: String,
    status: String, // pending, in_progress, completed, cancelled
}

pub struct TodoWriteTool;

#[async_trait]
impl Tool for TodoWriteTool {
    fn name(&self) -> &str {
        "todo_write"
    }
    fn description(&self) -> &str {
        "Create and manage a task list for your current coding session. \
         Use this to plan and track multi-step work. \
         Parameters: action ('create'|'update'|'list'), \
         id (for update), content (task description), \
         status (for update: 'pending'|'in_progress'|'completed'|'cancelled')"
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": ["create", "update", "list"]},
                "id": {"type": "string", "description": "Task ID (for update)"},
                "content": {"type": "string", "description": "Task description"},
                "status": {"type": "string", "enum": ["pending", "in_progress", "completed", "cancelled"]}
            },
            "required": ["action"]
        })
    }
    fn execute<'a>(&'a self, params: ToolParameters) -> BoxFuture<'a, Result<ToolResult>> {
        Box::pin(async move {
            let action = params
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("list");
            let mut tasks = TASKS.lock().unwrap_or_else(|e| e.into_inner());
            match action {
                "create" => {
                    let content = params.get("content").and_then(|v| v.as_str()).unwrap_or("");
                    // Evict oldest completed/cancelled tasks if at capacity
                    if tasks.len() >= MAX_TASKS {
                        if let Some(pos) = tasks.iter().position(|t| t.status == "completed" || t.status == "cancelled") {
                            tasks.remove(pos);
                        } else {
                            tasks.remove(0); // fallback: evict oldest
                        }
                    }
                    let id = format!("task_{}", tasks.len() + 1);
                    tasks.push(TodoEntry {
                        id: id.clone(),
                        content: content.to_string(),
                        status: "pending".into(),
                    });
                    Ok(ToolResult::success(format!(
                        "Created task {}: {}",
                        id, content
                    )))
                }
                "update" => {
                    let id = params.get("id").and_then(|v| v.as_str()).unwrap_or("");
                    let status = params
                        .get("status")
                        .and_then(|v| v.as_str())
                        .unwrap_or("pending");
                    if let Some(t) = tasks.iter_mut().find(|t| t.id == id) {
                        t.status = status.to_string();
                        if let Some(content) = params.get("content").and_then(|v| v.as_str()) {
                            t.content = content.to_string();
                        }
                        Ok(ToolResult::success(format!(
                            "Updated task {}: status={}",
                            id, status
                        )))
                    } else {
                        Ok(ToolResult::error(format!("Task {} not found", id)))
                    }
                }
                _ => {
                    if tasks.is_empty() {
                        Ok(ToolResult::success(
                            "No tasks. Use todo_write with action='create' to add tasks.",
                        ))
                    } else {
                        let list: Vec<String> = tasks
                            .iter()
                            .map(|t| format!("[{}] {} — {}", t.status, t.id, t.content))
                            .collect();
                        Ok(ToolResult::success(list.join("\n")))
                    }
                }
            }
        })
    }
    fn risk_level(&self) -> echo_core::tools::ToolRiskLevel {
        echo_core::tools::ToolRiskLevel::ReadOnly
    }
}
