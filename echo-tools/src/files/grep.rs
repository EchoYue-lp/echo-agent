//! Grep tool — search file contents by regex pattern
//!
//! Uses the `regex` crate for pattern matching. Supports context lines,
//! file type filtering, and case-insensitive mode.

use echo_core::error::{Result, ToolError};
use echo_core::tools::pagination::PageRequest;
use echo_core::tools::{Tool, ToolParameters, ToolResult};
use futures::future::BoxFuture;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

// ── GrepTool ────────────────────────────────────────────────────────────────

/// 远端内容搜索单次请求允许回传的记录条数上界。
///
/// 本机遍历天然把结果留在内存里；远端记录要跨网络传输，必须有界。该上界
/// 不随分页 `limit` 变化——分页指纹覆盖结果集合本身，若上界随页大小变化，
/// 同一查询的后续页会因快照不同而失配。
const CONTENT_SEARCH_MAX_RESULTS: usize = 1_000;

/// Search file contents by regex pattern.
///
/// Walks the directory tree from `path` (default: `.`), reads each file,
/// and returns lines matching the `pattern` regex. Supports context lines,
/// file glob filtering, and case-insensitive mode.
#[derive(Clone)]
pub struct GrepTool {
    base_dir: Option<PathBuf>,
    file_system: std::sync::Arc<dyn echo_core::tools::filesystem::FileSystem>,
    search_provider: Option<std::sync::Arc<dyn echo_core::tools::search::ContentSearchProvider>>,
}

impl GrepTool {
    pub fn with_file_system(
        mut self,
        file_system: std::sync::Arc<dyn echo_core::tools::filesystem::FileSystem>,
    ) -> Self {
        self.file_system = file_system;
        self
    }

    /// 接入可选远端搜索提供者。`None`（默认）时保持本机遍历，行为不变。
    ///
    /// 提供者只在远端完成匹配并回传最终展示记录；本机遍历是它说“不支持”
    /// 时的回退路径，不是被替换的实现。
    pub fn with_search_provider(
        mut self,
        provider: Option<std::sync::Arc<dyn echo_core::tools::search::ContentSearchProvider>>,
    ) -> Self {
        self.search_provider = provider;
        self
    }

    pub fn new() -> Self {
        Self {
            base_dir: None,
            file_system: std::sync::Arc::new(super::LocalFileSystem),
            search_provider: None,
        }
    }

    pub fn with_base_dir(base: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: Some(base.into()),
            file_system: std::sync::Arc::new(super::LocalFileSystem),
            search_provider: None,
        }
    }
}

