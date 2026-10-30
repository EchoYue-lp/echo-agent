//! Glob tool — find files by name pattern
//!
//! Walks a directory tree and returns file paths matching a glob pattern.

use echo_core::error::{Result, ToolError};
use echo_core::tools::pagination::PageRequest;
use echo_core::tools::{Tool, ToolParameters, ToolResult};
use futures::future::BoxFuture;
use serde_json::{Value, json};
use std::path::PathBuf;

// ── GlobTool ────────────────────────────────────────────────────────────────

/// Find files by name pattern.
///
/// Walks the directory tree from `path` (default: `.`) and returns
/// file paths matching the `pattern` glob (e.g. `**/*.rs`, `src/**/*.ts`).
#[derive(Clone)]
pub struct GlobTool {
    base_dir: Option<PathBuf>,
    file_system: std::sync::Arc<dyn echo_core::tools::filesystem::FileSystem>,
}

impl GlobTool {
    pub fn with_file_system(
        mut self,
        file_system: std::sync::Arc<dyn echo_core::tools::filesystem::FileSystem>,
    ) -> Self {
        self.file_system = file_system;
        self
    }

    pub fn new() -> Self {
        Self {
            base_dir: None,
            file_system: std::sync::Arc::new(super::LocalFileSystem),
        }
    }

    pub fn with_base_dir(base: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: Some(base.into()),
            file_system: std::sync::Arc::new(super::LocalFileSystem),
        }
    }
}

impl Default for GlobTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for GlobTool {
    fn name(&self) -> &str {
        "glob"
    }

    fn description(&self) -> &str {
        "Find file paths by glob; ** recurses."
    }

    fn permissions(&self) -> Vec<echo_core::tools::permission::ToolPermission> {
        vec![echo_core::tools::permission::ToolPermission::Read]
    }
    fn risk_level(&self) -> echo_core::tools::ToolRiskLevel {
        echo_core::tools::ToolRiskLevel::ReadOnly
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Filename glob."
                },
                "path": {
                    "type": "string",
                    "description": "Search directory; default cwd."
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 200,
                    "description": "Results/page; default 100."
                },
                "cursor": {
                    "type": "string",
                    "description": "Prior cursor."
                }
            },
            "required": ["pattern"]
        })
    }

    fn execute_with_context<'a>(
        &'a self,
        parameters: ToolParameters,
        ctx: &'a echo_core::tools::ToolContext,
    ) -> BoxFuture<'a, Result<ToolResult>> {
        let this = self.clone();
        let ctx = ctx.clone();
        super::run_file_operation("glob", &ctx.clone(), move || {
            let file_system = this.file_system.as_ref();
            let cancel = ctx.cancel.as_deref();
            let pattern_str = parameters
                .get("pattern")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("pattern".to_string()))?;

            let path_str = parameters
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or(".");

            let page_request = match PageRequest::from_parameters(&parameters, 100, 200) {
                Ok(request) => request,
                Err(error) => return Ok(ToolResult::invalid_arguments(error.to_string())),
            };

            // Effective base: construction-time base_dir (with confinement)
            // takes priority; otherwise fall back to runtime working_dir
            // (CWD override, no confinement check).
            let effective_base: Option<PathBuf> = this
                .base_dir
                .clone()
                .or_else(|| ctx.working_dir.as_ref().map(|p| p.to_path_buf()));

            let search_path = if let Some(ref base) = effective_base {
                if std::path::Path::new(path_str).is_absolute() {
                    PathBuf::from(path_str)
                } else {
                    base.join(path_str)
                }
            } else {
                PathBuf::from(path_str)
            };

            // 仅显式构造根启用范围校验；无根时保留 provider 的相对根形状，
            // 不额外要求 current_dir/canonicalize，matcher 与输出继续使用原路径。
            if this.base_dir.is_some() {
                super::resolve_path(
                    file_system,
                    "glob",
                    path_str,
                    &this.base_dir,
                    ctx.working_dir.as_deref(),
                )?;
            }

            if !file_system.exists(&search_path)? {
                return Ok(ToolResult::invalid_arguments(format!(
                    "Path does not exist: {}",
                    search_path.display()
                )));
            }

            if !file_system.is_dir(&search_path)? {
                return Ok(ToolResult::invalid_arguments(format!(
                    "Path is not a directory: {}",
                    search_path.display()
                )));
            }

            let mut results = Vec::new();
            walk_glob(
                file_system,
                cancel,
                &search_path,
                &search_path,
                &this.base_dir,
                pattern_str,
                &mut results,
            )?;
            let query = serde_json::json!({
                "pattern": pattern_str,
                "path": search_path,
            });
            let (page, page_info) = match page_request.paginate(results, &query) {
                Ok(page) => page,
                Err(error) => return Ok(ToolResult::invalid_arguments(error.to_string())),
            };
            let output = if page.is_empty() {
                format!("No files matching '{pattern_str}' found.")
            } else {
                format!(
                    "{}\n\n{} files returned ({} total)",
                    page.join("\n"),
                    page_info.returned,
                    page_info.total.unwrap_or(0)
                )
            };
            let mut result = ToolResult::success(output);
            page_info.apply_to(&mut result);
            Ok(result)
        })
    }
}

