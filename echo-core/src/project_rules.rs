//! Project-scoped instruction discovery with explicit repository boundaries.

use std::path::{Path, PathBuf};

const NATIVE_FILES: &[(&str, &str)] = &[
    (".echo-agent/AGENT.md", "echo_agent"),
    (".echo-agent/RULES.md", "echo_agent"),
    (".echo-agent/rules.md", "echo_agent"),
];
const COMPATIBLE_FILES: &[(&str, &str)] = &[
    ("AGENTS.override.md", "agents_override"),
    ("AGENTS.md", "agents"),
    ("CLAUDE.md", "claude"),
];
const AGENTS_FILES: &[(&str, &str)] = &[
    ("AGENTS.override.md", "agents_override"),
    ("AGENTS.md", "agents"),
];

#[derive(Debug, Clone, Copy, Default)]
enum InstructionFileSet {
    #[default]
    All,
    AgentsOnly,
}

/// One instruction file included in the resolved chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstructionSource {
    pub path: PathBuf,
    pub kind: String,
    /// Root-to-leaf order. Larger values are closer to the working directory.
    pub precedence: usize,
}

/// Fully resolved project instructions plus diagnostics.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedInstructions {
    pub content: String,
    pub sources: Vec<InstructionSource>,
    pub project_root: Option<PathBuf>,
}

impl ResolvedInstructions {
    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }

    /// Render the resolved chain with source annotations for diagnostics.
    pub fn annotated(&self) -> Option<String> {
        (!self.content.is_empty()).then(|| self.content.clone())
    }
}

/// Resolves native and compatible instruction files for one working directory.
#[derive(Debug, Clone)]
pub struct InstructionResolver {
    working_dir: PathBuf,
    explicit_project_root: Option<PathBuf>,
    file_set: InstructionFileSet,
}

impl InstructionResolver {
    pub fn new(working_dir: impl Into<PathBuf>) -> Self {
        Self {
            working_dir: working_dir.into(),
            explicit_project_root: None,
            file_set: InstructionFileSet::All,
        }
    }

    pub fn project_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.explicit_project_root = Some(root.into());
        self
    }

    /// Resolve only the cross-tool `AGENTS.override.md` / `AGENTS.md` chain.
    ///
    /// This excludes echo-agent-native `.echo-agent/*` files and `CLAUDE.md`.
    /// It is useful for consumers that own a separate product-specific
    /// instruction namespace but still want the standard root-to-leaf AGENTS
    /// semantics and repository-boundary protections.
    pub fn agents_files_only(mut self) -> Self {
        self.file_set = InstructionFileSet::AgentsOnly;
        self
    }

    pub fn resolve(&self) -> ResolvedInstructions {
        let working_dir = canonical_or_original(&self.working_dir);
        let project_root = self
            .explicit_project_root
            .as_ref()
            .map(|root| canonical_or_original(root))
            .filter(|root| working_dir.starts_with(root))
            .or_else(|| find_git_root(&working_dir));
        let scan_root = project_root.clone().unwrap_or_else(|| working_dir.clone());
        let directories = path_from_root_to_working_dir(&scan_root, &working_dir);

        let mut sources = Vec::new();
        let mut blocks = Vec::new();
        for directory in directories {
            let Some((path, kind, content)) =
                load_one_directory(&directory, &scan_root, self.file_set)
            else {
                continue;
            };
            let precedence = sources.len();
            blocks.push(format!(
                "<!-- PROJECT INSTRUCTIONS: {} -->\n{}\n<!-- END PROJECT INSTRUCTIONS -->",
                path.display(),
                content.trim()
            ));
            sources.push(InstructionSource {
                path,
                kind: kind.to_string(),
                precedence,
            });
        }

        ResolvedInstructions {
            content: blocks.join("\n\n"),
            sources,
            project_root,
        }
    }
}

