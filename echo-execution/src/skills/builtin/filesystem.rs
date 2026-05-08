use std::path::PathBuf;

use echo_core::tools::Tool;
use echo_core::tools::skill::Skill;
use echo_tools::files::diff::DiffTool;
use echo_tools::files::edit::EditFileTool;
use echo_tools::files::files::{
    AppendFileTool, CreateFileTool, DeleteFileTool, ListDirTool, MoveFileTool, ReadFileTool,
    UpdateFileTool, WriteFileTool,
};
use echo_tools::files::glob::GlobTool;
use echo_tools::files::grep::GrepTool;

/// Filesystem skill
///
/// Provides the Agent with local filesystem read/write capabilities:
/// - `create_file`: create files
/// - `delete_file`: delete files
/// - `read_file`: read file content
/// - `write_file`: overwrite file
/// - `update_file`: update file
/// - `append_file`: append to file
/// - `move_file`: move files
/// - `list_dir`: list directory contents
///
/// # Security
/// Use `with_base_dir()` to restrict the Agent to only access a specified
/// directory and its subdirectories, preventing path traversal attacks
/// (`../../../etc/passwd`, etc.).
///
/// # Usage
/// ```rust,ignore
/// use echo_agent::prelude::{FileSystemSkill, AgentConfig, ReactAgent};
///
/// let config = AgentConfig::new("qwen3-max", "filesystem", "You are a file manager");
/// let mut agent = ReactAgent::new(config);
/// // No path restriction (use with caution)
/// agent.add_skill(Box::new(FileSystemSkill::new()));
///
/// // Restrict to /workspace directory
/// agent.add_skill(Box::new(FileSystemSkill::with_base_dir("/workspace")));
/// ```
pub struct FileSystemSkill {
    base_dir: Option<PathBuf>,
}

impl FileSystemSkill {
    /// Create a filesystem Skill without path restrictions
    pub fn new() -> Self {
        Self { base_dir: None }
    }

    /// Create a filesystem Skill restricted to a specified directory
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
        "Local filesystem read/write capability: create files, delete files, move file paths, read file content, write file content, append to files, modify file content, and list directory contents"
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
            Box::new(match &base {
                Some(b) => GrepTool::with_base_dir(b),
                None => GrepTool::new(),
            }),
            Box::new(match &base {
                Some(b) => GlobTool::with_base_dir(b),
                None => GlobTool::new(),
            }),
            Box::new(match &base {
                Some(b) => EditFileTool::with_base_dir(b),
                None => EditFileTool::new(),
            }),
            Box::new(match &base {
                Some(b) => DiffTool::with_base_dir(b),
                None => DiffTool::new(),
            }),
        ]
    }

    fn system_prompt_injection(&self) -> Option<String> {
        let restriction = if let Some(base) = &self.base_dir {
            format!(" (operations restricted to '{}' directory)", base.display())
        } else {
            " (no path restriction, exercise caution when operating)".to_string()
        };

        Some(format!(
            "\n\n## Filesystem Capability (FileSystem Skill){restriction}\n\
             You can operate on the local filesystem. Please use the following tools appropriately:\n\
             - `create_file(path)`: Create a file, suitable for creating an empty file, etc.\n\
             - `delete_file(path)`: Delete a file, suitable for removing unwanted old files such as configs, logs, code, etc.\n\
             - `move_file(old_path, new_path)`: Move a file path, for relocating files\n\
             - `read_file(path)`: Read file content, suitable for viewing configs, logs, code, etc.\n\
             - `write_file(path, content)`: Overwrite file content, clears existing content\n\
             - `edit_file(path, old_content, new_content, replace_all?, dry_run?)`: Edit file content — replace old_content with new_content (exact match). Returns unified diff. Use replace_all=true for multiple occurrences, dry_run=true to preview.\n\
             - `update_file(path, old_content, new_content)`: Modify file content, replace old content with new content (exact match, first occurrence)\n\
             - `append_file(path, content)`: Append content to the end of a file, does not clear existing content\n\
             - `diff(path_a, path_b?, content_b?, context?)`: Compare two files or a file against a string, returns unified diff.\n\
             - `list_dir(path)`: List files and subdirectories in a directory\n\
             - `grep(pattern, path?, glob?, case_insensitive?, context?, max_results?)`: Search file contents by regex pattern. Use glob to filter file types (e.g. '*.rs').\n\
             - `glob(pattern, path?, max_results?)`: Find files by name pattern (e.g. '**/*.rs', 'src/**/*.ts').\n\
             **Note**: Prefer `edit_file` over `update_file` for code editing — it provides diff output, replace_all, dry_run, and better error messages. Use `write_file` only for new files or complete rewrites."
        ))
    }
}