fn walk_glob(
    file_system: &dyn echo_core::tools::filesystem::FileSystem,
    cancel: Option<&echo_core::agent::CancellationToken>,
    dir: &std::path::Path,
    search_root: &std::path::Path,
    base_dir: &Option<PathBuf>,
    pattern: &str,
    results: &mut Vec<String>,
) -> Result<()> {
    super::check_file_cancel(cancel, "glob")?;
    let entries = match file_system.read_dir(dir) {
        Ok(e) => e,
        Err(echo_core::error::ReactError::Io(_)) => return Ok(()),
        Err(error) => return Err(error),
    };

    let mut entry_paths: Vec<PathBuf> = Vec::new();
    for entry in entries {
        super::check_file_cancel(cancel, "glob")?;
        entry_paths.push(entry.path());
    }
    entry_paths.sort();

    for path in entry_paths {
        super::check_file_cancel(cancel, "glob")?;
        // Skip hidden and common ignored directories
        if file_system.is_dir(&path)?
            && let Some(name) = path.file_name().and_then(|n| n.to_str())
            && (name.starts_with('.')
                || name == "target"
                || name == "node_modules"
                || name == "__pycache__"
                || name == ".git"
                || name == "vendor"
                || name == "build"
                || name == "dist")
        {
            continue;
        }

        if base_dir.is_some() {
            super::resolve_path_buf(file_system, "glob", &path, base_dir)?;
        }
        if file_system.is_dir(&path)? {
            walk_glob(
                file_system,
                cancel,
                &path,
                search_root,
                base_dir,
                pattern,
                results,
            )?;
        } else if file_system.is_file(&path)?
            && let Some(name) = path.file_name().and_then(|n| n.to_str())
            && glob_pattern_matches(pattern, name, &path, search_root)
        {
            results.push(path.display().to_string());
        }
    }
    Ok(())
}

/// Match a glob pattern against a file name.
///
/// Supports:
/// - `*` matches any sequence of characters (except /)
/// - `**` matches any path prefix (handled by walking)
/// - `?` matches any single character
/// - `{a,b}` matches a or b
/// - `[abc]` matches any character in the set
fn glob_pattern_matches(
    pattern: &str,
    name: &str,
    full_path: &std::path::Path,
    search_root: &std::path::Path,
) -> bool {
    let pattern_path = std::path::Path::new(pattern);
    let components: Vec<_> = pattern_path.components().collect();
    // 原无separator的filename模式继续匹配每层basename；./是明确路径锚，
    // Windows Prefix/RootDir/反斜杠同样由Path components解释，不另猜字符串。
    if components.len() == 1
        && let Some(std::path::Component::Normal(value)) = components.first()
        && let Some(pattern_name) = value.to_str()
    {
        return glob_match_advanced(pattern_name, name);
    }
    let target = if pattern_path.is_absolute() {
        full_path
    } else {
        full_path.strip_prefix(search_root).unwrap_or(full_path)
    };
    glob_with_doublestar(pattern, target)
}