fn canonical_or_original(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn find_git_root(working_dir: &Path) -> Option<PathBuf> {
    working_dir
        .ancestors()
        .find(|directory| directory.join(".git").exists())
        .map(Path::to_path_buf)
}

fn path_from_root_to_working_dir(root: &Path, working_dir: &Path) -> Vec<PathBuf> {
    if !working_dir.starts_with(root) {
        return vec![working_dir.to_path_buf()];
    }
    let mut directories: Vec<PathBuf> = working_dir
        .ancestors()
        .take_while(|directory| directory.starts_with(root))
        .map(Path::to_path_buf)
        .collect();
    directories.reverse();
    directories
}

fn load_one_directory(
    directory: &Path,
    project_root: &Path,
    file_set: InstructionFileSet,
) -> Option<(PathBuf, &'static str, String)> {
    let candidates: Box<dyn Iterator<Item = &(&str, &str)> + '_> = match file_set {
        InstructionFileSet::All => Box::new(NATIVE_FILES.iter().chain(COMPATIBLE_FILES.iter())),
        InstructionFileSet::AgentsOnly => Box::new(AGENTS_FILES.iter()),
    };
    for (relative, kind) in candidates {
        let candidate = directory.join(relative);
        let canonical = match candidate.canonicalize() {
            Ok(path) => path,
            Err(_) => continue,
        };
        if !canonical.starts_with(project_root) || !canonical.is_file() {
            continue;
        }
        match std::fs::read_to_string(&canonical) {
            Ok(content) if !content.trim().is_empty() => return Some((canonical, kind, content)),
            _ => continue,
        }
    }
    None
}

/// Backward-compatible convenience API returning the highest-precedence source
/// and the full resolved chain.
pub fn load_project_rules(working_dir: &Path) -> Option<(PathBuf, String)> {
    let resolved = InstructionResolver::new(working_dir).resolve();
    let path = resolved.sources.last()?.path.clone();
    Some((path, resolved.content))
}

pub fn rules_injection(working_dir: &Path) -> Option<String> {
    InstructionResolver::new(working_dir).resolve().annotated()
}

pub fn rules_injection_with_root(
    working_dir: &Path,
    project_root: Option<&Path>,
) -> Option<String> {
    let resolver = InstructionResolver::new(working_dir);
    let resolver = match project_root {
        Some(root) => resolver.project_root(root),
        None => resolver,
    };
    resolver.resolve().annotated()
}

pub fn inject_rules(existing_prompt: &str, working_dir: &Path) -> String {
    match rules_injection(working_dir) {
        Some(rules) => format!("{}\n\n{}", rules, existing_prompt),
        None => existing_prompt.to_string(),
    }
}

