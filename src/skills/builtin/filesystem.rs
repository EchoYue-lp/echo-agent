use std::path::PathBuf;

use crate::skills::Skill;
use crate::tools::Tool;
use crate::tools::files::files::{
    AppendFileTool, CreateFileTool, DeleteFileTool, ListDirTool, MoveFileTool, ReadFileTool,
    UpdateFileTool, WriteFileTool,
};

/// 文件系统技能
///
/// 为 Agent 提供本地文件系统读写能力：
/// - `create_file`：创建文件
/// - `delete_file`：删除文件
/// - `read_file`：读取文件内容
/// - `write_file`：覆盖写入文件
/// - `update_file`：更新文件
/// - `append_file`：追加写入文件
/// - `move_file`：移动文件
/// - `list_dir`：列出目录内容
///
/// # 安全说明
/// 通过 `with_base_dir()` 可限制 Agent 只能访问指定目录及其子目录，
/// 防止路径穿越攻击（`../../../etc/passwd` 等）。
///
/// # 使用方式
/// ```rust
/// // 不限制路径（谨慎使用）
/// agent.add_skill(Box::new(FileSystemSkill::new()));
///
/// // 限制在 /workspace 目录下
/// agent.add_skill(Box::new(FileSystemSkill::with_base_dir("/workspace")));
/// ```
pub struct FileSystemSkill {
    base_dir: Option<PathBuf>,
}

impl FileSystemSkill {
    /// 创建不限制路径的文件系统 Skill
    pub fn new() -> Self {
        Self { base_dir: None }
    }

    /// 创建限制在指定目录下的文件系统 Skill
    pub fn with_base_dir(base: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: Some(base.into()),
        }
    }
}

impl Default for FileSystemSkill {
    fn default() -> Self {
        Self::new()
    }
}

impl Skill for FileSystemSkill {
    fn name(&self) -> &str {
        "filesystem"
    }

    fn description(&self) -> &str {
        "本地文件系统读写能力：创建文件、删除文件、移动文件路径、读取文件内容、写入文件内容、追加文件、修改文件内容，以及列出目录内容"
    }

    fn tools(&self) -> Vec<Box<dyn Tool>> {
        let base = self.base_dir.clone();
        vec![
            Box::new(match &base {
                Some(b) => ReadFileTool::with_base_dir(b),
                None => ReadFileTool::new(),
            }),
            Box::new(match &base {
                Some(b) => WriteFileTool::with_base_dir(b),
                None => WriteFileTool::new(),
            }),
            Box::new(match &base {
                Some(b) => AppendFileTool::with_base_dir(b),
                None => AppendFileTool::new(),
            }),
            Box::new(match &base {
                Some(b) => ListDirTool::with_base_dir(b),
                None => ListDirTool::new(),
            }),
            Box::new(match &base {
                Some(b) => CreateFileTool::with_base_dir(b),
                None => CreateFileTool::new(),
            }),
            Box::new(match &base {
                Some(b) => DeleteFileTool::with_base_dir(b),
                None => DeleteFileTool::new(),
            }),
            Box::new(match &base {
                Some(b) => UpdateFileTool::with_base_dir(b),
                None => UpdateFileTool::new(),
            }),
            Box::new(match &base {
                Some(b) => MoveFileTool::with_base_dir(b),
                None => MoveFileTool::new(),
            }),
        ]
    }

    fn system_prompt_injection(&self) -> Option<String> {
        let restriction = if let Some(base) = &self.base_dir {
            format!("（操作范围限制在 '{}' 目录下）", base.display())
        } else {
            "（无路径限制，操作时请谨慎）".to_string()
        };

        Some(format!(
            "\n\n## 文件系统能力（FileSystem Skill）{restriction}\n\
             你可以操作本地文件系统，请合理使用以下工具：\n\
             - `create_file(path)`：创建文件，适合创建一个空文件等\n\
             - `delete_file(path)`：删除文件，适合删除 配置、日志、代码等不需要的旧文件\n\
             - `move_file(old_path, new_path)`：移动文件路径，需要移动文件路径等\n\
             - `read_file(path)`：读取文件内容，适合查看配置、日志、代码等\n\
             - `write_file(path, content)`：覆盖写入文件，会清空原有内容\n\
             - `update_file(path, old_content, new_content)`：修改文件内容，用新内容替换旧内容（精确替换，首次匹配）\n\
             - `append_file(path, content)`：在文件末尾追加内容，不会清空原有内容\n\
             - `list_dir(path)`：列出目录下的文件和子目录\n\
             **注意**：write_file 会覆盖原文件，如需保留原内容请先 read_file 再决定使用 write_file 还是 append_file。"
        ))
    }
}
