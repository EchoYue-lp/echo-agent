//! DiffTool — compare two files or strings and show differences
//!
//! Uses the `similar` crate to produce unified diffs. Supports:
//! - Comparing two files (`path_a` + `path_b`)
//! - Comparing a file against a string (`path_a` + `content_b`)

use super::resolve_path;
use echo_core::error::{Result, ToolError};
use echo_core::tools::pagination::PageRequest;
use echo_core::tools::{Tool, ToolParameters, ToolResult};
use futures::future::BoxFuture;
use serde_json::{Value, json};
use similar::udiff;
use std::path::PathBuf;

// ── DiffTool ────────────────────────────────────────────────────────────────────

/// Compare two files or a file against a string, returning a unified diff.
#[derive(Clone)]
pub struct DiffTool {
    base_dir: Option<PathBuf>,
    file_system: std::sync::Arc<dyn echo_core::tools::filesystem::FileSystem>,
}

impl DiffTool {
    pub fn with_file_system(
        mut self,
        file_system: std::sync::Arc<dyn echo_core::tools::filesystem::FileSystem>,
    ) -> Self {
        self.file_system = file_system;
        self
    }

    pub fn new() -> Self {
        Self {
            base_dir: None,
            file_system: std::sync::Arc::new(super::LocalFileSystem),
        }
    }

    pub fn with_base_dir(base: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: Some(base.into()),
            file_system: std::sync::Arc::new(super::LocalFileSystem),
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
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 50,
                    "description": "Hunks per page (default 10)"
                },
                "cursor": {
                    "type": "string",
                    "description": "Cursor from the previous page"
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
        let this = self.clone();
        let ctx = ctx.clone();
        super::run_file_operation("diff", &ctx.clone(), move || {
            let file_system = this.file_system.as_ref();
            let path_a_str = parameters
                .get("path_a")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("path_a".to_string()))?;

            let path_b_str = parameters.get("path_b").and_then(|v| v.as_str());
            let content_b = parameters.get("content_b").and_then(|v| v.as_str());

            let context_lines = match parameters.get("context") {
                Some(value) => match value.as_u64().and_then(|value| usize::try_from(value).ok()) {
                    Some(value) => value,
                    None => {
                        return Ok(ToolResult::invalid_arguments(
                            "context must be a non-negative integer supported by this platform"
                                .to_string(),
                        ));
                    }
                },
                None => 3,
            };
            let page_request = match PageRequest::from_parameters(&parameters, 10, 50) {
                Ok(request) => request,
                Err(error) => return Ok(ToolResult::invalid_arguments(error.to_string())),
            };

            if path_b_str.is_none() && content_b.is_none() {
                return Ok(ToolResult::invalid_arguments(
                    "Either path_b or content_b must be provided.".to_string(),
                ));
            }

            let path_a = resolve_path(
                this.file_system.as_ref(),
                "diff",
                path_a_str,
                &this.base_dir,
                ctx.working_dir.as_deref(),
            )?;

            if !file_system.exists(&path_a)? {
                return Ok(ToolResult::invalid_arguments(format!(
                    "File does not exist: {}",
                    path_a.display()
                )));
            }

            let content_a = file_system.read_to_string(&path_a).map_err(|e| {
                super::file_error(e, |e| ToolError::ExecutionFailed {
                    tool: "diff".to_string(),
                    message: format!("Failed to read {}: {}", path_a.display(), e),
                })
            })?;

            let (content_b_val, label_b) = if let Some(content) = content_b {
                (content.to_string(), "<provided content>".to_string())
            } else if let Some(path_b_str) = path_b_str {
                let path_b = resolve_path(
                    this.file_system.as_ref(),
                    "diff",
                    path_b_str,
                    &this.base_dir,
                    ctx.working_dir.as_deref(),
                )?;
                if !file_system.exists(&path_b)? {
                    return Ok(ToolResult::invalid_arguments(format!(
                        "File does not exist: {}",
                        path_b.display()
                    )));
                }
                let content = file_system.read_to_string(&path_b).map_err(|e| {
                    super::file_error(e, |e| ToolError::ExecutionFailed {
                        tool: "diff".to_string(),
                        message: format!("Failed to read {}: {}", path_b.display(), e),
                    })
                })?;
                (content, path_b.display().to_string())
            } else {
                return Ok(ToolResult::invalid_arguments(
                    "Either path_b or content_b must be provided.".to_string(),
                ));
            };

            let label_a = path_a.display().to_string();
            let diff = udiff::unified_diff(
                similar::Algorithm::default(),
                &content_a,
                &content_b_val,
                context_lines,
                Some((&label_a, &label_b)),
            );

            let chunks = crate::diff_pagination::split_unified_diff(&diff);
            let query = serde_json::json!({
                "path_a": path_a,
                "path_b": path_b_str,
                "content_b_sha256": content_b.map(|content| {
                    use sha2::{Digest, Sha256};
                    format!("{:x}", Sha256::digest(content.as_bytes()))
                }),
                "context": context_lines,
            });
            let (page, page_info) = match page_request.paginate(chunks, &query) {
                Ok(page) => page,
                Err(error) => return Ok(ToolResult::invalid_arguments(error.to_string())),
            };
            let output = if page.is_empty() {
                "Files are identical - no differences found.".to_string()
            } else {
                page.join("\n")
            };
            let mut result = ToolResult::success(output);
            page_info.apply_to(&mut result);
            Ok(result)
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
