use super::resolve_path;
use echo_core::error::ToolError;
use echo_core::tools::permission::ToolPermission;
use echo_core::tools::{Tool, ToolParameters, ToolResult, ToolRiskLevel};
use futures::future::BoxFuture;
use serde_json::{Value, json};
use std::path::PathBuf;
use tokio::fs;
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

            tokio::fs::remove_file(&path)
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: "delete_file".to_string(),
                    message: format!("Failed to delete: {}", e),
                })?;

            Ok(ToolResult::success(format!(
                "File deleted successfully: {}",
                path.display()
            )))
        })
    }
}

// ── ReadFileTool ──────────────────────────────────────────────────────────────
/// Read file content
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
        "Read file content from the specified path, returns text content with line numbers. \
         Supports offset/limit for reading specific sections of large files."
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
                    "description": "File path to read (relative or absolute path)"
                },
                "offset": {
                    "type": "integer",
                    "description": "Line number to start reading from (1-based, default: 1)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of lines to read (default: all lines)"
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

            let offset = parameters
                .get("offset")
                .and_then(|v| v.as_u64())
                .unwrap_or(1) as usize;
            let limit = parameters.get("limit").and_then(|v| v.as_u64()).map(|l| l as usize);

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

            // Size guard: warn for files > 100KB
            let metadata = tokio::fs::metadata(&path).await.map_err(|e| {
                ToolError::ExecutionFailed {
                    tool: "read_file".to_string(),
                    message: format!("Failed to read metadata: {}", e),
                }
            })?;
            let file_size = metadata.len();
            if file_size > 100 * 1024 && limit.is_none() && offset == 1 {
                // For large files without offset/limit, read only first 2000 lines
                let content = tokio::fs::read_to_string(&path).await.map_err(|e| {
                    ToolError::ExecutionFailed {
                        tool: "read_file".to_string(),
                        message: format!("Failed to read: {}", e),
                    }
                })?;
                let lines: Vec<&str> = content.lines().collect();
                let total_lines = lines.len();
                let end = 2000.min(total_lines);
                let mut output = String::new();
                for (i, line) in lines.iter().take(end).enumerate() {
                    output.push_str(&format!("{:>6}\t{}\n", i + 1, line));
                }
                if total_lines > end {
                    output.push_str(&format!(
                        "\n... (showing first {end} of {total_lines} lines, file size: {} bytes. Use offset/limit to read more.)",
                        file_size
                    ));
                }
                return Ok(ToolResult::success(output));
            }

            let content = tokio::fs::read_to_string(&path).await.map_err(|e| {
                ToolError::ExecutionFailed {
                    tool: "read_file".to_string(),
                    message: format!("Failed to read: {}", e),
                }
            })?;

            let all_lines: Vec<&str> = content.lines().collect();
            let total_lines = all_lines.len();

            // offset is 1-based
            let start = if offset > 0 { offset - 1 } else { 0 };
            let end = match limit {
                Some(l) => (start + l).min(total_lines),
                None => total_lines,
            };

            if start >= total_lines {
                return Ok(ToolResult::success(format!(
                    "Offset {offset} exceeds total lines ({total_lines})"
                )));
            }

            let mut output = String::new();
            for (i, line) in all_lines[start..end].iter().enumerate() {
                output.push_str(&format!("{:>6}\t{}\n", start + i + 1, line));
            }

            if start > 0 || end < total_lines {
                output.push_str(&format!(
                    "\n(lines {start_offset}-{end_offset} of {total_lines})",
                    start_offset = start + 1,
                    end_offset = end,
                ));
            }

            Ok(ToolResult::success(output))
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

            let bytes = content.len();
            tokio::fs::write(&path, content)
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: "write_file".to_string(),
                    message: format!("Failed to write: {}", e),
                })?;

            Ok(ToolResult::success(format!(
                "Successfully wrote {} bytes to '{}'",
                bytes,
                path.display()
            )))
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
