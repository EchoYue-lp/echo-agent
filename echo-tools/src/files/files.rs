use super::resolve_path;
use echo_core::error::ToolError;
use echo_core::tokenizer::{HeuristicTokenizer, Tokenizer};
use echo_core::tools::pagination::PageRequest;
use echo_core::tools::permission::ToolPermission;
use echo_core::tools::{
    Tool, ToolFailure, ToolFailureCategory, ToolParameters, ToolResult, ToolRiskLevel,
    ToolSideEffect,
};
use futures::future::BoxFuture;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::borrow::Cow;
use std::path::{Path, PathBuf};
use tokio::fs;

// Used for the existing UTF-8-first, GBK-fallback decoding behavior.
use encoding_rs;

const DEFAULT_READ_MAX_LINES: usize = 500;
const MAX_READ_OUTPUT_TOKENS: usize = 4_000;
const READ_NOTICE_TOKEN_RESERVE: usize = 128;
const MAX_READ_CONTENT_TOKENS: usize = MAX_READ_OUTPUT_TOKENS - READ_NOTICE_TOKEN_RESERVE;

fn content_hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn positive_usize_parameter(
    parameters: &ToolParameters,
    name: &str,
    default: usize,
) -> Result<usize, String> {
    let Some(value) = parameters.get(name) else {
        return Ok(default);
    };
    let raw = value
        .as_u64()
        .filter(|value| *value > 0)
        .ok_or_else(|| format!("'{name}' must be a positive integer"))?;
    usize::try_from(raw).map_err(|_| format!("'{name}' is too large"))
}

fn with_read_file_metadata(
    mut result: ToolResult,
    path: &Path,
    hash: &str,
    encoding: &str,
    total_lines: usize,
    start_line: usize,
    end_line: usize,
) -> ToolResult {
    result
        .metadata
        .insert("path".to_string(), path.display().to_string());
    result
        .metadata
        .insert("content_hash".to_string(), hash.to_string());
    result
        .metadata
        .insert("encoding".to_string(), encoding.to_string());
    result
        .metadata
        .insert("total_lines".to_string(), total_lines.to_string());
    result
        .metadata
        .insert("start_line".to_string(), start_line.to_string());
    result
        .metadata
        .insert("end_line".to_string(), end_line.to_string());
    result.metadata.insert(
        "remaining_lines".to_string(),
        total_lines.saturating_sub(end_line).to_string(),
    );
    result
}

fn file_idempotency_key(
    ctx: &echo_core::tools::ToolContext,
    tool_name: &str,
    path: &std::path::Path,
    hash: &str,
) -> String {
    ctx.call_id
        .clone()
        .unwrap_or_else(|| format!("{tool_name}:{}:{hash}", path.display()))
}

fn partial_file_failure(
    ctx: &echo_core::tools::ToolContext,
    tool_name: &str,
    path: &std::path::Path,
    hash: &str,
    message: impl Into<String>,
) -> ToolResult {
    ToolResult::error(message).with_failure(
        ToolFailure::new(ToolFailureCategory::PartialSideEffect)
            .with_side_effect(ToolSideEffect::Possible)
            .with_idempotency_key(file_idempotency_key(ctx, tool_name, path, hash))
            .with_postcondition(format!(
                "read '{}' and compare content_hash with {hash} before retrying",
                path.display()
            )),
    )
}
// ── CreateFileTool ────────────────────────────────────────────────────────────
pub struct CreateFileTool {
    base_dir: Option<PathBuf>,
}

impl CreateFileTool {
    pub fn new() -> Self {
        Self { base_dir: None }
    }

    pub fn with_base_dir(base: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: Some(base.into()),
        }
    }
}

impl Default for CreateFileTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for CreateFileTool {
    fn name(&self) -> &str {
        "create_file"
    }

    fn description(&self) -> &str {
        "Create a specified file."
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Write]
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File path to create (relative or absolute path)"
                }
            },
            "required": ["path"]
        })
    }

    fn execute_with_context<'a>(
        &'a self,
        parameters: ToolParameters,
        ctx: &'a echo_core::tools::ToolContext,
    ) -> BoxFuture<'a, echo_core::error::Result<ToolResult>> {
        Box::pin(async move {
            let path_str = parameters
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("path".to_string()))?;

            let path = resolve_path(
                "create_file",
                path_str,
                &self.base_dir,
                ctx.working_dir.as_deref(),
            )?;

            if path.exists() {
                return Ok(ToolResult::failure(
                    ToolFailureCategory::InvalidArguments,
                    format!("File already exists: {}", path.display()),
                ));
            }

            // Auto-create parent directory
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent).await.map_err(|e| {
                    ToolError::ExecutionFailed {
                        tool: "create_file".to_string(),
                        message: format!("Failed to create directory: {}", e),
                    }
                })?;
            }

            tokio::fs::write(&path, "")
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: "create_file".to_string(),
                    message: format!("Failed to create file: {}", e),
                })?;

            Ok(ToolResult::success(format!(
                "File created successfully: {}",
                path.display()
            )))
        })
    }
}

