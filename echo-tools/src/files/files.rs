use super::resolve_path;
use echo_core::error::ToolError;
use echo_core::tools::permission::ToolPermission;
use echo_core::tools::{Tool, ToolParameters, ToolResult, ToolRiskLevel};
use futures::future::BoxFuture;
use serde_json::{Value, json};
use std::path::PathBuf;
use tokio::fs;

// Re-export encoding_rs for encoding detection
use encoding_rs;
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

    fn execute(
        &self,
        parameters: ToolParameters,
    ) -> BoxFuture<'_, echo_core::error::Result<ToolResult>> {
        Box::pin(async move {
            let path_str = parameters
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("path".to_string()))?;

            let path = resolve_path("create_file", path_str, &self.base_dir)?;

            if path.exists() {
                return Ok(ToolResult::error(format!(
                    "File already exists: {}",
                    path.display()
                )));
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

    fn execute(
        &self,
        parameters: ToolParameters,
    ) -> BoxFuture<'_, echo_core::error::Result<ToolResult>> {
        Box::pin(async move {
            let path_str = parameters
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("path".to_string()))?;

            let path = resolve_path("delete_file", path_str, &self.base_dir)?;

            if !path.exists() {
                return Ok(ToolResult::error(format!(
                    "File does not exist: {}",
                    path.display()
                )));
            }
            if !path.is_file() {
                return Ok(ToolResult::error(format!(
                    "'{}' is not a file",
                    path.display()
                )));
            }

            // Create git checkpoint before deletion
            let checkpoint_tag = crate::git_checkpoint::create_checkpoint(&path);
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
/// Read file content with auto-encoding detection and structured output
///
/// Unified file reading tool with auto-encoding detection and structured JSON output.
/// Supports offset/limit for pagination and automatic GBK fallback encoding.
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
        "Read file content with auto-encoding detection. Supports offset/limit for reading specific sections. \
         Returns structured JSON with line numbers. Auto-detects UTF-8/GBK encoding."
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
                    "description": "File path to read (relative or absolute path). Also accepts 'file_path' for backward compatibility."
                },
                "file_path": {
                    "type": "string",
                    "description": "Alias for 'path' (backward compatibility)"
                },
                "offset": {
                    "type": "integer",
                    "description": "Line number to start reading from (1-based, default: 1). Also accepts 'start_line'."
                },
                "start_line": {
                    "type": "integer",
                    "description": "Alias for 'offset' (backward compatibility)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of lines to read (default: all lines, -1 means preview limit). Also accepts 'line_count'."
                },
                "line_count": {
                    "type": "integer",
                    "description": "Alias for 'limit' (backward compatibility)"
                },
                "encoding": {
                    "type": "string",
                    "description": "File encoding (e.g. 'utf-8', 'gbk'), default auto-detect"
                }
            },
            "required": ["path"]
        })
    }

    fn execute(
        &self,
        parameters: ToolParameters,
    ) -> BoxFuture<'_, echo_core::error::Result<ToolResult>> {
        Box::pin(async move {
            // Support both 'path' and 'file_path' parameters
            let path_str = parameters
                .get("path")
                .or_else(|| parameters.get("file_path"))
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("path".to_string()))?;

            // Support both 'offset' and 'start_line' parameters
            let offset = parameters
                .get("offset")
                .or_else(|| parameters.get("start_line"))
                .and_then(|v| v.as_u64())
                .unwrap_or(1) as usize;

            // Support both 'limit' and 'line_count' parameters
            let limit = parameters
                .get("limit")
                .or_else(|| parameters.get("line_count"))
                .and_then(|v| v.as_i64());

            let _encoding = parameters.get("encoding").and_then(|v| v.as_str());

            let path = resolve_path("read_file", path_str, &self.base_dir)?;

            if !path.exists() {
                return Ok(ToolResult::error(format!(
                    "File does not exist: {}",
                    path.display()
                )));
            }
            if !path.is_file() {
                return Ok(ToolResult::error(format!(
                    "'{}' is not a file",
                    path.display()
                )));
            }

            // Read file bytes
            let bytes = tokio::fs::read(&path)
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: "read_file".to_string(),
                    message: format!("Failed to read: {}", e),
                })?;

            // Auto-detect encoding (prefer UTF-8, fall back to GBK)
            let content = String::from_utf8(bytes.clone())
                .unwrap_or_else(|_| encoding_rs::GBK.decode(&bytes).0.into_owned());

            let lines: Vec<&str> = content.lines().collect();
            let total_lines = lines.len();

            // Calculate read range
            let start = if offset > 0 { offset - 1 } else { 0 };
            let effective_limit = if let Some(l) = limit {
                if l < 0 { 100 } else { l as usize } // -1 means preview limit (100 lines)
            } else {
                total_lines // Read all lines
            };

            let end = (start + effective_limit).min(total_lines);

            if start >= total_lines {
                return Ok(ToolResult::success_json(serde_json::json!({
                    "error": format!("Offset {} exceeds total lines ({})", offset, total_lines),
                    "total_lines": total_lines
                })));
            }

            // Build structured output
            let preview_lines: Vec<Value> = lines[start..end]
                .iter()
                .enumerate()
                .map(|(idx, line)| {
                    serde_json::json!({
                        "line_number": start + idx + 1,
                        "content": line,
                    })
                })
                .collect();

            let result = serde_json::json!({
                "file": path_str,
                "total_lines": total_lines,
                "start_line": start + 1,
                "end_line": end,
                "truncated": end < total_lines,
                "remaining_lines": total_lines.saturating_sub(end),
                "lines": preview_lines,
            });

            Ok(ToolResult::success_json(result))
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
                }
            },
            "required": ["path", "content"]
        })
    }

    fn execute(
        &self,
        parameters: ToolParameters,
    ) -> BoxFuture<'_, echo_core::error::Result<ToolResult>> {
        Box::pin(async move {
            let path_str = parameters
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("path".to_string()))?;

            let content = parameters
                .get("content")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("content".to_string()))?;

            let path = resolve_path("write_file", path_str, &self.base_dir)?;

            // Auto-create parent directory
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent).await.map_err(|e| {
                    ToolError::ExecutionFailed {
                        tool: "write_file".to_string(),
                        message: format!("Failed to create directory: {}", e),
                    }
                })?;
            }

            // Create git checkpoint before mutation (only if file already exists)
            let checkpoint_tag = if path.exists() {
                let tag = crate::git_checkpoint::create_checkpoint(&path);
                if tag.is_some() {
                    crate::git_checkpoint::cleanup_old_checkpoints(&path, 10);
                }
                tag
            } else {
                None
            };

            let bytes = content.len();
            tokio::fs::write(&path, content)
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: "write_file".to_string(),
                    message: format!("Failed to write: {}", e),
                })?;

            let mut result = ToolResult::success(format!(
                "Successfully wrote {} bytes to '{}'",
                bytes,
                path.display()
            ));

            if let Some(tag) = checkpoint_tag {
                result = result.with_meta("git_checkpoint", tag);
            }

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

    fn execute(
        &self,
        parameters: ToolParameters,
    ) -> BoxFuture<'_, echo_core::error::Result<ToolResult>> {
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

            let path = resolve_path("append_file", path_str, &self.base_dir)?;

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

    fn execute(
        &self,
        parameters: ToolParameters,
    ) -> BoxFuture<'_, echo_core::error::Result<ToolResult>> {
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

            let path = resolve_path("update_file", path_str, &self.base_dir)?;

            if !path.exists() {
                return Ok(ToolResult::error(format!(
                    "File does not exist: {}",
                    path.display()
                )));
            }

            let content =
                tokio::fs::read_to_string(&path)
                    .await
                    .map_err(|e| ToolError::ExecutionFailed {
                        tool: "update_file".to_string(),
                        message: format!("Failed to read file: {}", e),
                    })?;

            if !content.contains(old_content) {
                return Ok(ToolResult::error(format!(
                    "Specified content not found in file, replacement failed: {}",
                    path.display()
                )));
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

    fn execute(
        &self,
        parameters: ToolParameters,
    ) -> BoxFuture<'_, echo_core::error::Result<ToolResult>> {
        Box::pin(async move {
            let old_path_str = parameters
                .get("old_path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("old_path".to_string()))?;

            let new_path_str = parameters
                .get("new_path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("new_path".to_string()))?;

            let old_path = resolve_path("move_file", old_path_str, &self.base_dir)?;
            let new_path = resolve_path("move_file", new_path_str, &self.base_dir)?;

            if !old_path.exists() {
                return Ok(ToolResult::error(format!(
                    "Source file does not exist: {}",
                    old_path.display()
                )));
            }
            if !old_path.is_file() {
                return Ok(ToolResult::error(format!(
                    "'{}' is not a file",
                    old_path.display()
                )));
            }
            if new_path.exists() {
                return Ok(ToolResult::error(format!(
                    "Target path already exists: {}",
                    new_path.display()
                )));
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
                }
            },
            "required": []
        })
    }

    fn execute(
        &self,
        parameters: ToolParameters,
    ) -> BoxFuture<'_, echo_core::error::Result<ToolResult>> {
        Box::pin(async move {
            let path_str = parameters
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or(".");

            let path = resolve_path("list_dir", path_str, &self.base_dir)?;

            if !path.exists() {
                return Ok(ToolResult::error(format!(
                    "Directory does not exist: {}",
                    path.display()
                )));
            }
            if !path.is_dir() {
                return Ok(ToolResult::error(format!(
                    "'{}' is not a directory",
                    path.display()
                )));
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

            if dirs.is_empty() && files.is_empty() {
                return Ok(ToolResult::success(format!(
                    "Directory '{}' is empty",
                    path.display()
                )));
            }

            let mut output = format!("Directory '{}' contents:\n", path.display());
            for d in &dirs {
                output.push_str(&format!("  {}\n", d));
            }
            for f in &files {
                output.push_str(&format!("  {}\n", f));
            }
            output.push_str(&format!(
                "\nTotal: {} dirs, {} files",
                dirs.len(),
                files.len()
            ));

            Ok(ToolResult::success(output))
        })
    }
}