fn glob_with_doublestar(pattern: &str, full_path: &std::path::Path) -> bool {
    // Match path components with the existing wildcard matcher; ** reaches
    // zero or more whole components. Do not treat *.rs as a literal suffix.
    // Pattern与path两边都用Path components：Windows UNC/verbatim的Prefix
    // 保持整体结构并先精确核对，不把一边prefix拆成server/share两segment。
    let pattern_path = std::path::Path::new(pattern);
    if pattern_path.has_root() != full_path.has_root()
        || pattern_path.is_absolute() != full_path.is_absolute()
    {
        return false;
    }
    #[cfg(windows)]
    let pattern_prefix = pattern_path.components().find_map(|part| match part {
        std::path::Component::Prefix(prefix) => Some(prefix.kind()),
        _ => None,
    });
    #[cfg(windows)]
    let file_prefix = full_path.components().find_map(|part| match part {
        std::path::Component::Prefix(prefix) => Some(prefix.kind()),
        _ => None,
    });
    #[cfg(windows)]
    if pattern_prefix != file_prefix {
        return false;
    }
    let parts = |path: &std::path::Path| -> Vec<String> {
        path.components()
            .filter_map(|part| match part {
                std::path::Component::Normal(value) => Some(value.to_string_lossy().into_owned()),
                std::path::Component::ParentDir => Some("..".to_string()),
                _ => None,
            })
            .collect()
    };
    let path_parts = parts(full_path);
    let pattern_parts = parts(pattern_path);
    let mut reachable = vec![false; path_parts.len().saturating_add(1)];
    if let Some(first) = reachable.first_mut() {
        *first = true;
    }
    for segment in pattern_parts.iter().map(String::as_str) {
        let mut next = vec![false; reachable.len()];
        if segment == "**" {
            let mut prior_reachable = false;
            for (index, state) in reachable.iter().enumerate() {
                prior_reachable |= *state;
                if let Some(slot) = next.get_mut(index) {
                    *slot = prior_reachable;
                }
            }
        } else {
            for (index, (part, state)) in path_parts.iter().zip(&reachable).enumerate() {
                if *state
                    && glob_match_advanced(segment, part)
                    && let Some(slot) = next.get_mut(index.saturating_add(1))
                {
                    *slot = true;
                }
            }
        }
        reachable = next;
    }
    reachable.last().copied().unwrap_or(false)
}