// ── DeleteFileTool ────────────────────────────────────────────────────────────
pub struct DeleteFileTool {
    base_dir: Option<PathBuf>,
}

impl DeleteFileTool {
    pub fn new() -> Self {
        Self { base_dir: None }
    }

    pub fn with_base_dir(base: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: Some(base.into()),
        }
    }
}

impl Default for DeleteFileTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for DeleteFileTool {
    fn name(&self) -> &str {
        "delete_file"
    }

    fn description(&self) -> &str {
        "Delete a specified file."
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Write]
    }
    fn risk_level(&self) -> ToolRiskLevel {
        ToolRiskLevel::Dangerous
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File path to delete (relative or absolute path)"
                }
            },
            "required": ["path"]
        })
    }

    fn execute_with_context<'a>(
        &'a self,
        parameters: ToolParameters,
        ctx: &'a echo_core::tools::ToolContext,
    ) -> BoxFuture<'a, echo_core::error::Result<ToolResult>> {
        Box::pin(async move {
            let path_str = parameters
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("path".to_string()))?;

            let path = resolve_path(
                "delete_file",
                path_str,
                &self.base_dir,
                ctx.working_dir.as_deref(),
            )?;

            if !path.exists() {
                return Ok(ToolResult::failure(
                    ToolFailureCategory::InvalidArguments,
                    format!("File does not exist: {}", path.display()),
                ));
            }
            if !path.is_file() {
                return Ok(ToolResult::failure(
                    ToolFailureCategory::InvalidArguments,
                    format!("'{}' is not a file", path.display()),
                ));
            }

            // Create git checkpoint before deletion
            let checkpoint_tag =
                crate::git_checkpoint::create_checkpoint(&path).map_err(|error| {
                    ToolError::ExecutionFailed {
                        tool: "delete_file".to_string(),
                        message: format!("Failed to create recovery checkpoint: {error}"),
                    }
                })?;
            if checkpoint_tag.is_some() {
                crate::git_checkpoint::cleanup_old_checkpoints(&path, 10);
            }

            tokio::fs::remove_file(&path)
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: "delete_file".to_string(),
                    message: format!("Failed to delete: {}", e),
                })?;

            let mut result =
                ToolResult::success(format!("File deleted successfully: {}", path.display()));

            if let Some(tag) = checkpoint_tag {
                result = result.with_meta("git_checkpoint", tag);
            }

            Ok(result)
        })
    }
}

// ── ReadFileTool ──────────────────────────────────────────────────────────────
/// Read text with compact line numbers, bounded pagination, and encoding detection.
pub struct ReadFileTool {
    base_dir: Option<PathBuf>,
}

impl ReadFileTool {
    pub fn new() -> Self {
        Self { base_dir: None }
    }

    pub fn with_base_dir(base: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: Some(base.into()),
        }
    }
}

