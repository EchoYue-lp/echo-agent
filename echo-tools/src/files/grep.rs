//! Grep tool — search file contents by regex pattern
//!
//! Uses the `regex` crate for pattern matching. Supports context lines,
//! file type filtering, and case-insensitive mode.

use echo_core::error::{Result, ToolError};
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
        "Search file contents by regex pattern. Walks directories and returns matching lines with file paths and line numbers. Use glob parameter to filter file types (e.g. '*.rs', '*.py')."
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
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of matched lines to return (default: 100)"
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

            let context_lines = parameters
                .get("context")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;

            let max_results = parameters
                .get("max_results")
                .and_then(|v| v.as_u64())
                .unwrap_or(100) as usize;

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
            let mut total_matches = 0usize;

            if search_path.is_file() {
                search_file(
                    &search_path,
                    &regex,
                    glob_filter,
                    context_lines,
                    max_results,
                    &mut results,
                    &mut total_matches,
                )
                .await?;
            } else if search_path.is_dir() {
                walk_and_search(
                    &search_path,
                    &regex,
                    glob_filter,
                    context_lines,
                    max_results,
                    &mut results,
                    &mut total_matches,
                )
                .await?;
            }

            if results.is_empty() {
                return Ok(ToolResult::success("No matches found.".to_string()));
            }

            let output = results.join("\n");
            let summary = if total_matches > max_results {
                format!(
                    "\n\n... (showing {} of {} matches)",
                    max_results, total_matches
                )
            } else {
                format!("\n\n{} matches found", total_matches)
            };

            Ok(ToolResult::success(format!("{}{}", output, summary)))
        })
    }
}

async fn search_file(
    path: &Path,
    regex: &regex::Regex,
    glob_filter: Option<&str>,
    context_lines: usize,
    max_results: usize,
    results: &mut Vec<String>,
    total_matches: &mut usize,
) -> Result<()> {
    // Check glob filter
    if let Some(glob) = glob_filter {
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if !glob_matches(glob, name) {
                return Ok(());
            }
        }
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
            *total_matches += 1;
            if results.len() < max_results {
                let line_num = line_idx + 1;

                if context_lines > 0 {
                    // Show context lines
                    let start = line_idx.saturating_sub(context_lines);
                    let end = (line_idx + context_lines + 1).min(lines.len());

                    for (ctx_idx, line) in lines.iter().enumerate().skip(start).take(end - start) {
                        let ctx_num = ctx_idx + 1;
                        let prefix = if ctx_idx == line_idx { ">" } else { " " };
                        results.push(format!("{}{}:{}:  {}", prefix, path_display, ctx_num, line));
                    }
                    if end < lines.len() || start > 0 {
                        results.push(format!("  {}---", path_display));
                    }
                } else {
                    results.push(format!("{}:{}:  {}", path_display, line_num, line));
                }
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
    max_results: usize,
    results: &mut Vec<String>,
    total_matches: &mut usize,
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
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if name.starts_with('.')
                || name == "target"
                || name == "node_modules"
                || name == "__pycache__"
                || name == ".git"
                || name == "vendor"
                || name == "build"
                || name == "dist"
            {
                continue;
            }
        }
        entry_paths.push(path);
    }
    entry_paths.sort();

    for path in entry_paths {
        if *total_matches >= max_results && results.len() >= max_results {
            break;
        }

        if path.is_dir() {
            Box::pin(walk_and_search(
                &path,
                regex,
                glob_filter,
                context_lines,
                max_results,
                results,
                total_matches,
            ))
            .await?;
        } else if path.is_file() {
            search_file(
                &path,
                regex,
                glob_filter,
                context_lines,
                max_results,
                results,
                total_matches,
            )
            .await?;
        }
    }

    Ok(())
}

/// Simple glob matching supporting * and ? wildcards
fn glob_matches(pattern: &str, name: &str) -> bool {
    // Handle brace patterns like *.{rs,ts}
    if pattern.contains('{') && pattern.contains('}') {
        if let Some(start) = pattern.find('{') {
            if let Some(end) = pattern.find('}') {
                let prefix = &pattern[..start];
                let suffix = &pattern[end + 1..];
                let alternatives: Vec<&str> = pattern[start + 1..end].split(',').collect();
                for alt in alternatives {
                    let full = format!("{}{}{}", prefix, alt.trim(), suffix);
                    if glob_matches_simple(&full, name) {
                        return true;
                    }
                }
                return false;
            }
        }
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

    if pat[pi] == '*' {
        // Try matching 0 or more characters
        for skip in ni..=name.len() {
            if glob_match_inner(pat, name, pi + 1, skip) {
                return true;
            }
        }
        return false;
    }

    if ni >= name.len() {
        return false;
    }

    if pat[pi] == '?' || pat[pi] == name[ni] {
        return glob_match_inner(pat, name, pi + 1, ni + 1);
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
