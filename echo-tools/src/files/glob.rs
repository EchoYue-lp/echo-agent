//! Glob tool — find files by name pattern
//!
//! Walks a directory tree and returns file paths matching a glob pattern.

use echo_core::error::{Result, ToolError};
use echo_core::tools::{Tool, ToolParameters, ToolResult};
use futures::future::BoxFuture;
use serde_json::{Value, json};
use std::path::PathBuf;
use tokio::fs;

// ── GlobTool ────────────────────────────────────────────────────────────────

/// Find files by name pattern.
///
/// Walks the directory tree from `path` (default: `.`) and returns
/// file paths matching the `pattern` glob (e.g. `**/*.rs`, `src/**/*.ts`).
pub struct GlobTool {
    base_dir: Option<PathBuf>,
}

impl GlobTool {
    pub fn new() -> Self {
        Self { base_dir: None }
    }

    pub fn with_base_dir(base: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: Some(base.into()),
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
        "Find files by name pattern. Returns file paths matching a glob pattern (e.g. '**/*.rs', 'src/**/*.ts', '*.md'). Supports ** for recursive directory matching."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern to match file names (e.g. '**/*.rs', 'src/**/*.ts', '*.md')"
                },
                "path": {
                    "type": "string",
                    "description": "Directory to search in (default: current directory)"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of file paths to return (default: 200)"
                }
            },
            "required": ["pattern"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let pattern_str = parameters
                .get("pattern")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("pattern".to_string()))?;

            let path_str = parameters
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or(".");

            let max_results = parameters
                .get("max_results")
                .and_then(|v| v.as_u64())
                .unwrap_or(200) as usize;

            let search_path = if let Some(ref base) = self.base_dir {
                let resolved = if std::path::Path::new(path_str).is_absolute() {
                    PathBuf::from(path_str)
                } else {
                    base.join(path_str)
                };
                if !resolved.starts_with(base) {
                    return Ok(ToolResult::error(format!(
                        "Path '{}' is outside the allowed directory scope",
                        path_str
                    )));
                }
                resolved
            } else {
                PathBuf::from(path_str)
            };

            if !search_path.exists() {
                return Ok(ToolResult::error(format!(
                    "Path does not exist: {}",
                    search_path.display()
                )));
            }

            if !search_path.is_dir() {
                return Ok(ToolResult::error(format!(
                    "Path is not a directory: {}",
                    search_path.display()
                )));
            }

            let mut results = Vec::new();
            walk_glob(&search_path, pattern_str, max_results, &mut results).await;

            if results.is_empty() {
                return Ok(ToolResult::success(format!(
                    "No files matching '{}' found.",
                    pattern_str
                )));
            }

            let total = results.len();
            let output = results.join("\n");
            let summary = format!("\n\n{} files matched", total);

            Ok(ToolResult::success(format!("{}{}", output, summary)))
        })
    }
}

async fn walk_glob(
    dir: &std::path::Path,
    pattern: &str,
    max_results: usize,
    results: &mut Vec<String>,
) {
    let mut entries = match fs::read_dir(dir).await {
        Ok(e) => e,
        Err(_) => return,
    };

    let mut entry_paths: Vec<PathBuf> = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        entry_paths.push(entry.path());
    }
    entry_paths.sort();

    for path in entry_paths {
        if results.len() >= max_results {
            break;
        }

        // Skip hidden and common ignored directories
        if path.is_dir() {
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
        }

        if path.is_dir() {
            Box::pin(walk_glob(&path, pattern, max_results, results)).await;
        } else if path.is_file() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if glob_pattern_matches(pattern, name, &path) {
                    results.push(path.display().to_string());
                }
            }
        }
    }
}

/// Match a glob pattern against a file name.
///
/// Supports:
/// - `*` matches any sequence of characters (except /)
/// - `**` matches any path prefix (handled by walking)
/// - `?` matches any single character
/// - `{a,b}` matches a or b
/// - `[abc]` matches any character in the set
fn glob_pattern_matches(pattern: &str, name: &str, full_path: &std::path::Path) -> bool {
    // Handle ** patterns (recursive) — they match at any directory level
    if pattern.contains("**") {
        return glob_with_doublestar(pattern, full_path);
    }

    // For simple patterns, just match the file name
    let name_pattern = if pattern.contains('/') {
        // If pattern contains /, match against relative path components
        pattern.to_string()
    } else {
        pattern.to_string()
    };

    if pattern.contains('/') {
        // Match against path suffix
        glob_match_advanced(&name_pattern, &full_path.display().to_string())
    } else {
        glob_match_advanced(&name_pattern, name)
    }
}