impl Default for ReadFileTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Read compact line-numbered text with bounded output. Use offset/limit to paginate; prefer grep for known text."
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Read]
    }
    fn risk_level(&self) -> ToolRiskLevel {
        ToolRiskLevel::ReadOnly
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative or absolute file path"
                },
                "offset": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "First line to read, 1-based (default: 1)"
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Maximum lines to read (default: 500)"
                }
            },
            "required": ["path"],
            "additionalProperties": false
        })
    }

    fn execute_with_context<'a>(
        &'a self,
        parameters: ToolParameters,
        ctx: &'a echo_core::tools::ToolContext,
    ) -> BoxFuture<'a, echo_core::error::Result<ToolResult>> {
        Box::pin(async move {
            let allowed_parameters = ["path", "offset", "limit"];
            let mut unknown_parameters = parameters
                .keys()
                .filter(|key| !allowed_parameters.contains(&key.as_str()))
                .cloned()
                .collect::<Vec<_>>();
            unknown_parameters.sort();
            if !unknown_parameters.is_empty() {
                return Ok(ToolResult::failure(
                    ToolFailureCategory::InvalidArguments,
                    format!(
                        "Unknown read_file parameter(s): {}",
                        unknown_parameters.join(", ")
                    ),
                ));
            }

            let path_str = parameters
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("path".to_string()))?;
            let offset = match positive_usize_parameter(&parameters, "offset", 1) {
                Ok(offset) => offset,
                Err(message) => {
                    return Ok(ToolResult::failure(
                        ToolFailureCategory::InvalidArguments,
                        message,
                    ));
                }
            };
            let explicit_limit = parameters.contains_key("limit");
            let limit = match positive_usize_parameter(&parameters, "limit", DEFAULT_READ_MAX_LINES)
            {
                Ok(limit) => limit,
                Err(message) => {
                    return Ok(ToolResult::failure(
                        ToolFailureCategory::InvalidArguments,
                        message,
                    ));
                }
            };

            let path = resolve_path(
                "read_file",
                path_str,
                &self.base_dir,
                ctx.working_dir.as_deref(),
            )?;

            if !path.exists() {
                return Ok(ToolResult::failure(
                    ToolFailureCategory::InvalidArguments,
                    format!("File does not exist: {}", path.display()),
                ));
            }
            if !path.is_file() {
                return Ok(ToolResult::failure(
                    ToolFailureCategory::InvalidArguments,
                    format!("'{}' is not a file", path.display()),
                ));
            }

            let bytes = tokio::fs::read(&path)
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: "read_file".to_string(),
                    message: format!("Failed to read: {}", e),
                })?;

            let hash = content_hash(&bytes);
            let (content, detected_encoding): (Cow<'_, str>, &str) =
                match std::str::from_utf8(&bytes) {
                    Ok(content) => (Cow::Borrowed(content), "utf-8"),
                    Err(_) => (encoding_rs::GBK.decode(&bytes).0, "gbk"),
                };
            let total_lines = content.lines().count();

            if total_lines == 0 {
                return Ok(with_read_file_metadata(
                    ToolResult::success("File is empty."),
                    &path,
                    &hash,
                    detected_encoding,
                    0,
                    0,
                    0,
                ));
            }

            let start = offset.saturating_sub(1);

            if start >= total_lines {
                return Ok(ToolResult::failure(
                    ToolFailureCategory::InvalidArguments,
                    format!("Offset {offset} exceeds total lines ({total_lines})"),
                ));
            }

            let tokenizer = HeuristicTokenizer;
            let mut output = String::new();
            let mut output_tokens = 0_usize;
            let mut returned_lines = 0_usize;
            let mut token_limited = false;

            for (index, line) in content.lines().skip(start).take(limit).enumerate() {
                let line_number = offset.saturating_add(index);
                let fragment = format!("{line_number}|{line}\n");
                let fragment_tokens = tokenizer.count_tokens(&fragment);

                if fragment_tokens > MAX_READ_CONTENT_TOKENS {
                    return Ok(ToolResult::failure(
                        ToolFailureCategory::InvalidArguments,
                        format!(
                            "Line {line_number} exceeds the read_file token budget by itself. Use grep to locate specific content instead."
                        ),
                    ));
                }

                if output_tokens.saturating_add(fragment_tokens) > MAX_READ_CONTENT_TOKENS {
                    if explicit_limit {
                        let suggested_limit = returned_lines.max(1);
                        return Ok(ToolResult::failure(
                            ToolFailureCategory::InvalidArguments,
                            format!(
                                "Requested range exceeds the {MAX_READ_OUTPUT_TOKENS}-token read_file budget. Retry with limit={suggested_limit} or use grep for specific content."
                            ),
                        ));
                    }
                    token_limited = true;
                    break;
                }

                output.push_str(&fragment);
                output_tokens = output_tokens.saturating_add(fragment_tokens);
                returned_lines = returned_lines.saturating_add(1);
            }

            let end_line = offset.saturating_add(returned_lines.saturating_sub(1));
            let has_more = end_line < total_lines;
            let truncation_reason = if has_more {
                Some(if token_limited {
                    "token_budget"
                } else {
                    "line_limit"
                })
            } else {
                None
            };

            if has_more {
                let next_offset = end_line.saturating_add(1);
                output.push_str(&format!(
                    "[Partial: lines {offset}-{end_line} of {total_lines}; continue with offset={next_offset}.]"
                ));
            }

            let mut result = with_read_file_metadata(
                ToolResult::success(output).with_truncated(has_more),
                &path,
                &hash,
                detected_encoding,
                total_lines,
                offset,
                end_line,
            );
            if let Some(reason) = truncation_reason {
                result
                    .metadata
                    .insert("truncation_reason".to_string(), reason.to_string());
            }
            Ok(result)
        })
    }
}

// ── WriteFileTool ─────────────────────────────────────────────────────────────

/// Write (overwrite) file content, auto-create parent directories if they don't exist
pub struct WriteFileTool {
    base_dir: Option<PathBuf>,
}

impl WriteFileTool {
    pub fn new() -> Self {
        Self { base_dir: None }
    }

    pub fn with_base_dir(base: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: Some(base.into()),
        }
    }
}

