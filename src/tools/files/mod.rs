use std::path::{Component, Path, PathBuf};

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::error::{Result, ToolError};
use crate::tools::{Tool, ToolParameters, ToolResult};

// ── 路径安全辅助函数 ──────────────────────────────────────────────────────────

/// 将用户提供的相对/绝对路径解析为安全的绝对路径。
/// 若设置了 base_dir，则限制在其下；否则直接使用原路径。
///
/// - 绝对路径：规范化后直接校验是否在 base_dir 内
/// - 相对路径：以 base_dir 为根展开后校验
fn resolve_path(tool: &str, path_str: &str, base_dir: &Option<PathBuf>) -> Result<PathBuf> {
    let requested = Path::new(path_str);

    let resolved = if let Some(base) = base_dir {
        let normalized_base = normalize_path(base);

        // 相对路径以 base_dir 为根展开；绝对路径直接规范化
        let normalized = if requested.is_absolute() {
            normalize_path(requested)
        } else {
            normalize_path(&normalized_base.join(requested))
        };

        if !normalized.starts_with(&normalized_base) {
            return Err(ToolError::ExecutionFailed {
                tool: tool.to_string(),
                message: format!("路径 '{}' 超出允许的目录范围", path_str),
            }
            .into());
        }
        normalized
    } else {
        normalize_path(requested)
    };

    Ok(resolved)
}

/// 不依赖文件系统的路径规范化（消除 `.` 和 `..`）
fn normalize_path(path: &Path) -> PathBuf {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                if let Some(Component::Normal(_)) = components.last() {
                    components.pop();
                }
            }
            Component::CurDir => {}
            c => components.push(c),
        }
    }
    components.iter().collect()
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

    async fn execute(&self, parameters: ToolParameters) -> Result<ToolResult> {
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

    async fn execute(&self, parameters: ToolParameters) -> Result<ToolResult> {
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

    async fn execute(&self, parameters: ToolParameters) -> Result<ToolResult> {
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

    async fn execute(&self, parameters: ToolParameters) -> Result<ToolResult> {
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
