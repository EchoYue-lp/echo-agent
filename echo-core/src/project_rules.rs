//! Project-scoped instruction discovery with explicit repository boundaries.

use crate::error::{ReactError, Result};
use crate::tools::filesystem::FileSystem;
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
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
        // The existing local convenience API keeps its original best-effort behavior.
        self.resolve_from(None).unwrap_or_default()
    }

    /// Resolve the same precedence chain in one provider namespace. Call from
    /// the caller's blocking preparation boundary; transport failures are errors,
    /// not missing instructions and never local same-name fallbacks.
    pub fn resolve_with_file_system(
        &self,
        file_system: &dyn FileSystem,
    ) -> Result<ResolvedInstructions> {
        self.resolve_from(Some(file_system))
    }

    fn resolve_from(&self, file_system: Option<&dyn FileSystem>) -> Result<ResolvedInstructions> {
        let working_dir = canonical_or_original(&self.working_dir, file_system)?;
        let explicit_root = self
            .explicit_project_root
            .as_ref()
            .map(|root| canonical_or_original(root, file_system))
            .transpose()?;
        let project_root = match explicit_root.filter(|root| working_dir.starts_with(root)) {
            Some(root) => Some(root),
            None => find_git_root(&working_dir, file_system)?,
        };
        self.resolve_at_root(working_dir, project_root, file_system)
    }

    fn resolve_at_root(
        &self,
        working_dir: PathBuf,
        project_root: Option<PathBuf>,
        file_system: Option<&dyn FileSystem>,
    ) -> Result<ResolvedInstructions> {
        let scan_root = project_root.clone().unwrap_or_else(|| working_dir.clone());
        let directories = path_from_root_to_working_dir(&scan_root, &working_dir);

        let mut sources = Vec::new();
        let mut blocks = Vec::new();
        for directory in directories {
            let Some((path, kind, content)) =
                load_one_directory(&directory, &scan_root, self.file_set, file_system)?
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

        Ok(ResolvedInstructions {
            content: blocks.join("\n\n"),
            sources,
            project_root,
        })
    }
}

fn canonical_or_original(path: &Path, file_system: Option<&dyn FileSystem>) -> Result<PathBuf> {
    let Some(file_system) = file_system else {
        return Ok(path.canonicalize().unwrap_or_else(|_| path.to_path_buf()));
    };
    match file_system.canonicalize(path) {
        Ok(path) => Ok(path),
        Err(ReactError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(path.to_path_buf())
        }
        Err(error) => Err(error),
    }
}