impl Default for WriteFileTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        "Write content to a file at the specified path (overwrite), auto-creating parent directories"
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Write]
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File path to write to"
                },
                "content": {
                    "type": "string",
                    "description": "Text content to write"
                },
                "expected_hash": {
                    "type": "string",
                    "description": "Optional SHA-256 content_hash returned by read_file; the write is rejected if the file changed"
                }
            },
            "required": ["path", "content"]
        })
    }

    fn execute_with_context<'a>(
        &'a self,
        parameters: ToolParameters,
        ctx: &'a echo_core::tools::ToolContext,
    ) -> BoxFuture<'a, echo_core::error::Result<ToolResult>> {
        Box::pin(async move {
            let path_str = parameters
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("path".to_string()))?;

            let content = parameters
                .get("content")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("content".to_string()))?;
            let expected_hash = parameters.get("expected_hash").and_then(Value::as_str);

            let path = resolve_path(
                "write_file",
                path_str,
                &self.base_dir,
                ctx.working_dir.as_deref(),
            )?;

            if let Some(expected_hash) = expected_hash {
                let current = match tokio::fs::read(&path).await {
                    Ok(current) => current,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        return Ok(ToolResult::failure(
                            ToolFailureCategory::InvalidArguments,
                            format!(
                                "File '{}' no longer exists; expected content_hash {expected_hash}",
                                path.display()
                            ),
                        ));
                    }
                    Err(error) => {
                        return Ok(ToolResult::failure(
                            ToolFailureCategory::Permanent,
                            format!("Failed to verify '{}': {error}", path.display()),
                        ));
                    }
                };
                let actual_hash = content_hash(&current);
                if actual_hash != expected_hash {
                    return Ok(ToolResult::failure(
                        ToolFailureCategory::InvalidArguments,
                        format!(
                            "File '{}' changed since it was read (expected {expected_hash}, actual {actual_hash})",
                            path.display()
                        ),
                    ));
                }
            }

            let target_hash = content_hash(content.as_bytes());

            // Auto-create parent directory
            if let Some(parent) = path.parent()
                && let Err(error) = tokio::fs::create_dir_all(parent).await
            {
                return Ok(partial_file_failure(
                    ctx,
                    "write_file",
                    &path,
                    &target_hash,
                    format!("Failed to create directory: {error}"),
                ));
            }

            // Create git checkpoint before mutation (only if file already exists)
            let checkpoint_tag = if path.exists() {
                let tag = crate::git_checkpoint::create_checkpoint(&path).map_err(|error| {
                    ToolError::ExecutionFailed {
                        tool: "write_file".to_string(),
                        message: format!("Failed to create recovery checkpoint: {error}"),
                    }
                })?;
                if tag.is_some() {
                    crate::git_checkpoint::cleanup_old_checkpoints(&path, 10);
                }
                tag
            } else {
                None
            };

            let bytes = content.len();
            if let Err(error) = tokio::fs::write(&path, content).await {
                return Ok(partial_file_failure(
                    ctx,
                    "write_file",
                    &path,
                    &target_hash,
                    format!("Failed to write: {error}"),
                ));
            }

            let mut result = ToolResult::success(format!(
                "Successfully wrote {} bytes to '{}'",
                bytes,
                path.display()
            ));

            if let Some(tag) = checkpoint_tag {
                result = result.with_meta("git_checkpoint", tag);
            }

            result = result
                .with_meta("content_hash", target_hash.clone())
                .with_meta(
                    "idempotency_key",
                    file_idempotency_key(ctx, "write_file", &path, &target_hash),
                )
                .with_meta(
                    "postcondition",
                    format!("content_hash('{}') == {target_hash}", path.display()),
                );

            Ok(result)
        })
    }
}

// ── AppendFileTool ────────────────────────────────────────────────────────────

/// Append content to end of file
pub struct AppendFileTool {
    base_dir: Option<PathBuf>,
}

impl AppendFileTool {
    pub fn new() -> Self {
        Self { base_dir: None }
    }

    pub fn with_base_dir(base: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: Some(base.into()),
        }
    }
}

impl Default for AppendFileTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for AppendFileTool {
    fn name(&self) -> &str {
        "append_file"
    }

    fn description(&self) -> &str {
        "Append content to end of file (auto-create if file does not exist)"
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Write]
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Target file path"
                },
                "content": {
                    "type": "string",
                    "description": "Text content to append"
                }
            },
            "required": ["path", "content"]
        })
    }

    fn execute_with_context<'a>(
        &'a self,
        parameters: ToolParameters,
        ctx: &'a echo_core::tools::ToolContext,
    ) -> BoxFuture<'a, echo_core::error::Result<ToolResult>> {
        Box::pin(async move {
            use tokio::io::AsyncWriteExt;

            let path_str = parameters
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("path".to_string()))?;

            let content = parameters
                .get("content")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("content".to_string()))?;

            let path = resolve_path(
                "append_file",
                path_str,
                &self.base_dir,
                ctx.working_dir.as_deref(),
            )?;

            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent).await.map_err(|e| {
                    ToolError::ExecutionFailed {
                        tool: "append_file".to_string(),
                        message: format!("Failed to create directory: {}", e),
                    }
                })?;
            }

            let mut file = tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: "append_file".to_string(),
                    message: format!("Failed to open file: {}", e),
                })?;

            file.write_all(content.as_bytes())
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: "append_file".to_string(),
                    message: format!("Failed to append write: {}", e),
                })?;

            Ok(ToolResult::success(format!(
                "Appended {} bytes to '{}'",
                content.len(),
                path.display()
            )))
        })
    }
}

