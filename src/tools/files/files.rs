use crate::error::ToolError;
use crate::prelude::{Tool, ToolParameters, ToolResult};
use crate::tools::files::resolve_path;
use async_trait::async_trait;
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

#[async_trait]
impl Tool for CreateFileTool {
    fn name(&self) -> &str {
        "create_file"
    }

    fn description(&self) -> &str {
        "创建指定文件。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "需要创建的文件路径（相对路径或绝对路径）"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, parameters: ToolParameters) -> crate::error::Result<ToolResult> {
        let path_str = parameters
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::MissingParameter("path".to_string()))?;

        let path = resolve_path("create_file", path_str, &self.base_dir)?;

        if path.exists() {
            return Ok(ToolResult::error(format!("文件已存在: {}", path.display())));
        }

        // 自动创建父目录
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: "create_file".to_string(),
                    message: format!("创建目录失败: {}", e),
                })?;
        }

        tokio::fs::write(&path, "")
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                tool: "create_file".to_string(),
                message: format!("创建文件失败: {}", e),
            })?;

        Ok(ToolResult::success(format!(
            "创建文件:{} 成功。",
            path.display()
        )))
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

#[async_trait]
impl Tool for DeleteFileTool {
    fn name(&self) -> &str {
        "delete_file"
    }

    fn description(&self) -> &str {
        "删除指定文件。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "需要删除的文件路径（相对路径或绝对路径）"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, parameters: ToolParameters) -> crate::error::Result<ToolResult> {
        let path_str = parameters
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::MissingParameter("path".to_string()))?;

        let path = resolve_path("delete_file", path_str, &self.base_dir)?;

        if !path.exists() {
            return Ok(ToolResult::error(format!("文件不存在: {}", path.display())));
        }
        if !path.is_file() {
            return Ok(ToolResult::error(format!("'{}' 不是文件", path.display())));
        }

        tokio::fs::remove_file(&path)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                tool: "delete_file".to_string(),
                message: format!("删除失败: {}", e),
            })?;

        Ok(ToolResult::success(format!(
            "删除文件:{} 成功。",
            path.display()
        )))
    }
}

// ── ReadFileTool ──────────────────────────────────────────────────────────────
/// 读取文件内容
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

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "读取指定路径的文件内容，返回文本内容"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "要读取的文件路径（相对路径或绝对路径）"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, parameters: ToolParameters) -> crate::error::Result<ToolResult> {
        let path_str = parameters
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::MissingParameter("path".to_string()))?;

        let path = resolve_path("read_file", path_str, &self.base_dir)?;

        if !path.exists() {
            return Ok(ToolResult::error(format!("文件不存在: {}", path.display())));
        }
        if !path.is_file() {
            return Ok(ToolResult::error(format!("'{}' 不是文件", path.display())));
        }

        let content =
            tokio::fs::read_to_string(&path)
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: "read_file".to_string(),
                    message: format!("读取失败: {}", e),
                })?;

        Ok(ToolResult::success(content))
    }
}

// ── WriteFileTool ─────────────────────────────────────────────────────────────

/// 写入（覆盖）文件内容，若目录不存在则自动创建
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

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        "将内容写入指定路径的文件（覆盖写），若目录不存在则自动创建"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "要写入的文件路径"
                },
                "content": {
                    "type": "string",
                    "description": "要写入的文本内容"
                }
            },
            "required": ["path", "content"]
        })
    }

    async fn execute(&self, parameters: ToolParameters) -> crate::error::Result<ToolResult> {
        let path_str = parameters
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::MissingParameter("path".to_string()))?;

        let content = parameters
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::MissingParameter("content".to_string()))?;

        let path = resolve_path("write_file", path_str, &self.base_dir)?;

        // 自动创建父目录
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: "write_file".to_string(),
                    message: format!("创建目录失败: {}", e),
                })?;
        }

        let bytes = content.len();
        tokio::fs::write(&path, content)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                tool: "write_file".to_string(),
                message: format!("写入失败: {}", e),
            })?;

        Ok(ToolResult::success(format!(
            "已成功写入 {} 字节到 '{}'",
            bytes,
            path.display()
        )))
    }
}

// ── AppendFileTool ────────────────────────────────────────────────────────────

/// 追加内容到文件末尾
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

#[async_trait]
impl Tool for AppendFileTool {
    fn name(&self) -> &str {
        "append_file"
    }

    fn description(&self) -> &str {
        "将内容追加到文件末尾（文件不存在时自动创建）"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "目标文件路径"
                },
                "content": {
                    "type": "string",
                    "description": "要追加的文本内容"
                }
            },
            "required": ["path", "content"]
        })
    }

    async fn execute(&self, parameters: ToolParameters) -> crate::error::Result<ToolResult> {
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
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: "append_file".to_string(),
                    message: format!("创建目录失败: {}", e),
                })?;
        }

        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                tool: "append_file".to_string(),
                message: format!("打开文件失败: {}", e),
            })?;

        file.write_all(content.as_bytes())
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                tool: "append_file".to_string(),
                message: format!("追加写入失败: {}", e),
            })?;

        Ok(ToolResult::success(format!(
            "已追加 {} 字节到 '{}'",
            content.len(),
            path.display()
        )))
    }
}