pub fn inject_rules_with_root(
    existing_prompt: &str,
    working_dir: &Path,
    project_root: Option<&Path>,
) -> String {
    match rules_injection_with_root(working_dir, project_root) {
        Some(rules) => format!("{}\n\n{}", rules, existing_prompt),
        None => existing_prompt.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> std::io::Result<Self> {
            let path = std::env::temp_dir()
                .join(format!("echo-agent-instructions-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&path)?;
            Ok(Self(path))
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write(path: &Path, content: &str) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, content)
    }

    #[test]
    fn resolves_root_to_leaf_and_one_file_per_directory() -> Result<(), Box<dyn std::error::Error>>
    {
        let temp = TestDir::new()?;
        let root = temp.path().join("repo");
        let child = root.join("src/module");
        fs::create_dir_all(root.join(".git"))?;
        write(&root.join("AGENTS.md"), "root agents")?;
        write(&root.join(".echo-agent/AGENT.md"), "root native")?;
        write(&root.join("src/AGENTS.md"), "src agents")?;
        write(&root.join("src/CLAUDE.md"), "ignored same level")?;
        write(&child.join("AGENTS.override.md"), "leaf override")?;

        let resolved = InstructionResolver::new(&child).resolve();
        let canonical_root = canonical_or_original(&root);
        assert_eq!(
            resolved.project_root.as_deref(),
            Some(canonical_root.as_path())
        );
        assert_eq!(resolved.sources.len(), 3);
        assert_eq!(
            resolved.sources.first().map(|source| source.kind.as_str()),
            Some("echo_agent")
        );
        assert_eq!(
            resolved.sources.get(1).map(|source| source.kind.as_str()),
            Some("agents")
        );
        assert_eq!(
            resolved.sources.get(2).map(|source| source.kind.as_str()),
            Some("agents_override")
        );
        assert!(resolved.content.contains("root native"));
        assert!(!resolved.content.contains("root agents"));
        assert!(!resolved.content.contains("ignored same level"));
        assert!(resolved.content.find("root native") < resolved.content.find("leaf override"));
        Ok(())
    }

    #[test]
    fn explicit_root_blocks_parent_instructions() -> Result<(), Box<dyn std::error::Error>> {
        let temp = TestDir::new()?;
        let parent = temp.path().join("parent");
        let root = parent.join("repo");
        let child = root.join("src");
        fs::create_dir_all(&child)?;
        write(&parent.join("AGENTS.md"), "outside")?;
        write(&root.join("AGENTS.md"), "inside")?;

        let resolved = InstructionResolver::new(&child)
            .project_root(&root)
            .resolve();
        assert!(resolved.content.contains("inside"));
        assert!(!resolved.content.contains("outside"));
        Ok(())
    }

    #[test]
    fn no_git_root_checks_only_working_directory() -> Result<(), Box<dyn std::error::Error>> {
        let temp = TestDir::new()?;
        let parent = temp.path().join("parent");
        let child = parent.join("child");
        fs::create_dir_all(&child)?;
        write(&parent.join("AGENTS.md"), "outside")?;
        write(&child.join("CLAUDE.md"), "local")?;

        let resolved = InstructionResolver::new(&child).resolve();
        assert_eq!(resolved.project_root, None);
        assert_eq!(resolved.sources.len(), 1);
        assert!(resolved.content.contains("local"));
        assert!(!resolved.content.contains("outside"));
        Ok(())
    }

    #[test]
    fn empty_and_invalid_utf8_files_are_skipped() -> Result<(), Box<dyn std::error::Error>> {
        let temp = TestDir::new()?;
        let root = temp.path().join("repo");
        fs::create_dir_all(root.join(".git"))?;
        write(&root.join(".echo-agent/AGENT.md"), "  \n")?;
        fs::write(root.join("AGENTS.override.md"), [0xff, 0xfe])?;
        write(&root.join("AGENTS.md"), "valid fallback")?;

        let resolved = InstructionResolver::new(&root).resolve();
        assert_eq!(resolved.sources.len(), 1);
        assert!(resolved.content.contains("valid fallback"));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn symlink_outside_project_root_is_skipped() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::symlink;

        let temp = TestDir::new()?;
        let root = temp.path().join("repo");
        let outside = temp.path().join("outside.md");
        fs::create_dir_all(root.join(".git"))?;
        write(&outside, "outside")?;
        symlink(&outside, root.join("AGENTS.override.md"))?;
        write(&root.join("AGENTS.md"), "inside")?;

        let resolved = InstructionResolver::new(&root).resolve();
        assert_eq!(resolved.sources.len(), 1);
        assert!(resolved.content.contains("inside"));
        assert!(!resolved.content.contains("outside"));
        Ok(())
    }

    #[test]
    fn git_file_marks_worktree_root() -> Result<(), Box<dyn std::error::Error>> {
        let temp = TestDir::new()?;
        let root = temp.path().join("worktree");
        let child = root.join("src");
        fs::create_dir_all(&child)?;
        write(&root.join(".git"), "gitdir: ../git/worktrees/test")?;
        write(&root.join("AGENTS.md"), "worktree rules")?;

        let resolved = InstructionResolver::new(&child).resolve();
        let canonical_root = canonical_or_original(&root);
        assert_eq!(
            resolved.project_root.as_deref(),
            Some(canonical_root.as_path())
        );
        assert!(resolved.content.contains("worktree rules"));
        Ok(())
    }

    #[test]
    fn agents_only_excludes_native_and_claude_files() -> Result<(), Box<dyn std::error::Error>> {
        let temp = TestDir::new()?;
        let root = temp.path().join("repo");
        let child = root.join("src");
        fs::create_dir_all(root.join(".git"))?;
        fs::create_dir_all(&child)?;
        write(&root.join(".echo-agent/AGENT.md"), "native")?;
        write(&root.join("CLAUDE.md"), "claude")?;
        write(&root.join("AGENTS.md"), "root agents")?;
        write(&child.join("AGENTS.override.md"), "leaf override")?;

        let resolved = InstructionResolver::new(&child)
            .project_root(&root)
            .agents_files_only()
            .resolve();

        assert_eq!(resolved.sources.len(), 2);
        assert!(resolved.content.contains("root agents"));
        assert!(resolved.content.contains("leaf override"));
        assert!(!resolved.content.contains("native"));
        assert!(!resolved.content.contains("claude"));
        Ok(())
    }
}