// ── UpdateFileTool ────────────────────────────────────────────────────────────────

/// Update file content
pub struct UpdateFileTool {
    base_dir: Option<PathBuf>,
}

impl UpdateFileTool {
    pub fn new() -> Self {
        Self { base_dir: None }
    }

    pub fn with_base_dir(base: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: Some(base.into()),
        }
    }
}

impl Default for UpdateFileTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for UpdateFileTool {
    fn name(&self) -> &str {
        "update_file"
    }

    fn description(&self) -> &str {
        "Update file content by replacing old content with new content."
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Write]
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Target file path"
                },
                "old_content": {
                    "type": "string",
                    "description": "Old content to be replaced."
                },
                "new_content": {
                    "type": "string",
                    "description": "New content to replace with."
                }
            },
            "required": ["path", "old_content", "new_content"]
        })
    }

    fn execute_with_context<'a>(
        &'a self,
        parameters: ToolParameters,
        ctx: &'a echo_core::tools::ToolContext,
    ) -> BoxFuture<'a, echo_core::error::Result<ToolResult>> {
        Box::pin(async move {
            let path_str = parameters
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("path".to_string()))?;

            let old_content = parameters
                .get("old_content")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("old_content".to_string()))?;
            let new_content = parameters
                .get("new_content")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("new_content".to_string()))?;

            let path = resolve_path(
                "update_file",
                path_str,
                &self.base_dir,
                ctx.working_dir.as_deref(),
            )?;

            if !path.exists() {
                return Ok(ToolResult::failure(
                    ToolFailureCategory::InvalidArguments,
                    format!("File does not exist: {}", path.display()),
                ));
            }

            let content =
                tokio::fs::read_to_string(&path)
                    .await
                    .map_err(|e| ToolError::ExecutionFailed {
                        tool: "update_file".to_string(),
                        message: format!("Failed to read file: {}", e),
                    })?;

            if !content.contains(old_content) {
                return Ok(ToolResult::failure(
                    ToolFailureCategory::InvalidArguments,
                    format!(
                        "Specified content not found in file, replacement failed: {}",
                        path.display()
                    ),
                ));
            }

            let updated = content.replacen(old_content, new_content, 1);

            tokio::fs::write(&path, &updated)
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: "update_file".to_string(),
                    message: format!("Failed to write update: {}", e),
                })?;

            Ok(ToolResult::success(format!(
                "File updated: {}",
                path.display()
            )))
        })
    }
}
// ── MoveFileTool ──────────────────────────────────────────────────────────────────
/// Move file to a new path
pub struct MoveFileTool {
    base_dir: Option<PathBuf>,
}

impl MoveFileTool {
    pub fn new() -> Self {
        Self { base_dir: None }
    }

    pub fn with_base_dir(base: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: Some(base.into()),
        }
    }
}

impl Default for MoveFileTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for MoveFileTool {
    fn name(&self) -> &str {
        "move_file"
    }

    fn description(&self) -> &str {
        "Move file to a new path"
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Write]
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "old_path": {
                    "type": "string",
                    "description": "Old file path"
                },"new_path": {
                    "type": "string",
                    "description": "New file path"
                }
            },
            "required": ["old_path","new_path"]
        })
    }

    fn execute_with_context<'a>(
        &'a self,
        parameters: ToolParameters,
        ctx: &'a echo_core::tools::ToolContext,
    ) -> BoxFuture<'a, echo_core::error::Result<ToolResult>> {
        Box::pin(async move {
            let old_path_str = parameters
                .get("old_path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("old_path".to_string()))?;

            let new_path_str = parameters
                .get("new_path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("new_path".to_string()))?;

            let old_path = resolve_path(
                "move_file",
                old_path_str,
                &self.base_dir,
                ctx.working_dir.as_deref(),
            )?;
            let new_path = resolve_path(
                "move_file",
                new_path_str,
                &self.base_dir,
                ctx.working_dir.as_deref(),
            )?;

            if !old_path.exists() {
                return Ok(ToolResult::failure(
                    ToolFailureCategory::InvalidArguments,
                    format!("Source file does not exist: {}", old_path.display()),
                ));
            }
            if !old_path.is_file() {
                return Ok(ToolResult::failure(
                    ToolFailureCategory::InvalidArguments,
                    format!("'{}' is not a file", old_path.display()),
                ));
            }
            if new_path.exists() {
                return Ok(ToolResult::failure(
                    ToolFailureCategory::InvalidArguments,
                    format!("Target path already exists: {}", new_path.display()),
                ));
            }
            // Auto-create target parent directory
            if let Some(parent) = new_path.parent() {
                fs::create_dir_all(parent)
                    .await
                    .map_err(|e| ToolError::ExecutionFailed {
                        tool: "move_file".to_string(),
                        message: format!("Failed to create target directory: {}", e),
                    })?;
            }

            fs::rename(&old_path, &new_path)
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: "move_file".to_string(),
                    message: format!(
                        "Failed to move file, old_path: {}, new_path: {}. err: {}",
                        old_path.display(),
                        new_path.display(),
                        e
                    ),
                })?;

            Ok(ToolResult::success(format!(
                "File moved successfully, old_path: {}, new_path: {}.",
                old_path.display(),
                new_path.display()
            )))
        })
    }
}
// ── ListDirTool ───────────────────────────────────────────────────────────────