fn find_git_root(
    working_dir: &Path,
    file_system: Option<&dyn FileSystem>,
) -> Result<Option<PathBuf>> {
    for directory in working_dir.ancestors() {
        let marker = directory.join(".git");
        let exists = match file_system {
            Some(file_system) => file_system.exists(&marker)?,
            None => marker.exists(),
        };
        if exists {
            return Ok(Some(directory.to_path_buf()));
        }
    }
    Ok(None)
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
    file_system: Option<&dyn FileSystem>,
) -> Result<Option<(PathBuf, &'static str, String)>> {
    let candidates: Box<dyn Iterator<Item = &(&str, &str)> + '_> = match file_set {
        InstructionFileSet::All => Box::new(NATIVE_FILES.iter().chain(COMPATIBLE_FILES.iter())),
        InstructionFileSet::AgentsOnly => Box::new(AGENTS_FILES.iter()),
    };
    for (relative, kind) in candidates {
        let candidate = directory.join(relative);
        let canonical = match file_system {
            Some(file_system) => match file_system.canonicalize(&candidate) {
                Ok(path) => path,
                Err(ReactError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                    continue;
                }
                Err(error) => return Err(error),
            },
            None => match candidate.canonicalize() {
                Ok(path) => path,
                Err(_) => continue,
            },
        };
        let is_file = match file_system {
            Some(file_system) => file_system.is_file(&canonical)?,
            None => canonical.is_file(),
        };
        if !canonical.starts_with(project_root) || !is_file {
            continue;
        }
        let content = match file_system {
            Some(file_system) => match file_system.read_to_string(&canonical) {
                Ok(content) => content,
                Err(ReactError::Io(error))
                    if error.kind() == std::io::ErrorKind::NotFound
                        || error.kind() == std::io::ErrorKind::InvalidData =>
                {
                    continue;
                }
                Err(error) => return Err(error),
            },
            None => match std::fs::read_to_string(&canonical) {
                Ok(content) => content,
                Err(_) => continue,
            },
        };
        if !content.trim().is_empty() {
            return Ok(Some((canonical, kind, content)));
        }
    }
    Ok(None)
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
    use std::result::Result;

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
        let canonical_root = canonical_or_original(&root, None)?;
        assert_eq!(
            resolved.project_root.as_deref(),
            Some(canonical_root.as_path())
        );
        assert_eq!(resolved.sources.len(), 3);
        assert_eq!(
            resolved
                .sources
                .iter()
                .map(|source| (source.path.clone(), source.precedence))
                .collect::<Vec<_>>(),
            vec![
                (canonical_root.join(".echo-agent/AGENT.md"), 0),
                (canonical_root.join("src/AGENTS.md"), 1),
                (canonical_root.join("src/module/AGENTS.override.md"), 2),
            ],
            "真实discovery必须停在最近fixture边界，且root到leaf每层只取一个文件",
        );
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

        // TMPDIR可位于真实仓库中；直接验证Git探测返回None后的同一解析路径，
        // 不依赖测试宿主祖先目录恰好没有.git，也不改进程全局cwd/env。
        let resolved = InstructionResolver::new(&child).resolve_at_root(
            canonical_or_original(&child, None)?,
            None,
            None,
        )?;
        // 此单测只覆盖discovery无根后的解析分支；实际祖先边界由相邻public resolver用例验证。
        assert_eq!(resolved.project_root, None);
        let canonical_child = canonical_or_original(&child, None)?;
        assert_eq!(
            resolved.sources,
            vec![InstructionSource {
                path: canonical_child.join("CLAUDE.md"),
                kind: "claude".to_string(),
                precedence: 0,
            }],
            "无根时只读cwd，不准把parent或宿主仓库指令当作本项目来源",
        );
        assert!(resolved.content.contains("local"));
        assert!(!resolved.content.contains("outside"));
        Ok(())
    }

    #[test]
    fn nearest_git_marker_excludes_outer_instruction_chain()
    -> Result<(), Box<dyn std::error::Error>> {
        // 本resolver既有合同识别.git目录/文件边界，不校验Git对象数据库；
        // 两种标记都必须在最近边界停下，不能穿透到TMPDIR祖先中的外层仓库。
        for gitfile in [false, true] {
            let temp = TestDir::new()?;
            let outer = temp.path().join("outer");
            let inner = outer.join("nested/inner");
            let child = inner.join("src/module");
            fs::create_dir_all(outer.join(".git"))?;
            fs::create_dir_all(&child)?;
            if gitfile {
                write(&inner.join(".git"), "gitdir: ../../metadata/inner")?;
            } else {
                fs::create_dir_all(inner.join(".git"))?;
            }
            write(
                &outer.join("AGENTS.md"),
                "outer instructions must be excluded",
            )?;
            write(
                &outer.join("nested/CLAUDE.md"),
                "intermediate instructions must be excluded",
            )?;
            write(&inner.join("AGENTS.md"), "inner instructions")?;
            write(&child.join("AGENTS.override.md"), "leaf instructions")?;

            let resolved = InstructionResolver::new(&child).resolve();
            let canonical_inner = canonical_or_original(&inner, None)?;
            assert_eq!(
                resolved.project_root.as_deref(),
                Some(canonical_inner.as_path())
            );
            assert_eq!(
                resolved
                    .sources
                    .iter()
                    .map(|source| (source.path.clone(), source.precedence))
                    .collect::<Vec<_>>(),
                vec![
                    (canonical_inner.join("AGENTS.md"), 0),
                    (canonical_inner.join("src/module/AGENTS.override.md"), 1)
                ],
            );
            let inner_position = resolved
                .content
                .find("inner instructions")
                .ok_or_else(|| std::io::Error::other("inner instruction block missing"))?;
            let leaf_position = resolved
                .content
                .find("leaf instructions")
                .ok_or_else(|| std::io::Error::other("leaf instruction block missing"))?;
            assert!(inner_position < leaf_position);
            assert!(!resolved.content.contains("outer instructions"));
            assert!(!resolved.content.contains("intermediate instructions"));
        }
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
        let canonical_root = canonical_or_original(&root, None)?;
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
        write(
            &root.join(".echo-agent/AGENT.md"),
            "EXCLUDED_NATIVE_PAYLOAD",
        )?;
        write(&root.join("CLAUDE.md"), "EXCLUDED_CLAUDE_PAYLOAD")?;
        write(&root.join("AGENTS.md"), "AGENTS_ONLY_ROOT_PAYLOAD")?;
        write(
            &child.join("AGENTS.override.md"),
            "AGENTS_ONLY_LEAF_PAYLOAD",
        )?;

        let resolved = InstructionResolver::new(&child)
            .project_root(&root)
            .agents_files_only()
            .resolve();

        assert_eq!(resolved.sources.len(), 2);
        let canonical_root = canonical_or_original(&root, None)?;
        assert_eq!(
            resolved.project_root.as_deref(),
            Some(canonical_root.as_path())
        );
        assert_eq!(
            resolved.sources.as_slice(),
            &[
                InstructionSource {
                    path: canonical_root.join("AGENTS.md"),
                    kind: "agents".to_string(),
                    precedence: 0,
                },
                InstructionSource {
                    path: canonical_root.join("src/AGENTS.override.md"),
                    kind: "agents_override".to_string(),
                    precedence: 1,
                },
            ]
        );
        // 注释保留真实源路径；排除判断针对夹具正文，避免路径中的同名单词误命中。
        assert!(resolved.content.contains("AGENTS_ONLY_ROOT_PAYLOAD"));
        assert!(resolved.content.contains("AGENTS_ONLY_LEAF_PAYLOAD"));
        assert!(!resolved.content.contains("EXCLUDED_NATIVE_PAYLOAD"));
        assert!(!resolved.content.contains("EXCLUDED_CLAUDE_PAYLOAD"));
        Ok(())
    }
}