/// Advanced glob matching with * ? {a,b} [abc]
fn glob_match_advanced(pattern: &str, text: &str) -> bool {
    let mut pi = 0;
    let mut ti = 0;
    let p_chars: Vec<char> = pattern.chars().collect();
    let t_chars: Vec<char> = text.chars().collect();

    while let (Some(pattern_character), Some(text_character)) =
        (p_chars.get(pi).copied(), t_chars.get(ti).copied())
    {
        match pattern_character {
            '*' => {
                // Match any sequence
                pi = pi.saturating_add(1);
                if pi == p_chars.len() {
                    return true;
                }
                for skip in ti..=t_chars.len() {
                    // UTF-8-safe slicing: pi/skip are char indices (from
                    // p_chars/t_chars), NOT byte indices. Using &pattern[pi..]
                    // would panic on multibyte chars (e.g. paths like
                    // screenshot_北京.png). Reconstruct via char iterators.
                    let pat_rest: String = p_chars.get(pi..).unwrap_or_default().iter().collect();
                    let txt_rest: String = t_chars.get(skip..).unwrap_or_default().iter().collect();
                    if glob_match_advanced(&pat_rest, &txt_rest) {
                        return true;
                    }
                }
                return false;
            }
            '?' => {
                pi = pi.saturating_add(1);
                ti = ti.saturating_add(1);
            }
            '[' => {
                // Character class
                pi = pi.saturating_add(1);
                let mut found = false;
                let negated = p_chars.get(pi).is_some_and(|character| *character == '!');
                if negated {
                    pi = pi.saturating_add(1);
                }
                while p_chars.get(pi).is_some_and(|character| *character != ']') {
                    if p_chars.get(pi) == t_chars.get(ti) {
                        found = true;
                    }
                    pi = pi.saturating_add(1);
                }
                if pi < p_chars.len() {
                    pi = pi.saturating_add(1); // skip ]
                }
                if negated {
                    found = !found;
                }
                if !found {
                    return false;
                }
                ti = ti.saturating_add(1);
            }
            '{' => {
                // Brace expansion
                let close = p_chars
                    .get(pi..)
                    .and_then(|remainder| remainder.iter().position(|character| *character == '}'));
                if let Some(close_offset) = close {
                    let close_index = pi.saturating_add(close_offset);
                    let brace_content: String = p_chars
                        .get(pi.saturating_add(1)..close_index)
                        .unwrap_or_default()
                        .iter()
                        .collect();
                    let alternatives: Vec<&str> = brace_content.split(',').collect();
                    let after_brace: String = p_chars
                        .get(close_index.saturating_add(1)..)
                        .unwrap_or_default()
                        .iter()
                        .collect();
                    let before_brace: String =
                        p_chars.get(..pi).unwrap_or_default().iter().collect();
                    for alt in alternatives {
                        let full = format!("{}{}{}", before_brace, alt.trim(), after_brace);
                        if glob_match_advanced(&full, text) {
                            return true;
                        }
                    }
                    return false;
                }
                // No closing brace, treat as literal
                if pattern_character == text_character {
                    pi = pi.saturating_add(1);
                    ti = ti.saturating_add(1);
                } else {
                    return false;
                }
            }
            c => {
                if c == text_character {
                    pi = pi.saturating_add(1);
                    ti = ti.saturating_add(1);
                } else {
                    return false;
                }
            }
        }
    }

    // Consume trailing * in pattern
    while p_chars.get(pi).is_some_and(|character| *character == '*') {
        pi = pi.saturating_add(1);
    }

    pi == p_chars.len() && ti == t_chars.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_glob_match_star() {
        assert!(glob_match_advanced("*.rs", "main.rs"));
        assert!(glob_match_advanced("*.rs", "lib.rs"));
        assert!(!glob_match_advanced("*.rs", "main.py"));
    }

    #[test]
    fn test_glob_match_brace() {
        assert!(glob_match_advanced("*.{rs,ts}", "main.rs"));
        assert!(glob_match_advanced("*.{rs,ts}", "app.ts"));
        assert!(!glob_match_advanced("*.{rs,ts}", "main.py"));
    }

    #[test]
    fn test_glob_match_question() {
        assert!(glob_match_advanced("?.rs", "a.rs"));
        assert!(!glob_match_advanced("?.rs", "ab.rs"));
    }

    #[test]
    fn test_glob_match_bracket() {
        assert!(glob_match_advanced("[abc].rs", "a.rs"));
        assert!(!glob_match_advanced("[abc].rs", "d.rs"));
    }

    #[test]
    fn test_glob_match_exact() {
        assert!(glob_match_advanced("main.rs", "main.rs"));
        assert!(!glob_match_advanced("main.rs", "lib.rs"));
    }

    #[test]
    fn test_glob_match_multibyte_path() {
        // Regression: paths with multibyte UTF-8 chars (e.g. Chinese) must not
        // panic. Previously `&pattern[pi..]` / `&text[skip..]` used byte slicing
        // on char-index values, panicking on paths like screenshot_北京.png.
        assert!(glob_match_advanced("*.png", "screenshot_北京.png"));
        assert!(glob_match_advanced("screenshot_*", "screenshot_北京.png"));
        assert!(glob_match_advanced(
            "*/北京*",
            "/Users/ls/照片/北京/景山.png"
        ));
        assert!(!glob_match_advanced("*.rs", "screenshot_北京.png"));
        // Sanity: plain ASCII still works after the fix.
        assert!(glob_match_advanced("*.png", "screenshot_shanghai.png"));
    }
    // 候选：并入 echo-tools/src/files/glob.rs 的既有 tests module。
    // 未编译、未运行；要求实际走 GlobTool，不能只调用 glob_match_advanced。
    #[tokio::test]
    async fn recursive_glob_dispatch_matches_zero_and_many_components()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        use echo_core::tools::{Tool, ToolContext, ToolParameters};
        let root = tempfile::tempdir()?;
        for relative in [
            "root.rs",
            "src/main.rs",
            "src/deep/另一个.rs",
            "文档🙂/中文.rs",
            "not-rust.rs.txt",
            "other/src/decoy.rs",
        ] {
            let path = root.path().join(relative);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, b"fixture")?;
        }
        let tool = GlobTool::with_base_dir(root.path());
        for (pattern, expected) in [
            (
                "**/*.rs",
                vec![
                    "root.rs",
                    "src/main.rs",
                    "src/deep/另一个.rs",
                    "文档🙂/中文.rs",
                    "other/src/decoy.rs",
                ],
            ),
            ("src/**/*.rs", vec!["src/main.rs", "src/deep/另一个.rs"]),
            ("src/**/?.{rs,ts}", vec![]),
        ] {
            let result = tool
                .execute_with_context(
                    ToolParameters::from([("pattern".into(), serde_json::json!(pattern))]),
                    &ToolContext::default(),
                )
                .await?;
            assert!(result.success, "glob failed: {:?}", result.error);
            let mut actual = result
                .output
                .lines()
                .filter_map(|line| {
                    std::path::Path::new(line)
                        .strip_prefix(root.path())
                        .ok()
                        .map(|p| {
                            p.components()
                                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                                .collect::<Vec<_>>()
                                .join("/")
                        })
                })
                .collect::<Vec<_>>();
            let mut expected = expected.into_iter().map(str::to_string).collect::<Vec<_>>();
            actual.sort();
            expected.sort();
            assert_eq!(actual, expected, "dispatcher pattern={pattern}");
        }
        Ok(())
    }

    // 新增非回归控制；初次RED的单函数未包含本函数，不追标旧RED。
    #[tokio::test]
    async fn source8_glob_dispatch_preserves_absolute_and_shallow_anchors()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        use echo_core::tools::{Tool, ToolContext, ToolParameters};
        let root = tempfile::tempdir()?;
        for relative in ["src/main.rs", "src/deep/nested.rs", "src-other/decoy.rs"] {
            let path = root.path().join(relative);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, b"fixture")?;
        }
        let tool = GlobTool::with_base_dir(root.path());
        for (pattern, expected) in [
            ("src/*.rs".to_string(), vec!["src/main.rs"]),
            ("./src/*.rs".to_string(), vec!["src/main.rs"]),
            ("./src/main.rs".to_string(), vec!["src/main.rs"]),
            (
                root.path().join("src/*.rs").to_string_lossy().into_owned(),
                vec!["src/main.rs"],
            ),
            (
                root.path()
                    .join("src/main.rs")
                    .to_string_lossy()
                    .into_owned(),
                vec!["src/main.rs"],
            ),
            (
                root.path()
                    .join("src/**/*.rs")
                    .to_string_lossy()
                    .into_owned(),
                vec!["src/main.rs", "src/deep/nested.rs"],
            ),
        ] {
            let result = tool
                .execute_with_context(
                    ToolParameters::from([("pattern".into(), serde_json::json!(pattern))]),
                    &ToolContext::default(),
                )
                .await?;
            assert!(result.success);
            let mut actual = result
                .output
                .lines()
                .filter_map(|line| {
                    std::path::Path::new(line)
                        .strip_prefix(root.path())
                        .ok()
                        .map(|relative| {
                            relative
                                .components()
                                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                                .collect::<Vec<_>>()
                                .join("/")
                        })
                })
                .collect::<Vec<_>>();
            let mut expected = expected.into_iter().map(str::to_string).collect::<Vec<_>>();
            actual.sort();
            expected.sort();
            assert_eq!(actual, expected, "pattern={pattern}");
        }
        Ok(())
    }

    // portable原matcher控制：不解析host cwd；相同component但root不同不能互认。
    #[test]
    fn source8_glob_rooted_pattern_cannot_match_relative_provider_entry() {
        assert!(!glob_pattern_matches(
            "/src/main.rs",
            "main.rs",
            std::path::Path::new("./src/main.rs"),
            std::path::Path::new(".")
        ));
        assert!(!glob_pattern_matches(
            "/src/*.rs",
            "main.rs",
            std::path::Path::new("./src/main.rs"),
            std::path::Path::new(".")
        ));
    }

    // 平台token控制，不访问UNC服务器；只有Windows实际运行才授该平台test票。
    #[cfg(windows)]
    #[test]
    fn source8_glob_windows_prefix_tokens_are_compared_consistently() {
        for (root, file) in [
            (r"C:\repo", r"C:\repo\main.rs"),
            (r"\\server\share\repo", r"\\server\share\repo\main.rs"),
            (
                r"\\?\UNC\server\share\repo",
                r"\\?\UNC\server\share\repo\main.rs",
            ),
        ] {
            let root = std::path::Path::new(root);
            let file = std::path::Path::new(file);
            let pattern = file.to_string_lossy().into_owned();
            assert!(glob_pattern_matches(&pattern, "main.rs", file, root));
            let recursive = format!(r"{}\**\*.rs", root.to_string_lossy());
            assert!(glob_pattern_matches(&recursive, "main.rs", file, root));
        }
        let forward_root = std::path::Path::new("//server/share/repo");
        let forward_file = std::path::Path::new("//server/share/repo/main.rs");
        assert!(glob_pattern_matches(
            "//server/share/repo/main.rs",
            "main.rs",
            forward_file,
            forward_root
        ));
        assert!(glob_pattern_matches(
            "//server/share/repo/**/*.rs",
            "main.rs",
            forward_file,
            forward_root
        ));
        let file = std::path::Path::new(r"\\server\share\repo\main.rs");
        assert!(!glob_pattern_matches(
            "//different/share/repo/main.rs",
            "main.rs",
            file,
            std::path::Path::new(r"\\server\share\repo")
        ));
        assert!(!glob_pattern_matches(
            "C:/repo/main.rs",
            "main.rs",
            file,
            std::path::Path::new(r"\\server\share\repo")
        ));
    }

    // 真实GlobTool、默认path='.'与provider相对目录项；不改全局cwd、不访问host/root目录。
    #[tokio::test]
    async fn source8_glob_default_path_provider_entries_preserve_root_shape()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        use echo_core::tools::filesystem::{FileEntry, FileMetadata, FileSystem, FileType};
        use echo_core::tools::{Tool, ToolContext, ToolParameters};
        use std::path::{Path, PathBuf};
        struct RelativeFiles;
        fn unsupported<T>() -> echo_core::error::Result<T> {
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "fixture operation not used",
            )
            .into())
        }
        impl FileSystem for RelativeFiles {
            fn current_dir(&self) -> echo_core::error::Result<PathBuf> {
                unsupported()
            }
            fn canonicalize(&self, _: &Path) -> echo_core::error::Result<PathBuf> {
                unsupported()
            }
            fn metadata(&self, p: &Path) -> echo_core::error::Result<FileMetadata> {
                let kind = if p == Path::new(".") || p == Path::new("./src") {
                    FileType::Directory
                } else if p == Path::new("./src/main.rs") {
                    FileType::File
                } else {
                    return Err(std::io::Error::from(std::io::ErrorKind::NotFound).into());
                };
                Ok(FileMetadata {
                    file_type: kind,
                    len: 0,
                    mode: None,
                })
            }
            fn symlink_metadata(&self, p: &Path) -> echo_core::error::Result<FileMetadata> {
                self.metadata(p)
            }
            fn read(&self, _: &Path) -> echo_core::error::Result<Vec<u8>> {
                unsupported()
            }
            fn read_dir(&self, p: &Path) -> echo_core::error::Result<Vec<FileEntry>> {
                if p == Path::new(".") {
                    Ok(vec![FileEntry {
                        path: "./src".into(),
                        file_name: "src".into(),
                        file_type: FileType::Directory,
                    }])
                } else if p == Path::new("./src") {
                    Ok(vec![FileEntry {
                        path: "./src/main.rs".into(),
                        file_name: "main.rs".into(),
                        file_type: FileType::File,
                    }])
                } else {
                    unsupported()
                }
            }
            fn create_dir_all(&self, _: &Path) -> echo_core::error::Result<()> {
                unsupported()
            }
            fn create_file(&self, _: &Path) -> echo_core::error::Result<()> {
                unsupported()
            }
            fn write(&self, _: &Path, _: &[u8]) -> echo_core::error::Result<()> {
                unsupported()
            }
            fn append(&self, _: &Path, _: &[u8]) -> echo_core::error::Result<()> {
                unsupported()
            }
            fn rename(&self, _: &Path, _: &Path) -> echo_core::error::Result<()> {
                unsupported()
            }
            fn remove_file(&self, _: &Path) -> echo_core::error::Result<()> {
                unsupported()
            }
            fn remove_dir(&self, _: &Path) -> echo_core::error::Result<()> {
                unsupported()
            }
            fn remove_dir_all(&self, _: &Path) -> echo_core::error::Result<()> {
                unsupported()
            }
            fn set_mode(&self, _: &Path, _: u32) -> echo_core::error::Result<()> {
                unsupported()
            }
            fn atomic_write(&self, _: &Path, _: &[u8]) -> echo_core::error::Result<()> {
                unsupported()
            }
            fn checkpoint(&self, _: &Path) -> echo_core::error::Result<Option<String>> {
                unsupported()
            }
        }
        let tool = GlobTool::new().with_file_system(std::sync::Arc::new(RelativeFiles));
        for (pattern, should_match) in [
            ("/src/main.rs", false),
            ("/src/*.rs", false),
            ("./src/*.rs", true),
            ("src/**/*.rs", true),
        ] {
            let result = tool
                .execute_with_context(
                    ToolParameters::from([("pattern".into(), serde_json::json!(pattern))]),
                    &ToolContext::default(),
                )
                .await?;
            assert!(result.success);
            assert_eq!(
                result.output.lines().any(|line| line == "./src/main.rs"),
                should_match,
                "pattern={pattern}"
            );
        }
        Ok(())
    }
}