/// List files and subdirectories in a directory
pub struct ListDirTool {
    base_dir: Option<PathBuf>,
}

impl ListDirTool {
    pub fn new() -> Self {
        Self { base_dir: None }
    }

    pub fn with_base_dir(base: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: Some(base.into()),
        }
    }
}

impl Default for ListDirTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for ListDirTool {
    fn name(&self) -> &str {
        "list_dir"
    }

    fn description(&self) -> &str {
        "List all files and subdirectories in a directory, returning a name list"
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Read]
    }
    fn risk_level(&self) -> ToolRiskLevel {
        ToolRiskLevel::ReadOnly
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Directory path to list, defaults to current directory"
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 200,
                    "description": "Entries per page (default 100)"
                },
                "cursor": {
                    "type": "string",
                    "description": "Cursor from the previous page"
                }
            },
            "required": []
        })
    }

    fn execute_with_context<'a>(
        &'a self,
        parameters: ToolParameters,
        ctx: &'a echo_core::tools::ToolContext,
    ) -> BoxFuture<'a, echo_core::error::Result<ToolResult>> {
        Box::pin(async move {
            let path_str = parameters
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or(".");
            let page_request = match PageRequest::from_parameters(&parameters, 100, 200) {
                Ok(request) => request,
                Err(error) => return Ok(ToolResult::invalid_arguments(error.to_string())),
            };

            let path = resolve_path(
                "list_dir",
                path_str,
                &self.base_dir,
                ctx.working_dir.as_deref(),
            )?;

            if !path.exists() {
                return Ok(ToolResult::failure(
                    ToolFailureCategory::InvalidArguments,
                    format!("Directory does not exist: {}", path.display()),
                ));
            }
            if !path.is_dir() {
                return Ok(ToolResult::failure(
                    ToolFailureCategory::InvalidArguments,
                    format!("'{}' is not a directory", path.display()),
                ));
            }

            let mut entries =
                tokio::fs::read_dir(&path)
                    .await
                    .map_err(|e| ToolError::ExecutionFailed {
                        tool: "list_dir".to_string(),
                        message: format!("Failed to read directory: {}", e),
                    })?;

            let mut files = Vec::new();
            let mut dirs = Vec::new();

            while let Some(entry) =
                entries
                    .next_entry()
                    .await
                    .map_err(|e| ToolError::ExecutionFailed {
                        tool: "list_dir".to_string(),
                        message: format!("Failed to iterate directory: {}", e),
                    })?
            {
                let name = entry.file_name().to_string_lossy().to_string();
                let file_type =
                    entry
                        .file_type()
                        .await
                        .map_err(|e| ToolError::ExecutionFailed {
                            tool: "list_dir".to_string(),
                            message: format!("Failed to get file type: {}", e),
                        })?;

                if file_type.is_dir() {
                    dirs.push(format!("[Dir] {}/", name));
                } else {
                    files.push(format!("[File] {}", name));
                }
            }

            dirs.sort();
            files.sort();

            let dir_count = dirs.len();
            let file_count = files.len();
            dirs.extend(files);
            let query = serde_json::json!({ "path": path });
            let (page, page_info) = match page_request.paginate(dirs, &query) {
                Ok(page) => page,
                Err(error) => return Ok(ToolResult::invalid_arguments(error.to_string())),
            };
            let mut output = if page.is_empty() {
                format!("Directory '{}' is empty", path.display())
            } else {
                format!("Directory '{}' contents:\n", path.display())
            };
            for entry in page {
                output.push_str(&format!("  {entry}\n"));
            }
            output.push_str(&format!(
                "\nTotal: {dir_count} dirs, {file_count} files; {} returned",
                page_info.returned
            ));
            let mut result = ToolResult::success(output);
            page_info.apply_to(&mut result);
            Ok(result)
        })
    }
}

#[cfg(test)]
mod worktree_cwd_tests {
    use super::*;
    use echo_core::tools::ToolContext;

