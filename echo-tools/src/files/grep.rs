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

            // Effective base: construction-time base_dir (with confinement)
            // takes priority; otherwise fall back to runtime working_dir.
            let effective_base: Option<PathBuf> = self
                .base_dir
                .clone()
                .or_else(|| ctx.working_dir.as_ref().map(|p| p.to_path_buf()));

            let search_path = if let Some(ref base) = effective_base {
                let resolved = if Path::new(path_str).is_absolute() {
                    PathBuf::from(path_str)
                } else {
                    base.join(path_str)
                };
                if !resolved.starts_with(base) {
                    return Ok(ToolResult::invalid_arguments(format!(
                        "Path '{}' is outside the allowed directory scope",
                        path_str
                    )));
                }
                resolved
            } else {
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
}
