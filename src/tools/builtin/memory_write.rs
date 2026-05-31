//! Memory Write tool — lets the agent persist information across sessions.
//!
//! The agent can write key-value memories scoped by lifetime (session/project/user).

use async_trait::async_trait;
use echo_core::error::Result;
use echo_core::tools::{Tool, ToolParameters, ToolResult};
use futures::future::BoxFuture;
use std::collections::HashMap;
use std::sync::Mutex;

static MEMORY: std::sync::LazyLock<Mutex<HashMap<String, String>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

const MAX_MEMORY_ENTRIES: usize = 500;

pub struct MemoryWriteTool;

#[async_trait]
impl Tool for MemoryWriteTool {
    fn name(&self) -> &str {
        "memory_write"
    }
    fn description(&self) -> &str {
        "Write a key-value memory that persists across the session. \
         Use this to remember important information the user shares, \
         project conventions, or decisions made. \
         Parameters: key (memory name), value (content), scope ('session'|'project'|'user')"
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "key": {"type": "string", "description": "Memory key/name"},
                "value": {"type": "string", "description": "Content to remember"},
                "scope": {"type": "string", "enum": ["session", "project", "user"], "default": "session"}
            },
            "required": ["key", "value"]
        })
    }
    fn execute<'a>(&'a self, params: ToolParameters) -> BoxFuture<'a, Result<ToolResult>> {
        Box::pin(async move {
            let key = params.get("key").and_then(|v| v.as_str()).unwrap_or("");
            let value = params.get("value").and_then(|v| v.as_str()).unwrap_or("");
            let scope = params
                .get("scope")
                .and_then(|v| v.as_str())
                .unwrap_or("session");
            if key.is_empty() {
                return Ok(ToolResult::error("key is required"));
            }
            let full_key = format!("{scope}:{key}");
            let mut mem = MEMORY.lock().unwrap_or_else(|e| e.into_inner());
            // Evict oldest entry if at capacity (FIFO via iter().next())
            if mem.len() >= MAX_MEMORY_ENTRIES && !mem.contains_key(&full_key) {
                if let Some(oldest_key) = mem.keys().next().cloned() {
                    mem.remove(&oldest_key);
                }
            }
            mem.insert(full_key.clone(), value.to_string());
            Ok(ToolResult::success(format!("Stored: {full_key}")))
        })
    }
    fn risk_level(&self) -> echo_core::tools::ToolRiskLevel {
        echo_core::tools::ToolRiskLevel::ReadOnly
    }
}
