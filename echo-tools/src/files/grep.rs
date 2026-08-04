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
use tokio::fs;

// ── GrepTool ────────────────────────────────────────────────────────────────

/// Search file contents by regex pattern.
///
/// Walks the directory tree from `path` (default: `.`), reads each file,
/// and returns lines matching the `pattern` regex. Supports context lines,
/// file glob filtering, and case-insensitive mode.
pub struct GrepTool {
    base_dir: Option<PathBuf>,
}

impl GrepTool {
    pub fn new() -> Self {
        Self { base_dir: None }
    }

    pub fn with_base_dir(base: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: Some(base.into()),
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
        "Exact text search over file contents using a regex. Returns matching lines with file paths and line numbers; use code_search for symbol-oriented code discovery."
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
                    "description": "Regular expression pattern to search for"
                },
                "path": {
                    "type": "string",
                    "description": "Directory or file path to search in (default: current directory)"
                },
                "glob": {
                    "type": "string",
                    "description": "File name glob filter (e.g. '*.rs', '*.py', '*.{ts,tsx}')"
                },
                "case_insensitive": {
                    "type": "boolean",
                    "description": "Case insensitive matching (default: false)"
                },
                "context": {
                    "type": "integer",
                    "description": "Number of context lines before and after each match (default: 0)"
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 200,
                    "description": "Results per page (default 50)"
                },
                "cursor": {
                    "type": "string",
                    "description": "Cursor from the previous page"
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
        Box::pin(async move {
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
            // then working_dir) to preserve the existing single-root behavior;
            // absolute `path` is used as-is. Confinement passes if the
            // canonicalized path starts with *any* allowed root.
            let mut allowed_roots: Vec<PathBuf> = Vec::new();
            if let Some(base) = &self.base_dir {
                allowed_roots.push(base.clone());
            }
            if let Some(working) = &ctx.working_dir {
                allowed_roots.push(working.clone());
            }
            if let Some(artifacts) = ctx.output_artifacts.as_ref() {
                allowed_roots.push(artifacts.root_dir.clone());
            }

            let preferred_root = self
                .base_dir
                .clone()
                .or_else(|| ctx.working_dir.as_ref().map(|p| p.to_path_buf()));

            let search_path = if let Some(ref base) = preferred_root {
                let resolved = if Path::new(path_str).is_absolute() {
                    PathBuf::from(path_str)
                } else {
                    base.join(path_str)
                };
                // Canonicalize both the candidate and the roots so symlink /
                // `..` escapes are caught. Missing roots (e.g. an uncreated
                // artifact dir) are skipped rather than erroring.
                let resolved_canon = match tokio::fs::canonicalize(&resolved).await {
                    Ok(p) => p,
                    Err(_) => resolved.clone(),
                };
                let within_scope = canonicalize_roots(&allowed_roots)
                    .await
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
                // No roots configured — no confinement (legacy behavior).
                PathBuf::from(path_str)
            };

            if !search_path.exists() {
                return Ok(ToolResult::invalid_arguments(format!(
                    "Path does not exist: {}",
                    search_path.display()
                )));
            }

            let mut results = Vec::new();

            if search_path.is_file() {
                search_file(
                    &search_path,
                    &regex,
                    glob_filter,
                    context_lines,
                    &mut results,
                )
                .await?;
            } else if search_path.is_dir() {
                walk_and_search(
                    &search_path,
                    &regex,
                    glob_filter,
                    context_lines,
                    &mut results,
                )
                .await?;
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
async fn canonicalize_roots(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut canon: Vec<PathBuf> = Vec::with_capacity(roots.len());
    for root in roots {
        // Missing root (e.g. an uncreated artifact dir) is skipped, not errored.
        if let Ok(p) = tokio::fs::canonicalize(root).await {
            canon.push(p);
        }
    }
    canon
}

async fn search_file(
    path: &Path,
    regex: &regex::Regex,
    glob_filter: Option<&str>,
    context_lines: usize,
    results: &mut Vec<String>,
) -> Result<()> {
    // Check glob filter
    if let Some(glob) = glob_filter
        && let Some(name) = path.file_name().and_then(|n| n.to_str())
        && !glob_matches(glob, name)
    {
        return Ok(());
    }

    // Skip binary files and large files
    let metadata = fs::metadata(path)
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            tool: "grep".to_string(),
            message: format!("Cannot read metadata for {}: {}", path.display(), e),
        })?;

    if metadata.len() > 10 * 1024 * 1024 {
        // Skip files > 10MB
        return Ok(());
    }

    let content = match fs::read_to_string(path).await {
        Ok(c) => c,
        Err(_) => return Ok(()), // Skip binary/unreadable files
    };

    let lines: Vec<&str> = content.lines().collect();
    let path_display = path.display().to_string();

    for (line_idx, line) in lines.iter().enumerate() {
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

async fn walk_and_search(
    dir: &Path,
    regex: &regex::Regex,
    glob_filter: Option<&str>,
    context_lines: usize,
    results: &mut Vec<String>,
) -> Result<()> {
    let mut entries = match fs::read_dir(dir).await {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };

    // Collect and sort entries for deterministic output
    let mut entry_paths: Vec<PathBuf> = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
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
        if path.is_dir() {
            Box::pin(walk_and_search(
                &path,
                regex,
                glob_filter,
                context_lines,
                results,
            ))
            .await?;
        } else if path.is_file() {
            search_file(&path, regex, glob_filter, context_lines, results).await?;
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

    /// Unique temp dir for a test (no uuid dep — mirrors artifact.rs's test_root).
    fn test_dir(label: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("echo-grep-{label}-{}-{nonce}", std::process::id()))
    }

    /// Build grep parameters for the given path/pattern.
    fn grep_params(pattern: &str, path: &str) -> ToolParameters {
        ToolParameters::from([
            ("pattern".to_string(), Value::String(pattern.to_string())),
            ("path".to_string(), Value::String(path.to_string())),
        ])
    }

    #[tokio::test]
    async fn grep_finds_content_under_artifact_root() {
        // Artifact root containing a spilled log; grep by absolute path must
        // succeed even though it is not under working_dir.
        let root = test_dir("artifact");
        std::fs::create_dir_all(&root).unwrap();
        let log = root.join("server.log");
        std::fs::write(&log, "ERROR something failed\nINFO ok\n").unwrap();

        let ctx = echo_core::tools::ToolContext {
            output_artifacts: Some(ToolOutputArtifactConfig::new(&root, "test")),
            ..Default::default()
        };
        let tool = GrepTool::new();
        let result = tool
            .execute_with_context(grep_params("ERROR", &log.display().to_string()), &ctx)
            .await
            .unwrap();
        assert!(result.success, "{}", result.error.unwrap_or_default());
        assert!(result.output.contains("something failed"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn grep_rejects_path_outside_all_roots() {
        // A temp file under neither working_dir nor the artifact root must be
        // rejected as outside the allowed scope.
        let outside = test_dir("outside");
        std::fs::write(&outside, "SECRET").unwrap();

        let working = test_dir("working");
        std::fs::create_dir_all(&working).unwrap();
        let artifact = test_dir("artifact2");
        std::fs::create_dir_all(&artifact).unwrap();

        let ctx = echo_core::tools::ToolContext {
            working_dir: Some(working.clone()),
            output_artifacts: Some(ToolOutputArtifactConfig::new(&artifact, "test")),
            ..Default::default()
        };
        let tool = GrepTool::new();
        let result = tool
            .execute_with_context(grep_params("SECRET", &outside.display().to_string()), &ctx)
            .await
            .unwrap();
        assert!(!result.success, "path outside all roots should be rejected");
        std::fs::remove_file(&outside).ok();
        std::fs::remove_dir_all(&working).ok();
        std::fs::remove_dir_all(&artifact).ok();
    }

    #[tokio::test]
    async fn grep_relative_path_still_uses_working_dir() {
        // Relative path resolves against working_dir (preferred root) as before.
        let working = test_dir("rel");
        std::fs::create_dir_all(&working).unwrap();
        std::fs::write(working.join("notes.txt"), "TODO fix this\n").unwrap();

        let ctx = echo_core::tools::ToolContext {
            working_dir: Some(working.clone()),
            ..Default::default()
        };
        let tool = GrepTool::new();
        let result = tool
            .execute_with_context(grep_params("TODO", "notes.txt"), &ctx)
            .await
            .unwrap();
        assert!(result.success, "{}", result.error.unwrap_or_default());
        assert!(result.output.contains("fix this"));
        std::fs::remove_dir_all(&working).ok();
    }
}