// ── UpdateFileTool ────────────────────────────────────────────────────────────────

/// 更新文件内容
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

#[async_trait]
impl Tool for UpdateFileTool {
    fn name(&self) -> &str {
        "update_file"
    }

    fn description(&self) -> &str {
        "更新文件内容，即用新内容替换旧内容。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "目标文件路径"
                },
                "old_content": {
                    "type": "string",
                    "description": "旧文件内容，废弃不用的内容。"
                },
                "new_content": {
                    "type": "string",
                    "description": "新文件内容，最新生成的文件内容"
                }
            },
            "required": ["path", "old_content", "new_content"]
        })
    }

    async fn execute(&self, parameters: ToolParameters) -> crate::error::Result<ToolResult> {
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
            return Ok(ToolResult::error(format!("文件不存在: {}", path.display())));
        }

        let content =
            tokio::fs::read_to_string(&path)
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: "update_file".to_string(),
                    message: format!("读取文件失败: {}", e),
                })?;

        if !content.contains(old_content) {
            return Ok(ToolResult::error(format!(
                "文件中未找到指定内容，替换失败: {}",
                path.display()
            )));
        }

        let updated = content.replacen(old_content, new_content, 1);

        tokio::fs::write(&path, &updated)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                tool: "update_file".to_string(),
                message: format!("更新写入失败: {}", e),
            })?;

        Ok(ToolResult::success(format!(
            "已更新文件: {}，替换成功。",
            path.display()
        )))
    }
}
// ── MoveFileTool ──────────────────────────────────────────────────────────────────
/// 移动文件到新路径
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

#[async_trait]
impl Tool for MoveFileTool {
    fn name(&self) -> &str {
        "move_file"
    }

    fn description(&self) -> &str {
        "移动文件到新路径"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "old_path": {
                    "type": "string",
                    "description": "旧文件路径"
                },"new_path": {
                    "type": "string",
                    "description": "新文件路径"
                }
            },
            "required": ["old_path","new_path"]
        })
    }

    async fn execute(&self, parameters: ToolParameters) -> crate::error::Result<ToolResult> {
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
                "源文件不存在: {}",
                old_path.display()
            )));
        }
        if !old_path.is_file() {
            return Ok(ToolResult::error(format!(
                "'{}' 不是文件",
                old_path.display()
            )));
        }
        if new_path.exists() {
            return Ok(ToolResult::error(format!(
                "目标路径已存在: {}",
                new_path.display()
            )));
        }
        // 自动创建目标父目录
        if let Some(parent) = new_path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: "move_file".to_string(),
                    message: format!("创建目标目录失败: {}", e),
                })?;
        }

        fs::rename(&old_path, &new_path)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                tool: "move_file".to_string(),
                message: format!(
                    "移动文件失败，old_path: {}，new_path:{}。err:{}",
                    old_path.display(),
                    new_path.display(),
                    e
                ),
            })?;

        Ok(ToolResult::success(format!(
            "移动文件成功，old_path: {}，new_path:{}。",
            old_path.display(),
            new_path.display()
        )))
    }
}
// ── ListDirTool ───────────────────────────────────────────────────────────────

/// 列出目录中的文件和子目录
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

#[async_trait]
impl Tool for ListDirTool {
    fn name(&self) -> &str {
        "list_dir"
    }

    fn description(&self) -> &str {
        "列出目录中的所有文件和子目录，返回名称列表"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "要列出的目录路径，默认为当前目录"
                }
            },
            "required": []
        })
    }

    async fn execute(&self, parameters: ToolParameters) -> crate::error::Result<ToolResult> {
        let path_str = parameters
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or(".");

        let path = resolve_path("list_dir", path_str, &self.base_dir)?;

        if !path.exists() {
            return Ok(ToolResult::error(format!("目录不存在: {}", path.display())));
        }
        if !path.is_dir() {
            return Ok(ToolResult::error(format!("'{}' 不是目录", path.display())));
        }

        let mut entries =
            tokio::fs::read_dir(&path)
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: "list_dir".to_string(),
                    message: format!("读取目录失败: {}", e),
                })?;

        let mut files = Vec::new();
        let mut dirs = Vec::new();

        while let Some(entry) =
            entries
                .next_entry()
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: "list_dir".to_string(),
                    message: format!("遍历目录失败: {}", e),
                })?
        {
            let name = entry.file_name().to_string_lossy().to_string();
            let file_type = entry
                .file_type()
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: "list_dir".to_string(),
                    message: format!("获取文件类型失败: {}", e),
                })?;

            if file_type.is_dir() {
                dirs.push(format!("[目录] {}/", name));
            } else {
                files.push(format!("[文件] {}", name));
            }
        }

        dirs.sort();
        files.sort();

        if dirs.is_empty() && files.is_empty() {
            return Ok(ToolResult::success(format!(
                "目录 '{}' 为空",
                path.display()
            )));
        }

        let mut output = format!("目录 '{}' 内容：\n", path.display());
        for d in &dirs {
            output.push_str(&format!("  {}\n", d));
        }
        for f in &files {
            output.push_str(&format!("  {}\n", f));
        }
        output.push_str(&format!(
            "\n共 {} 个目录，{} 个文件",
            dirs.len(),
            files.len()
        ));

        Ok(ToolResult::success(output))
    }
}