    /// Unique temp dir without the `tempfile` dev-dependency.
    fn unique_dir(prefix: &str) -> PathBuf {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("{prefix}-{}-{}", std::process::id(), nanos));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn test_create_file_lands_in_context_working_dir() {
        // Regression for worktree cwd: a relative `path` must resolve under
        // ctx.working_dir, not the process cwd.
        let wt = unique_dir("echo-files-wt");
        let tool = CreateFileTool::new();
        let ctx = ToolContext {
            working_dir: Some(wt.clone()),
            ..Default::default()
        };

        let mut params = ToolParameters::new();
        params.insert("path".into(), serde_json::json!("sub/hello.txt"));

        let result = tool.execute_with_context(params, &ctx).await.unwrap();
        assert!(result.success, "create_file should succeed: {:?}", result);

        let written = wt.join("sub").join("hello.txt");
        assert!(
            written.exists(),
            "file should exist at {:?}, working_dir was {:?}",
            written,
            wt
        );
        // CreateFileTool creates an empty file; content is verified to be empty.
        assert_eq!(std::fs::read_to_string(&written).unwrap(), "");

        let _ = std::fs::remove_dir_all(&wt);
    }

    #[tokio::test]
    async fn test_list_dir_uses_context_working_dir() {
        // Pre-create a marker file in the worktree dir, then list_dir with "."
        // and ctx.working_dir set — the listing must include the marker.
        let wt = unique_dir("echo-files-list");
        std::fs::write(wt.join("marker.txt"), "m").unwrap();

        let tool = ListDirTool::new();
        let ctx = ToolContext {
            working_dir: Some(wt.clone()),
            ..Default::default()
        };

        let mut params = ToolParameters::new();
        params.insert("path".into(), serde_json::json!("."));

        let result = tool.execute_with_context(params, &ctx).await.unwrap();
        assert!(result.success, "list_dir should succeed: {:?}", result);
        assert!(
            result.output.contains("marker.txt"),
            "list_dir output should include marker.txt: {}",
            result.output
        );

        let _ = std::fs::remove_dir_all(&wt);
    }

    #[tokio::test]
    async fn write_file_rejects_stale_content_hash() -> Result<(), String> {
        let wt = unique_dir("echo-files-hash");
        let path = wt.join("tracked.txt");
        std::fs::write(&path, "first").map_err(|error| error.to_string())?;
        let context = ToolContext {
            working_dir: Some(wt.clone()),
            call_id: Some("call-write-1".to_string()),
            ..Default::default()
        };

        let read = ReadFileTool::new()
            .execute_with_context(
                ToolParameters::from([("path".to_string(), json!("tracked.txt"))]),
                &context,
            )
            .await
            .map_err(|error| error.to_string())?;
        let old_hash = read
            .metadata
            .get("content_hash")
            .ok_or_else(|| "read_file did not return content_hash".to_string())?
            .to_string();
        std::fs::write(&path, "external change").map_err(|error| error.to_string())?;

        let write = WriteFileTool::new()
            .execute_with_context(
                ToolParameters::from([
                    ("path".to_string(), json!("tracked.txt")),
                    ("content".to_string(), json!("agent change")),
                    ("expected_hash".to_string(), json!(old_hash)),
                ]),
                &context,
            )
            .await
            .map_err(|error| error.to_string())?;

        assert!(!write.success);
        assert_eq!(
            write.failure.map(|failure| failure.category),
            Some(ToolFailureCategory::InvalidArguments)
        );
        assert_eq!(
            std::fs::read_to_string(&path).map_err(|error| error.to_string())?,
            "external change"
        );
        let _ = std::fs::remove_dir_all(&wt);
        Ok(())
    }