fn glob_with_doublestar(pattern: &str, full_path: &std::path::Path) -> bool {
    let path_str = full_path.display().to_string();

    // Split pattern on ** and try matching at each directory level
    let parts: Vec<&str> = pattern.split("**").collect();

    if parts.len() == 2 {
        let prefix = parts[0];
        let suffix = parts[1];

        // Try matching at each level
        if prefix.is_empty() && suffix.is_empty() {
            return true;
        }

        if prefix.is_empty() {
            // **/*.rs — match any path ending with suffix
            return path_str.ends_with(suffix.trim_start_matches('/'))
                || path_str.ends_with(suffix.trim_start_matches('/').trim_start_matches('\\'));
        }

        if suffix.is_empty() {
            return path_str.contains(prefix.trim_end_matches('/'));
        }

        // src/**/*.rs — match paths starting with prefix and ending with suffix
        let suffix_clean = suffix.trim_start_matches('/');
        if let Some(pos) = path_str.find(prefix.trim_end_matches('/')) {
            let rest = &path_str[pos + prefix.trim_end_matches('/').len()..];
            return rest.ends_with(suffix_clean);
        }
        return false;
    }

    // Fallback: simple match
    glob_match_advanced(pattern, &path_str)
}

/// Advanced glob matching with * ? {a,b} [abc]
fn glob_match_advanced(pattern: &str, text: &str) -> bool {
    let mut pi = 0;
    let mut ti = 0;
    let p_chars: Vec<char> = pattern.chars().collect();
    let t_chars: Vec<char> = text.chars().collect();

    while pi < p_chars.len() && ti < t_chars.len() {
        match p_chars[pi] {
            '*' => {
                // Match any sequence
                pi += 1;
                if pi == p_chars.len() {
                    return true;
                }
                for skip in ti..=t_chars.len() {
                    if glob_match_advanced(&pattern[pi..], &text[skip..]) {
                        return true;
                    }
                }
                return false;
            }
            '?' => {
                pi += 1;
                ti += 1;
            }
            '[' => {
                // Character class
                pi += 1;
                let mut found = false;
                let negated = pi < p_chars.len() && p_chars[pi] == '!';
                if negated {
                    pi += 1;
                }
                while pi < p_chars.len() && p_chars[pi] != ']' {
                    if ti < t_chars.len() && p_chars[pi] == t_chars[ti] {
                        found = true;
                    }
                    pi += 1;
                }
                if pi < p_chars.len() {
                    pi += 1; // skip ]
                }
                if negated {
                    found = !found;
                }
                if !found {
                    return false;
                }
                ti += 1;
            }
            '{' => {
                // Brace expansion
                let close = p_chars[pi..].iter().position(|&c| c == '}');
                if let Some(close_idx) = close {
                    let brace_content: String = p_chars[pi + 1..pi + close_idx].iter().collect();
                    let alternatives: Vec<&str> = brace_content.split(',').collect();
                    let after_brace: String = p_chars[pi + close_idx + 1..].iter().collect();
                    let before_brace: String = p_chars[..pi].iter().collect();
                    for alt in alternatives {
                        let full = format!("{}{}{}", before_brace, alt.trim(), after_brace);
                        if glob_match_advanced(&full, text) {
                            return true;
                        }
                    }
                    return false;
                }
                // No closing brace, treat as literal
                if p_chars[pi] == t_chars[ti] {
                    pi += 1;
                    ti += 1;
                } else {
                    return false;
                }
            }
            c => {
                if ti < t_chars.len() && c == t_chars[ti] {
                    pi += 1;
                    ti += 1;
                } else {
                    return false;
                }
            }
        }
    }

    // Consume trailing * in pattern
    while pi < p_chars.len() && p_chars[pi] == '*' {
        pi += 1;
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
}
