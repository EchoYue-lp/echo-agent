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

/// Search file contents by regex pattern.
///
/// Walks the directory tree from `path` (default: `.`), reads each file,
/// and returns lines matching the `pattern` regex. Supports context lines,
/// file glob filtering, and case-insensitive mode.
#[derive(Clone)]
pub struct GrepTool {
    base_dir: Option<PathBuf>,
    file_system: std::sync::Arc<dyn echo_core::tools::filesystem::FileSystem>,
}

impl GrepTool {
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

            if !file_system.exists(&search_path)? {
                return Ok(ToolResult::invalid_arguments(format!(
                    "Path does not exist: {}",
                    search_path.display()
                )));
            }

            let mut results = Vec::new();

            if file_system.is_file(&search_path)? {
                search_file(
                    file_system,
                    cancel,
                    &search_path,
                    &regex,
                    glob_filter,
                    context_lines,
                    &mut results,
                )?;
            } else if file_system.is_dir(&search_path)? {
                walk_and_search(
                    file_system,
                    cancel,
                    &search_path,
                    canonical_roots.as_deref(),
                    &regex,
                    (glob_filter, context_lines),
                    &mut results,
                )?;
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
}
