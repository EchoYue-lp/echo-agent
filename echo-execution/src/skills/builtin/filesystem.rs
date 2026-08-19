use std::path::PathBuf;

use echo_core::tools::Tool;
use echo_core::tools::skill::Skill;
use echo_tools::files::apply_patch::ApplyPatchTool;
use echo_tools::files::artifact::ReadArtifactTool;
use echo_tools::files::diff::DiffTool;
use echo_tools::files::files::{ListDirTool, ReadFileTool};
use echo_tools::files::glob::GlobTool;
use echo_tools::files::grep::GrepTool;

/// Filesystem skill with one canonical transactional mutation surface.
///
/// `apply_patch` handles file creation, update, move, and deletion. Read,
/// search, listing, artifact paging, and diff remain separate because their
/// schemas express distinct read-only operations.
pub struct FileSystemSkill {
    base_dir: Option<PathBuf>,
}

impl FileSystemSkill {
    /// Create a filesystem Skill without path restrictions.
    pub fn new() -> Self {
        Self { base_dir: None }
    }

    /// Create a filesystem Skill restricted to a specified directory.
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
        "Local filesystem capability with transactional patch editing, file reading, search, diff, and directory listing"
    }

    fn tools(&self) -> Vec<Box<dyn Tool>> {
        let base = self.base_dir.clone();
        vec![
            Box::new(match &base {
                Some(base) => ReadFileTool::with_base_dir(base),
                None => ReadFileTool::new(),
            }),
            Box::new(ReadArtifactTool),
            Box::new(match &base {
                Some(base) => ApplyPatchTool::with_base_dir(base),
                None => ApplyPatchTool::new(),
            }),
            Box::new(match &base {
                Some(base) => ListDirTool::with_base_dir(base),
                None => ListDirTool::new(),
            }),
            Box::new(match &base {
                Some(base) => GrepTool::with_base_dir(base),
                None => GrepTool::new(),
            }),
            Box::new(match &base {
                Some(base) => GlobTool::with_base_dir(base),
                None => GlobTool::new(),
            }),
            Box::new(match &base {
                Some(base) => DiffTool::with_base_dir(base),
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
             You can operate on the local filesystem with these tools:\n\
             - `apply_patch(patch, dry_run?)`: Transactionally add, update, move, or delete one or more files using the canonical `*** Begin Patch` format. Returns a unified diff.\n\
             - `read_file(path)`: Read file content.\n\
             - `read_artifact(path, cursor?, max_tokens?, expected_sha256?)`: Read complete spilled tool output in bounded pages.\n\
             - `diff(path_a, path_b?, content_b?, context?, limit?, cursor?)`: Compare files or content, returning unified-diff hunks by page.\n\
             - `list_dir(path, limit?, cursor?)`: List files and subdirectories by page.\n\
             - `grep(pattern, path?, glob?, case_insensitive?, context?, limit?, cursor?)`: Exact regex text search.\n\
             - `glob(pattern, path?, limit?, cursor?)`: Find file names by pattern."
        ))
    }
}