impl Default for GrepTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        "Regex content search: paths, line numbers, matching lines; use code_search for symbols."
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
                    "description": "Regex."
                },
                "path": {
                    "type": "string",
                    "description": "File/directory; default cwd."
                },
                "glob": {
                    "type": "string",
                    "description": "Filename glob."
                },
                "case_insensitive": {
                    "type": "boolean",
                    "description": "Ignore case; default false."
                },
                "context": {
                    "type": "integer",
                    "description": "Context lines on each side; default 0."
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 200,
                    "description": "Results/page; default 50."
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
        super::run_file_operation("grep", &ctx.clone(), move || {
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

            let glob_filter = parameters.get("glob").and_then(|v| v.as_str());

            let case_insensitive = parameters
                .get("case_insensitive")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let context_lines = match parameters.get("context") {
                Some(value) => match value.as_u64().and_then(|value| usize::try_from(value).ok()) {
                    Some(value) => value,
                    None => {
                        return Ok(ToolResult::invalid_arguments(
                            "context must be a non-negative integer supported by this platform"
                                .to_string(),
                        ));
                    }
                },
                None => 0,
            };

            let page_request = match PageRequest::from_parameters(&parameters, 50, 200) {
                Ok(request) => request,
                Err(error) => return Ok(ToolResult::invalid_arguments(error.to_string())),
            };

            // Compile regex
            let regex = regex::RegexBuilder::new(pattern_str)
                .case_insensitive(case_insensitive)
                .build()
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: "grep".to_string(),
                    message: format!("Invalid regex pattern '{}': {}", pattern_str, e),
                })?;

            // Allowed root set (all canonicalized so symlinks/`..` can't escape):
            //   1. construction-time base_dir (highest priority for resolution)
            //   2. runtime working_dir
            //   3. output_artifacts.root_dir — lets the model grep spilled
            //      tool output and user-input artifacts by absolute path, the
            //      same paths `read_artifact` already hands out. Mirrors
            //      `resolve_artifact_path` in `artifact.rs`.
            // Relative `path` is joined to the first available root (base_dir,
            // working_dir, then artifact root) to preserve the existing
            // single-root behavior;
            // absolute `path` is used as-is. Confinement passes if the
            // canonicalized path starts with *any* allowed root.
            let mut allowed_roots: Vec<PathBuf> = Vec::new();
            if let Some(base) = &this.base_dir {
                allowed_roots.push(base.clone());
            }
            if let Some(working) = &ctx.working_dir {
                allowed_roots.push(working.clone());
            }
            if file_system.paths_are_native()
                && let Some(artifacts) = ctx.output_artifacts.as_ref()
            {
                allowed_roots.push(artifacts.root_dir.clone());
            }

            let preferred_root = this
                .base_dir
                .clone()
                .or_else(|| ctx.working_dir.as_ref().map(|p| p.to_path_buf()))
                .or_else(|| {
                    if !file_system.paths_are_native() {
                        return None;
                    }
                    ctx.output_artifacts
                        .as_ref()
                        .map(|artifacts| artifacts.root_dir.clone())
                });

            let canonical_roots = if preferred_root.is_some() {
                Some(canonicalize_roots(file_system, cancel, &allowed_roots)?)
            } else {
                None
            };
            let search_path = if let Some(ref base) = preferred_root {
                let resolved = if Path::new(path_str).is_absolute() {
                    PathBuf::from(path_str)
                } else {
                    base.join(path_str)
                };
                // Canonicalize both the candidate and the roots so symlink /
                // `..` escapes are caught. Missing roots (e.g. an uncreated
                // artifact dir) are skipped rather than erroring.
                let resolved_canon = match file_system.canonicalize(&resolved) {
                    Ok(p) => p,
                    Err(echo_core::error::ReactError::Io(_)) => resolved.clone(),
                    Err(error) => return Err(error),
                };
                let within_scope = canonical_roots
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .any(|root| resolved_canon.starts_with(root));
                if !within_scope {
                    return Ok(ToolResult::invalid_arguments(format!(
                        "Path '{}' is outside the allowed directory scope",
                        path_str
                    )));
                }
                resolved
            } else {
                // No roots of any kind are configured — preserve the legacy
                // unrestricted behavior.
                PathBuf::from(path_str)
            };

            // 提供者优先于本机遍历：它在远端完成匹配、只回传排版好的记录，
            // 避免把候选正文逐文件拉回本机。`None` 表示该提供者不支持这类
            // 请求，随后落回原本机路径，结果与未接入提供者时完全一致。
            let mut provider_truncated = false;
            let mut results = Vec::new();
            let provider_outcome = match this.search_provider.as_ref() {
                Some(provider) => {
                    super::check_file_cancel(cancel, "grep")?;
                    let request = echo_core::tools::search::ContentSearchRequest {
                        root: &search_path,
                        pattern: pattern_str,
                        glob: glob_filter,
                        context_lines,
                        case_insensitive,
                        max_results: CONTENT_SEARCH_MAX_RESULTS,
                    };
                    // 远端搜索失败按本工具既有映射上抛；降级成本机遍历或
                    // “没有匹配”都会把一次故障伪装成一次成功的空结果。
                    provider.search(&request).map_err(|error| {
                        super::file_error(error, |error| ToolError::ExecutionFailed {
                            tool: "grep".to_string(),
                            message: format!(
                                "Content search failed for {}: {}",
                                search_path.display(),
                                error
                            ),
                        })
                    })?
                }
                None => None,
            };

            if let Some(outcome) = provider_outcome {
                provider_truncated = outcome.truncated;
                results = outcome.records;
            } else {
                // 一次工具调用只建立一个只读会话：支持会话的提供者因此把整次
                // 遍历复用到同一条传输上；本地提供者不支持会话，行为不变。
                let (searched, session_reused) =
                    echo_core::tools::filesystem::with_read_session(file_system, |files| {
                        if !files.exists(&search_path)? {
                            return Ok(false);
                        }
                        if files.is_file(&search_path)? {
                            search_file(
                                files,
                                cancel,
                                &search_path,
                                &regex,
                                glob_filter,
                                context_lines,
                                &mut results,
                            )?;
                        } else if files.is_dir(&search_path)? {
                            walk_and_search(
                                files,
                                cancel,
                                &search_path,
                                canonical_roots.as_deref(),
                                &regex,
                                (glob_filter, context_lines),
                                &mut results,
                            )?;
                        }
                        Ok(true)
                    });
                tracing::debug!(
                    tool = "grep",
                    reused = session_reused,
                    "content search scope settled"
                );
                if !searched? {
                    return Ok(ToolResult::invalid_arguments(format!(
                        "Path does not exist: {}",
                        search_path.display()
                    )));
                }
            }

            let query = serde_json::json!({
                "pattern": pattern_str,
                "path": search_path,
                "glob": glob_filter,
                "case_insensitive": case_insensitive,
                "context": context_lines,
            });
            let (page, page_info) = match page_request.paginate(results, &query) {
                Ok(page) => page,
                Err(error) => return Ok(ToolResult::invalid_arguments(error.to_string())),
            };
            let output = if page.is_empty() {
                "No matches found.".to_string()
            } else {
                format!(
                    "{}\n\n{} matches returned ({} total)",
                    page.join("\n"),
                    page_info.returned,
                    page_info.total.unwrap_or(0)
                )
            };
            let mut result = ToolResult::success(output);
            // 远端只回传了完整命中集的前缀时必须显式标记，否则模型会把它
            // 读成“已经搜到全部命中”。
            result = result.with_truncated(provider_truncated);
            page_info.apply_to(&mut result);
            Ok(result)
        })
    }
}

