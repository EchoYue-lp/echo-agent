//! DiffTool — compare two files or strings and show differences
//!
//! Uses the `similar` crate to produce unified diffs. Supports:
//! - Comparing two files (`path_a` + `path_b`)
//! - Comparing a file against a string (`path_a` + `content_b`)

use super::resolve_path;
use echo_core::error::{Result, ToolError};
use echo_core::tools::{Tool, ToolParameters, ToolResult};
use futures::future::BoxFuture;
use serde_json::{Value, json};
use similar::udiff;
use std::path::PathBuf;
use tokio::fs;

// ── DiffTool ────────────────────────────────────────────────────────────────────

/// Compare two files or a file against a string, returning a unified diff.
pub struct DiffTool {
    base_dir: Option<PathBuf>,
}

impl DiffTool {
    pub fn new() -> Self {
        Self { base_dir: None }
    }

    pub fn with_base_dir(base: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: Some(base.into()),
        }
    }
}

impl Default for DiffTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for DiffTool {
    fn name(&self) -> &str {
        "diff"
    }

    fn description(&self) -> &str {
        "Compare two files or a file against a string, returning a unified diff. Use path_a + path_b to compare two files, or path_a + content_b to compare a file against given content."
    }

    fn permissions(&self) -> Vec<echo_core::tools::permission::ToolPermission> {
        vec![echo_core::tools::permission::ToolPermission::Read]
    }
    fn risk_level(&self) -> echo_core::tools::ToolRiskLevel {
        echo_core::tools::ToolRiskLevel::ReadOnly
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path_a": {
                    "type": "string",
                    "description": "First file path (the 'before' side)"
                },
                "path_b": {
                    "type": "string",
                    "description": "Second file path (the 'after' side). Either path_b or content_b is required."
                },
                "content_b": {
                    "type": "string",
                    "description": "Alternative to path_b: compare path_a against this string content."
                },
                "context": {
                    "type": "integer",
                    "description": "Number of context lines around changes (default: 3)"
                }
            },
            "required": ["path_a"]
        })
    }

    fn execute_with_context<'a>(
        &'a self,
        parameters: ToolParameters,
        ctx: &'a echo_core::tools::ToolContext,
    ) -> BoxFuture<'a, Result<ToolResult>> {
        Box::pin(async move {
            let path_a_str = parameters
                .get("path_a")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("path_a".to_string()))?;

            let path_b_str = parameters.get("path_b").and_then(|v| v.as_str());
            let content_b = parameters.get("content_b").and_then(|v| v.as_str());

            let context_lines = parameters
                .get("context")
                .and_then(|v| v.as_u64())
                .unwrap_or(3) as usize;

            if path_b_str.is_none() && content_b.is_none() {
                return Ok(ToolResult::invalid_arguments(
                    "Either path_b or content_b must be provided.".to_string(),
                ));
            }

            let path_a = resolve_path(
                "diff",
                path_a_str,
                &self.base_dir,
                ctx.working_dir.as_deref(),
            )?;

            if !path_a.exists() {
                return Ok(ToolResult::invalid_arguments(format!(
                    "File does not exist: {}",
                    path_a.display()
                )));
            }

            let content_a =
                fs::read_to_string(&path_a)
                    .await
                    .map_err(|e| ToolError::ExecutionFailed {
                        tool: "diff".to_string(),
                        message: format!("Failed to read {}: {}", path_a.display(), e),
                    })?;

            let (content_b_val, label_b) = if let Some(content) = content_b {
                (content.to_string(), "<provided content>".to_string())
            } else {
                let path_b = resolve_path(
                    "diff",
                    path_b_str.unwrap(),
                    &self.base_dir,
                    ctx.working_dir.as_deref(),
                )?;
                if !path_b.exists() {
                    return Ok(ToolResult::invalid_arguments(format!(
                        "File does not exist: {}",
                        path_b.display()
                    )));
                }
                let content =
                    fs::read_to_string(&path_b)
                        .await
                        .map_err(|e| ToolError::ExecutionFailed {
                            tool: "diff".to_string(),
                            message: format!("Failed to read {}: {}", path_b.display(), e),
                        })?;
                (content, path_b.display().to_string())
            };

            let label_a = path_a.display().to_string();
            let diff = udiff::unified_diff(
                similar::Algorithm::default(),
                &content_a,
                &content_b_val,
                context_lines,
                Some((&label_a, &label_b)),
            );

            if diff.is_empty() {
                return Ok(ToolResult::success(
                    "Files are identical — no differences found.".to_string(),
                ));
            }

            Ok(ToolResult::success(diff))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_diff_identical() {
        let content = "hello\nworld\n";
        let diff = udiff::unified_diff(
            similar::Algorithm::default(),
            content,
            content,
            3,
            Some(("a.txt", "b.txt")),
        );
        assert!(diff.is_empty());
    }

    #[test]
    fn test_diff_changed() {
        let a = "hello\nworld\nfoo\n";
        let b = "hello\nrust\nfoo\n";
        let diff = udiff::unified_diff(
            similar::Algorithm::default(),
            a,
            b,
            3,
            Some(("a.txt", "b.txt")),
        );
        assert!(diff.contains("-world"));
        assert!(diff.contains("+rust"));
    }
}