    #[tokio::test]
    async fn read_file_default_page_is_compact_and_bounded() -> Result<(), String> {
        let wt = unique_dir("echo-files-read-page");
        let content = (1..=600)
            .map(|line| format!("line-{line}"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(wt.join("large.txt"), content).map_err(|error| error.to_string())?;
        let context = ToolContext {
            working_dir: Some(wt.clone()),
            ..Default::default()
        };

        let result = ReadFileTool::new()
            .execute_with_context(
                ToolParameters::from([("path".to_string(), json!("large.txt"))]),
                &context,
            )
            .await
            .map_err(|error| error.to_string())?;

        assert!(result.success);
        assert!(result.truncated);
        assert!(result.output.starts_with("1|line-1\n"));
        assert!(result.output.contains("500|line-500\n"));
        assert!(result.output.contains("continue with offset=501"));
        assert!(!result.output.contains("line_number"));
        assert!(!result.output.contains("content_hash"));
        assert!(result.output.len() < 32 * 1024);
        assert_eq!(
            result.metadata.get("end_line").map(String::as_str),
            Some("500")
        );
        assert_eq!(
            result.metadata.get("truncation_reason").map(String::as_str),
            Some("line_limit")
        );
        assert_eq!(
            result.metadata.get("content_hash").map(String::len),
            Some(64)
        );

        let _ = std::fs::remove_dir_all(&wt);
        Ok(())
    }

    #[tokio::test]
    async fn read_file_explicit_range_has_precise_continuation() -> Result<(), String> {
        let wt = unique_dir("echo-files-read-range");
        std::fs::write(wt.join("range.txt"), "alpha\nbeta\ngamma\ndelta")
            .map_err(|error| error.to_string())?;
        let context = ToolContext {
            working_dir: Some(wt.clone()),
            ..Default::default()
        };
        let result = ReadFileTool::new()
            .execute_with_context(
                ToolParameters::from([
                    ("path".to_string(), json!("range.txt")),
                    ("offset".to_string(), json!(2)),
                    ("limit".to_string(), json!(2)),
                ]),
                &context,
            )
            .await
            .map_err(|error| error.to_string())?;

        assert!(result.success);
        assert_eq!(
            result.output,
            "2|beta\n3|gamma\n[Partial: lines 2-3 of 4; continue with offset=4.]"
        );
        let _ = std::fs::remove_dir_all(&wt);
        Ok(())
    }

    #[tokio::test]
    async fn read_file_is_utf8_safe_and_rejects_oversized_single_lines() -> Result<(), String> {
        let wt = unique_dir("echo-files-read-utf8");
        std::fs::write(wt.join("utf8.txt"), "中文🙂\n第二行🚀")
            .map_err(|error| error.to_string())?;
        std::fs::write(wt.join("long.txt"), "界".repeat(9_000))
            .map_err(|error| error.to_string())?;
        let context = ToolContext {
            working_dir: Some(wt.clone()),
            ..Default::default()
        };

        let utf8 = ReadFileTool::new()
            .execute_with_context(
                ToolParameters::from([("path".to_string(), json!("utf8.txt"))]),
                &context,
            )
            .await
            .map_err(|error| error.to_string())?;
        assert!(utf8.success);
        assert_eq!(utf8.output, "1|中文🙂\n2|第二行🚀\n");

        let oversized = ReadFileTool::new()
            .execute_with_context(
                ToolParameters::from([("path".to_string(), json!("long.txt"))]),
                &context,
            )
            .await
            .map_err(|error| error.to_string())?;
        assert!(!oversized.success);
        assert!(
            oversized
                .error
                .as_deref()
                .is_some_and(|error| error.contains("Line 1") && error.contains("grep"))
        );

        let _ = std::fs::remove_dir_all(&wt);
        Ok(())
    }

    #[tokio::test]
    async fn read_file_rejects_unknown_and_invalid_parameters() -> Result<(), String> {
        let wt = unique_dir("echo-files-read-params");
        std::fs::write(wt.join("params.txt"), "content").map_err(|error| error.to_string())?;
        let context = ToolContext {
            working_dir: Some(wt.clone()),
            ..Default::default()
        };

        let unknown = ReadFileTool::new()
            .execute_with_context(
                ToolParameters::from([
                    ("path".to_string(), json!("params.txt")),
                    ("start_line".to_string(), json!(1)),
                ]),
                &context,
            )
            .await
            .map_err(|error| error.to_string())?;
        assert!(!unknown.success);
        assert!(
            unknown
                .error
                .as_deref()
                .is_some_and(|error| error.contains("start_line"))
        );

        let invalid = ReadFileTool::new()
            .execute_with_context(
                ToolParameters::from([
                    ("path".to_string(), json!("params.txt")),
                    ("limit".to_string(), json!(0)),
                ]),
                &context,
            )
            .await
            .map_err(|error| error.to_string())?;
        assert!(!invalid.success);
        assert!(
            invalid
                .error
                .as_deref()
                .is_some_and(|error| error.contains("positive integer"))
        );

        let _ = std::fs::remove_dir_all(&wt);
        Ok(())
    }

    #[tokio::test]
    async fn read_file_default_token_limit_pages_but_explicit_overflow_fails() -> Result<(), String>
    {
        let wt = unique_dir("echo-files-read-budget");
        let content = (1..=500)
            .map(|line| format!("{line}-{}", "x".repeat(100)))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(wt.join("budget.txt"), content).map_err(|error| error.to_string())?;
        let context = ToolContext {
            working_dir: Some(wt.clone()),
            ..Default::default()
        };

        let default_page = ReadFileTool::new()
            .execute_with_context(
                ToolParameters::from([("path".to_string(), json!("budget.txt"))]),
                &context,
            )
            .await
            .map_err(|error| error.to_string())?;
        assert!(default_page.success);
        assert!(default_page.truncated);
        assert_eq!(
            default_page
                .metadata
                .get("truncation_reason")
                .map(String::as_str),
            Some("token_budget")
        );
        assert!(default_page.output.len() < 32 * 1024);

        let explicit_range = ReadFileTool::new()
            .execute_with_context(
                ToolParameters::from([
                    ("path".to_string(), json!("budget.txt")),
                    ("limit".to_string(), json!(500)),
                ]),
                &context,
            )
            .await
            .map_err(|error| error.to_string())?;
        assert!(!explicit_range.success);
        assert!(
            explicit_range
                .error
                .as_deref()
                .is_some_and(|error| error.contains("Retry with limit="))
        );

        let _ = std::fs::remove_dir_all(&wt);
        Ok(())
    }
}
