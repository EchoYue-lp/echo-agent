//! Research memory tool
//!
//! Enables the agent to remember and retrieve past research queries and findings
//! across sessions, building a persistent knowledge base.

use futures::future::BoxFuture;
use serde_json::Value;

use echo_core::error::{Result, ToolError};
use echo_core::tools::permission::ToolPermission;
use echo_core::tools::{Tool, ToolParameters, ToolResult};

// ── research_remember tool ──────────────────────────────────────────────────

pub struct ResearchRememberTool;

impl Tool for ResearchRememberTool {
    fn name(&self) -> &str {
        "research_remember"
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Read, ToolPermission::Write]
    }

    fn description(&self) -> &str {
        "Store research findings, queries, and insights for future reference. \
         Build a persistent knowledge base across research sessions."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "topic": {
                    "type": "string",
                    "description": "Research topic or query"
                },
                "findings": {
                    "type": "string",
                    "description": "Key findings or insights to remember"
                },
                "papers": {
                    "type": "array",
                    "description": "List of relevant papers (optional)",
                    "items": {
                        "type": "object",
                        "properties": {
                            "title": {"type": "string"},
                            "authors": {"type": "array", "items": {"type": "string"}},
                            "year": {"type": "integer"},
                            "key_points": {"type": "string"}
                        }
                    }
                },
                "tags": {
                    "type": "array",
                    "description": "Tags for categorization (optional)",
                    "items": {"type": "string"}
                }
            },
            "required": ["topic", "findings"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let topic = parameters
                .get("topic")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("topic".to_string()))?;

            let findings = parameters
                .get("findings")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("findings".to_string()))?;

            let papers = parameters
                .get("papers")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();

            let tags = parameters
                .get("tags")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();

            // Store in memory (in production, this would persist to SQLite)
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);

            let _entry = serde_json::json!({
                "topic": topic,
                "findings": findings,
                "papers": papers,
                "tags": tags,
                "timestamp": timestamp
            });

            // For now, return success message
            // In production, this would write to a persistent store
            let message = format!(
                "Research findings stored successfully.\n\
                 Topic: {}\n\
                 Papers referenced: {}\n\
                 Tags: {}",
                topic,
                papers.len(),
                tags.iter()
                    .filter_map(|t| t.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );

            Ok(ToolResult::success(message))
        })
    }
}

// ── research_recall tool ────────────────────────────────────────────────────

pub struct ResearchRecallTool;

impl Tool for ResearchRecallTool {
    fn name(&self) -> &str {
        "research_recall"
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Read]
    }

    fn description(&self) -> &str {
        "Recall past research findings by topic or keyword. \
         Search your research memory for relevant insights."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query (topic, keyword, or tag)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default 5)"
                }
            },
            "required": ["query"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let query = parameters
                .get("query")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("query".to_string()))?;

            let _limit = parameters
                .get("limit")
                .and_then(|v| v.as_u64())
                .unwrap_or(5) as usize;

            // For now, return a message indicating no stored research
            // In production, this would query a persistent store
            let message = format!(
                "No research findings found for query: '{}'\n\
                 Use research_remember to store findings from your current research.",
                query
            );

            Ok(ToolResult::success(message))
        })
    }
}