/// Resolve each candidate root to its canonical form, skipping any that do not
/// exist on disk (e.g. an artifact dir not yet created). Used for confinement
/// checks so symlinks and `..` cannot escape the allowed scope.
fn canonicalize_roots(
    file_system: &dyn echo_core::tools::filesystem::FileSystem,
    cancel: Option<&echo_core::agent::CancellationToken>,
    roots: &[PathBuf],
) -> Result<Vec<PathBuf>> {
    super::check_file_cancel(cancel, "grep")?;
    let mut canon: Vec<PathBuf> = Vec::with_capacity(roots.len());
    for root in roots {
        // Missing root (e.g. an uncreated artifact dir) is skipped, not errored.
        match file_system.canonicalize(root) {
            Ok(path) => canon.push(path),
            Err(echo_core::error::ReactError::Io(_)) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(canon)
}

fn search_file(
    file_system: &dyn echo_core::tools::filesystem::FileSystem,
    cancel: Option<&echo_core::agent::CancellationToken>,
    path: &Path,
    regex: &regex::Regex,
    glob_filter: Option<&str>,
    context_lines: usize,
    results: &mut Vec<String>,
) -> Result<()> {
    super::check_file_cancel(cancel, "grep")?;
    // Check glob filter
    if let Some(glob) = glob_filter
        && let Some(name) = path.file_name().and_then(|n| n.to_str())
        && !glob_matches(glob, name)
    {
        return Ok(());
    }

    // Skip binary files and large files
    let metadata = file_system.metadata(path).map_err(|e| {
        super::file_error(e, |e| ToolError::ExecutionFailed {
            tool: "grep".to_string(),
            message: format!("Cannot read metadata for {}: {}", path.display(), e),
        })
    })?;

    if metadata.len() > 10 * 1024 * 1024 {
        // Skip files > 10MB
        return Ok(());
    }

    let content = match file_system.read_to_string(path) {
        Ok(c) => c,
        Err(echo_core::error::ReactError::Io(_)) => return Ok(()),
        Err(error) => return Err(error), // Skip binary/unreadable files
    };

    let lines: Vec<&str> = content.lines().collect();
    let path_display = path.display().to_string();

    for (line_idx, line) in lines.iter().enumerate() {
        super::check_file_cancel(cancel, "grep")?;
        if regex.is_match(line) {
            let line_num = line_idx.saturating_add(1);
            if context_lines > 0 {
                let start = line_idx.saturating_sub(context_lines);
                let end = line_idx
                    .saturating_add(context_lines)
                    .saturating_add(1)
                    .min(lines.len());
                let mut record = Vec::new();
                for (ctx_idx, context_line) in lines
                    .iter()
                    .enumerate()
                    .skip(start)
                    .take(end.saturating_sub(start))
                {
                    let ctx_num = ctx_idx.saturating_add(1);
                    let prefix = if ctx_idx == line_idx { ">" } else { " " };
                    record.push(format!("{prefix}{path_display}:{ctx_num}:  {context_line}"));
                }
                results.push(record.join("\n"));
            } else {
                results.push(format!("{path_display}:{line_num}:  {line}"));
            }
        }
    }

    Ok(())
}

fn walk_and_search(
    file_system: &dyn echo_core::tools::filesystem::FileSystem,
    cancel: Option<&echo_core::agent::CancellationToken>,
    dir: &Path,
    allowed_roots: Option<&[PathBuf]>,
    regex: &regex::Regex,
    display_options: (Option<&str>, usize),
    results: &mut Vec<String>,
) -> Result<()> {
    super::check_file_cancel(cancel, "grep")?;
    let (glob_filter, context_lines) = display_options;
    let entries = match file_system.read_dir(dir) {
        Ok(e) => e,
        Err(echo_core::error::ReactError::Io(_)) => return Ok(()),
        Err(error) => return Err(error),
    };

    // Collect and sort entries for deterministic output
    let mut entry_paths: Vec<PathBuf> = Vec::new();
    for entry in entries {
        super::check_file_cancel(cancel, "grep")?;
        let path = entry.path();
        // Skip hidden directories and common ignore patterns
        if let Some(name) = path.file_name().and_then(|n| n.to_str())
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
        entry_paths.push(path);
    }
    entry_paths.sort();

    for path in entry_paths {
        super::check_file_cancel(cancel, "grep")?;
        if let Some(roots) = allowed_roots {
            // 原 Grep root 集合选择范围；target namespace 与目录根分别校验。
            let canonical = match file_system.canonicalize(&path) {
                Ok(path) => path,
                Err(error) if super::is_io_kind(&error, std::io::ErrorKind::NotFound) => continue,
                Err(error) => return Err(error),
            };
            let root = roots
                .iter()
                .find(|root| canonical.starts_with(root))
                .ok_or_else(|| ToolError::InvalidParameter {
                    name: "path".to_string(),
                    message: format!(
                        "Path '{}' is outside the allowed directory scope",
                        path.display()
                    ),
                })?;
            super::resolve_path_buf(file_system, "grep", &canonical, &Some(root.clone()))?;
        }
        if file_system.is_dir(&path)? {
            walk_and_search(
                file_system,
                cancel,
                &path,
                allowed_roots,
                regex,
                display_options,
                results,
            )?;
        } else if file_system.is_file(&path)? {
            search_file(
                file_system,
                cancel,
                &path,
                regex,
                glob_filter,
                context_lines,
                results,
            )?;
        }
    }

    Ok(())
}

/// Simple glob matching supporting * and ? wildcards
fn glob_matches(pattern: &str, name: &str) -> bool {
    // Handle brace patterns like *.{rs,ts}
    if let Some((prefix, remainder)) = pattern.split_once('{')
        && let Some((alternatives, suffix)) = remainder.split_once('}')
    {
        for alt in alternatives.split(',') {
            let full = format!("{}{}{}", prefix, alt.trim(), suffix);
            if glob_matches_simple(&full, name) {
                return true;
            }
        }
        return false;
    }

    glob_matches_simple(pattern, name)
}

fn glob_matches_simple(pattern: &str, name: &str) -> bool {
    let pat_chars: Vec<char> = pattern.chars().collect();
    let name_chars: Vec<char> = name.chars().collect();
    glob_match_inner(&pat_chars, &name_chars, 0, 0)
}

fn glob_match_inner(pat: &[char], name: &[char], pi: usize, ni: usize) -> bool {
    if pi == pat.len() {
        return ni == name.len();
    }

    if pat.get(pi).is_some_and(|character| *character == '*') {
        // Try matching 0 or more characters
        for skip in ni..=name.len() {
            if glob_match_inner(pat, name, pi.saturating_add(1), skip) {
                return true;
            }
        }
        return false;
    }

    if ni >= name.len() {
        return false;
    }

    let pattern_character = pat.get(pi);
    let name_character = name.get(ni);
    if pattern_character.is_some_and(|character| *character == '?')
        || pattern_character == name_character
    {
        return glob_match_inner(pat, name, pi.saturating_add(1), ni.saturating_add(1));
    }

    false
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_glob_matches_star() {
        assert!(glob_matches("*.rs", "main.rs"));
        assert!(glob_matches("*.rs", "lib.rs"));
        assert!(!glob_matches("*.rs", "main.py"));
    }

    #[test]
    fn test_glob_matches_brace() {
        assert!(glob_matches("*.{rs,ts}", "main.rs"));
        assert!(glob_matches("*.{rs,ts}", "app.ts"));
        assert!(!glob_matches("*.{rs,ts}", "main.py"));
    }

    #[test]
    fn test_glob_matches_question_mark() {
        assert!(glob_matches("?.rs", "a.rs"));
        assert!(!glob_matches("?.rs", "ab.rs"));
    }

    #[test]
    fn test_glob_matches_exact() {
        assert!(glob_matches("main.rs", "main.rs"));
        assert!(!glob_matches("main.rs", "lib.rs"));
    }

    // ── confinement / artifact-root extension (execute_with_context) ──
    // These verify the new candidate-root-set logic: grep can search files
    // under the artifact root (so the model can grep spilled tool output /
    // user-input artifacts by absolute path) while still rejecting paths
    // outside every allowed root.

    use echo_core::tools::artifact::ToolOutputArtifactConfig;

    /// Build grep parameters for the given path/pattern.
    fn grep_params(pattern: &str, path: &str) -> ToolParameters {
        ToolParameters::from([
            ("pattern".to_string(), Value::String(pattern.to_string())),
            ("path".to_string(), Value::String(path.to_string())),
        ])
    }

    #[tokio::test]
    async fn grep_finds_relative_content_under_artifact_only_root() -> Result<()> {
        let root = tempfile::tempdir()?;
        let log = root.path().join("server.log");
        std::fs::write(&log, "ERROR something failed\nINFO ok\n")?;

        let ctx = echo_core::tools::ToolContext {
            output_artifacts: Some(ToolOutputArtifactConfig::new(root.path(), "test")),
            ..Default::default()
        };
        let tool = GrepTool::new();
        let result = tool
            .execute_with_context(grep_params("ERROR", "server.log"), &ctx)
            .await?;
        assert!(result.success, "{}", result.error.unwrap_or_default());
        assert!(result.output.contains("something failed"));
        Ok(())
    }

    #[tokio::test]
    async fn grep_rejects_path_outside_all_roots() -> Result<()> {
        // A temp file under neither working_dir nor the artifact root must be
        // rejected as outside the allowed scope.
        let outside_root = tempfile::tempdir()?;
        let outside = outside_root.path().join("outside.txt");
        std::fs::write(&outside, "SECRET")?;
        let working = tempfile::tempdir()?;
        let artifact = tempfile::tempdir()?;

        let ctx = echo_core::tools::ToolContext {
            working_dir: Some(working.path().to_path_buf()),
            output_artifacts: Some(ToolOutputArtifactConfig::new(artifact.path(), "test")),
            ..Default::default()
        };
        let tool = GrepTool::new();
        let result = tool
            .execute_with_context(grep_params("SECRET", &outside.display().to_string()), &ctx)
            .await?;
        assert!(!result.success, "path outside all roots should be rejected");
        Ok(())
    }

    #[tokio::test]
    async fn grep_relative_path_still_uses_working_dir() -> Result<()> {
        // Relative path resolves against working_dir (preferred root) as before.
        let working = tempfile::tempdir()?;
        std::fs::write(working.path().join("notes.txt"), "TODO fix this\n")?;

        let ctx = echo_core::tools::ToolContext {
            working_dir: Some(working.path().to_path_buf()),
            ..Default::default()
        };
        let tool = GrepTool::new();
        let result = tool
            .execute_with_context(grep_params("TODO", "notes.txt"), &ctx)
            .await?;
        assert!(result.success, "{}", result.error.unwrap_or_default());
        assert!(result.output.contains("fix this"));
        Ok(())
    }

    // ── read-session scope: one tool call = one transport session ──

    use std::sync::Arc as StdArc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Delegating provider that counts how many read sessions a single tool
    /// call establishes. The session hands back a real local provider, so the
    /// search still runs on the same files and only the scoping is observed.
    struct CountingFs {
        inner: crate::filesystem::LocalFileSystem,
        opens: StdArc<AtomicUsize>,
    }

    struct CountingSession {
        inner: crate::filesystem::LocalFileSystem,
    }

    impl echo_core::tools::filesystem::FileSystemReadSession for CountingSession {
        fn files(&self) -> &dyn echo_core::tools::filesystem::FileSystem {
            &self.inner
        }
        fn finish(self: Box<Self>) -> Result<()> {
            Ok(())
        }
    }

    impl echo_core::tools::filesystem::FileSystem for CountingFs {
        fn begin_read_session(
            &self,
        ) -> Result<Option<Box<dyn echo_core::tools::filesystem::FileSystemReadSession>>> {
            self.opens.fetch_add(1, Ordering::SeqCst);
            Ok(Some(Box::new(CountingSession {
                inner: crate::filesystem::LocalFileSystem,
            })))
        }
        fn current_dir(&self) -> Result<PathBuf> {
            self.inner.current_dir()
        }
        fn canonicalize(&self, path: &Path) -> Result<PathBuf> {
            self.inner.canonicalize(path)
        }
        fn metadata(&self, path: &Path) -> Result<echo_core::tools::filesystem::FileMetadata> {
            self.inner.metadata(path)
        }
        fn symlink_metadata(
            &self,
            path: &Path,
        ) -> Result<echo_core::tools::filesystem::FileMetadata> {
            self.inner.symlink_metadata(path)
        }
        fn read(&self, path: &Path) -> Result<Vec<u8>> {
            self.inner.read(path)
        }
        fn read_dir(&self, path: &Path) -> Result<Vec<echo_core::tools::filesystem::FileEntry>> {
            self.inner.read_dir(path)
        }
        fn create_dir_all(&self, path: &Path) -> Result<()> {
            self.inner.create_dir_all(path)
        }
        fn create_file(&self, path: &Path) -> Result<()> {
            self.inner.create_file(path)
        }
        fn write(&self, path: &Path, bytes: &[u8]) -> Result<()> {
            self.inner.write(path, bytes)
        }
        fn append(&self, path: &Path, bytes: &[u8]) -> Result<()> {
            self.inner.append(path, bytes)
        }
        fn rename(&self, from: &Path, to: &Path) -> Result<()> {
            self.inner.rename(from, to)
        }
        fn remove_file(&self, path: &Path) -> Result<()> {
            self.inner.remove_file(path)
        }
        fn remove_dir(&self, path: &Path) -> Result<()> {
            self.inner.remove_dir(path)
        }
        fn remove_dir_all(&self, path: &Path) -> Result<()> {
            self.inner.remove_dir_all(path)
        }
        fn set_mode(&self, path: &Path, mode: u32) -> Result<()> {
            self.inner.set_mode(path, mode)
        }
        fn atomic_write(&self, path: &Path, bytes: &[u8]) -> Result<()> {
            self.inner.atomic_write(path, bytes)
        }
        fn checkpoint(&self, path: &Path) -> Result<Option<String>> {
            self.inner.checkpoint(path)
        }
    }

    #[tokio::test]
    async fn grep_establishes_exactly_one_read_session_per_call() -> Result<()> {
        let root = tempfile::tempdir()?;
        std::fs::create_dir_all(root.path().join("nested"))?;
        std::fs::write(root.path().join("a.txt"), "needle one\n")?;
        std::fs::write(root.path().join("nested/b.txt"), "needle two\n")?;

        let opens = StdArc::new(AtomicUsize::new(0));
        let files = StdArc::new(CountingFs {
            inner: crate::filesystem::LocalFileSystem,
            opens: StdArc::clone(&opens),
        });
        let ctx = echo_core::tools::ToolContext {
            working_dir: Some(root.path().to_path_buf()),
            ..Default::default()
        };
        let tool = GrepTool::new().with_file_system(files);
        let result = tool
            .execute_with_context(grep_params("needle", "."), &ctx)
            .await?;
        assert!(result.success, "{}", result.error.unwrap_or_default());
        assert!(result.output.contains("needle one"));
        assert!(result.output.contains("needle two"));
        assert_eq!(
            opens.load(Ordering::SeqCst),
            1,
            "one tool call must reuse a single read session"
        );
        Ok(())
    }

    // ── remote content search seam ──

    use echo_core::tools::search::{
        ContentSearchOutcome, ContentSearchProvider, ContentSearchRequest,
    };

    /// 提供者在一次调用里看到的请求快照，用来断言工具确实按实际匹配语义转发。
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct RecordedRequest {
        root: PathBuf,
        pattern: String,
        glob: Option<String>,
        context_lines: usize,
        case_insensitive: bool,
        max_results: usize,
    }

    enum FakeReply {
        Matches(Vec<String>, bool),
        Unsupported,
        Fail,
    }

    struct FakeProvider {
        reply: FakeReply,
        seen: std::sync::Mutex<Option<RecordedRequest>>,
    }

    impl FakeProvider {
        fn new(reply: FakeReply) -> Self {
            Self {
                reply,
                seen: std::sync::Mutex::new(None),
            }
        }

        fn recorded(&self) -> Option<RecordedRequest> {
            self.seen.lock().ok().and_then(|slot| slot.clone())
        }
    }

    impl ContentSearchProvider for FakeProvider {
        fn search(
            &self,
            request: &ContentSearchRequest<'_>,
        ) -> Result<Option<ContentSearchOutcome>> {
            if let Ok(mut slot) = self.seen.lock() {
                *slot = Some(RecordedRequest {
                    root: request.root.to_path_buf(),
                    pattern: request.pattern.to_string(),
                    glob: request.glob.map(str::to_string),
                    context_lines: request.context_lines,
                    case_insensitive: request.case_insensitive,
                    max_results: request.max_results,
                });
            }
            match &self.reply {
                FakeReply::Matches(records, truncated) => Ok(Some(ContentSearchOutcome {
                    records: records.clone(),
                    truncated: *truncated,
                })),
                FakeReply::Unsupported => Ok(None),
                FakeReply::Fail => Err(ToolError::ExecutionFailed {
                    tool: "grep".to_string(),
                    message: "remote search transport lost".to_string(),
                }
                .into()),
            }
        }
    }

    #[tokio::test]
    async fn grep_uses_provider_records_without_local_traversal() -> Result<()> {
        // 目标目录在本机不存在：任何本机遍历都会变成 "Path does not exist"，
        // 因此成功返回且带远端记录即证明结果没有经过本机逐文件读取。
        let working_dir = tempfile::tempdir()?;
        let working = std::fs::canonicalize(working_dir.path())?;
        let opens = StdArc::new(AtomicUsize::new(0));
        let files = StdArc::new(CountingFs {
            inner: crate::filesystem::LocalFileSystem,
            opens: StdArc::clone(&opens),
        });
        let provider = StdArc::new(FakeProvider::new(FakeReply::Matches(
            vec!["/srv/app/src/remote.rs:7:  remote-only-hit".to_string()],
            false,
        )));
        let ctx = echo_core::tools::ToolContext {
            working_dir: Some(working.clone()),
            ..Default::default()
        };
        let tool = GrepTool::new()
            .with_file_system(files)
            .with_search_provider(Some(provider));
        let result = tool
            .execute_with_context(grep_params("needle", "remote-project"), &ctx)
            .await?;

        assert!(result.success, "{}", result.error.unwrap_or_default());
        assert!(
            result
                .output
                .contains("/srv/app/src/remote.rs:7:  remote-only-hit")
        );
        assert!(result.output.contains("1 matches returned"));
        assert_eq!(
            opens.load(Ordering::SeqCst),
            0,
            "提供者命中时不得建立本机遍历会话"
        );
        Ok(())
    }

    #[tokio::test]
    async fn grep_falls_back_to_local_scan_when_provider_returns_none() -> Result<()> {
        // `None` 是“本提供者不支持”，回退结果必须与未注入提供者时逐字相同。
        let root = tempfile::tempdir()?;
        std::fs::create_dir_all(root.path().join("nested"))?;
        std::fs::write(root.path().join("a.txt"), "alpha needle\nbeta\n")?;
        std::fs::write(root.path().join("nested/b.txt"), "gamma needle\n")?;
        let ctx = echo_core::tools::ToolContext {
            working_dir: Some(root.path().to_path_buf()),
            ..Default::default()
        };

        let baseline = GrepTool::new()
            .execute_with_context(grep_params("needle", "."), &ctx)
            .await?;
        let with_none_provider = GrepTool::new()
            .with_search_provider(Some(StdArc::new(FakeProvider::new(FakeReply::Unsupported))))
            .execute_with_context(grep_params("needle", "."), &ctx)
            .await?;

        assert!(baseline.success, "{}", baseline.error.unwrap_or_default());
        assert!(
            with_none_provider.success,
            "{}",
            with_none_provider.error.unwrap_or_default()
        );
        assert_eq!(baseline.output, with_none_provider.output);
        assert_eq!(baseline.truncated, with_none_provider.truncated);
        assert!(baseline.output.contains("alpha needle"));
        assert!(baseline.output.contains("gamma needle"));
        Ok(())
    }

    #[tokio::test]
    async fn grep_propagates_provider_error_instead_of_degrading() -> Result<()> {
        let root = tempfile::tempdir()?;
        std::fs::write(root.path().join("a.txt"), "needle\n")?;
        let ctx = echo_core::tools::ToolContext {
            working_dir: Some(root.path().to_path_buf()),
            ..Default::default()
        };
        let tool = GrepTool::new()
            .with_search_provider(Some(StdArc::new(FakeProvider::new(FakeReply::Fail))));
        let outcome = tool
            .execute_with_context(grep_params("needle", "."), &ctx)
            .await;

        let error = match outcome {
            Ok(result) => {
                return Err(ToolError::ExecutionFailed {
                    tool: "grep".to_string(),
                    message: format!(
                        "provider failure was swallowed into success: {}",
                        result.output
                    ),
                }
                .into());
            }
            Err(error) => error,
        };
        assert!(
            error.to_string().contains("remote search transport lost"),
            "{error}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn grep_forwards_search_request_fields() -> Result<()> {
        // 提供者必须看到与实际匹配语义一致的字段：大小写开关跟随
        // RegexBuilder 的 case_insensitive，上下文/glob/上界原样转发。
        let root = tempfile::tempdir()?;
        let working = std::fs::canonicalize(root.path())?;
        let provider = StdArc::new(FakeProvider::new(FakeReply::Matches(Vec::new(), false)));
        let recorded_handle = StdArc::clone(&provider);
        let ctx = echo_core::tools::ToolContext {
            working_dir: Some(working.clone()),
            ..Default::default()
        };
        let parameters = ToolParameters::from([
            ("pattern".to_string(), Value::String("needle".to_string())),
            (
                "path".to_string(),
                Value::String(working.display().to_string()),
            ),
            ("glob".to_string(), Value::String("*.rs".to_string())),
            ("case_insensitive".to_string(), Value::Bool(true)),
            ("context".to_string(), serde_json::json!(2)),
        ]);
        let tool = GrepTool::new().with_search_provider(Some(provider));
        let result = tool.execute_with_context(parameters, &ctx).await?;

        assert!(result.success, "{}", result.error.unwrap_or_default());
        assert_eq!(result.output, "No matches found.");
        let recorded = recorded_handle
            .recorded()
            .ok_or_else(|| ToolError::ExecutionFailed {
                tool: "grep".to_string(),
                message: "provider was never consulted".to_string(),
            })?;
        assert_eq!(recorded.root, working);
        assert_eq!(recorded.pattern, "needle");
        assert_eq!(recorded.glob.as_deref(), Some("*.rs"));
        assert_eq!(recorded.context_lines, 2);
        assert!(recorded.case_insensitive);
        assert_eq!(recorded.max_results, CONTENT_SEARCH_MAX_RESULTS);
        Ok(())
    }

    #[tokio::test]
    async fn grep_marks_result_truncated_when_provider_truncates() -> Result<()> {
        let working_dir = tempfile::tempdir()?;
        let working = std::fs::canonicalize(working_dir.path())?;
        let ctx = echo_core::tools::ToolContext {
            working_dir: Some(working),
            ..Default::default()
        };

        let truncated = GrepTool::new()
            .with_search_provider(Some(StdArc::new(FakeProvider::new(FakeReply::Matches(
                vec!["/srv/a.rs:1:  first".to_string()],
                true,
            )))))
            .execute_with_context(grep_params("needle", "remote-project"), &ctx)
            .await?;
        assert!(truncated.success, "{}", truncated.error.unwrap_or_default());
        assert!(truncated.truncated, "远端只回传命中集前缀时必须标记截断");

        let complete = GrepTool::new()
            .with_search_provider(Some(StdArc::new(FakeProvider::new(FakeReply::Matches(
                vec!["/srv/a.rs:1:  first".to_string()],
                false,
            )))))
            .execute_with_context(grep_params("needle", "remote-project"), &ctx)
            .await?;
        assert!(complete.success, "{}", complete.error.unwrap_or_default());
        assert!(!complete.truncated);
        Ok(())
    }
}
