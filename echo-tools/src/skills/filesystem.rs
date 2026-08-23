use std::path::PathBuf;

use echo_core::tools::Tool;
use echo_core::tools::skill::Skill;

use crate::files::apply_patch::ApplyPatchTool;
use crate::files::artifact::ReadArtifactTool;
use crate::files::diff::DiffTool;
use crate::files::files::{ListDirTool, ReadFileTool};
use crate::files::glob::GlobTool;
use crate::files::grep::GrepTool;

/// Filesystem skill with one canonical transactional mutation surface.
pub struct FileSystemSkill {
    base_dir: Option<PathBuf>,
}

impl FileSystemSkill {
    pub fn new() -> Self {
        Self { base_dir: None }
    }

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
        let restriction = self.base_dir.as_ref().map_or_else(
            || " (no path restriction, exercise caution when operating)".to_string(),
            |base| format!(" (operations restricted to '{}' directory)", base.display()),
        );
        Some(format!(
            "\n\n## Filesystem Capability{restriction}\nUse `apply_patch` for mutations and `read_file`, `read_artifact`, `diff`, `list_dir`, `grep`, and `glob` for inspection."
        ))
    }
}
