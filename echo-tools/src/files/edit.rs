//! EditFileTool — precise code editing with diff output
//!
//! Provides search-replace editing with:
//! - Exact string match (single or all occurrences)
//! - Unified diff output showing what changed
//! - Dry-run mode to preview changes without applying
//! - Helpful error messages with line numbers and fuzzy match hints

use super::resolve_path;
use echo_core::error::{Result, ToolError};
use echo_core::tools::permission::ToolPermission;
use echo_core::tools::{Tool, ToolParameters, ToolResult};
use futures::future::BoxFuture;
use serde_json::{Value, json};
use similar::udiff;
use std::collections::HashSet;
use std::path::PathBuf;
use tokio::fs;

// ── EditFileTool ────────────────────────────────────────────────────────────────

/// Precise code editing tool with diff output.
///
/// Replaces `old_content` with `new_content` in a file. Returns a unified diff
/// showing exactly what changed. Supports `replace_all` to replace every
/// occurrence and `dry_run` to preview without writing.
pub struct EditFileTool {
    base_dir: Option<PathBuf>,
}

impl EditFileTool {
    pub fn new() -> Self {
        Self { base_dir: None }
    }

    pub fn with_base_dir(base: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: Some(base.into()),
        }
    }
}

impl Default for EditFileTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for EditFileTool {
    fn name(&self) -> &str {
        "edit_file"
    }

    fn description(&self) -> &str {
        "Edit a file by replacing old content with new content. Returns a unified diff showing what changed. Use replace_all=true to replace every occurrence. Use dry_run=true to preview changes without writing."
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Read, ToolPermission::Write]
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File path to edit"
                },
                "old_content": {
                    "type": "string",
                    "description": "The exact text to find and replace. Must match exactly (including whitespace and indentation)."
                },
                "new_content": {
                    "type": "string",
                    "description": "The text to replace it with."
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "Replace all occurrences instead of just the first (default: false)"
                },
                "dry_run": {
                    "type": "boolean",
                    "description": "Preview changes without writing to disk (default: false)"
                }
            },
            "required": ["path", "old_content", "new_content"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let path_str = parameters
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("path".to_string()))?;

            let old_content = parameters
                .get("old_content")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("old_content".to_string()))?;

            let new_content = parameters
                .get("new_content")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("new_content".to_string()))?;

            let replace_all = parameters
                .get("replace_all")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let dry_run = parameters
                .get("dry_run")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let path = resolve_path("edit_file", path_str, &self.base_dir)?;

            if !path.exists() {
                return Ok(ToolResult::error(format!(
                    "File does not exist: {}",
                    path.display()
                )));
            }

            if !path.is_file() {
                return Ok(ToolResult::error(format!(
                    "'{}' is not a file",
                    path.display()
                )));
            }

            let original =
                fs::read_to_string(&path)
                    .await
                    .map_err(|e| ToolError::ExecutionFailed {
                        tool: "edit_file".to_string(),
                        message: format!("Failed to read file: {}", e),
                    })?;

            // Validate old_content exists in file
            if !original.contains(old_content) {
                let suggestion = find_nearby_match(old_content, &original);
                let mut msg = format!(
                    "old_content not found in '{}'. Ensure the text matches exactly (whitespace, indentation, etc.).",
                    path.display()
                );
                if let Some(s) = suggestion {
                    msg.push_str(&format!(
                        "\n\nHint: A similar block was found — check for whitespace or indentation differences:\n{}",
                        s
                    ));
                }
                return Ok(ToolResult::error(msg));
            }

            // Count occurrences
            let count = original.matches(old_content).count();
            if count > 1 && !replace_all {
                let occurrences = find_occurrence_lines(&original, old_content);
                let occ_strs: Vec<String> = occurrences.iter().map(|n| n.to_string()).collect();
                return Ok(ToolResult::error(format!(
                    "old_content found {} times in '{}'. Set replace_all=true to replace all, or narrow old_content to match a unique occurrence.\nOccurrences at lines: {}",
                    count,
                    path.display(),
                    occ_strs.join(", ")
                )));
            }

            // Apply replacement
            let updated = if replace_all {
                original.replace(old_content, new_content)
            } else {
                original.replacen(old_content, new_content, 1)
            };

            // Generate unified diff
            let filename = path.display().to_string();
            let diff = udiff::unified_diff(
                similar::Algorithm::default(),
                &original,
                &updated,
                3,
                Some((&filename, &filename)),
            );

            if diff.is_empty() {
                return Ok(ToolResult::success(format!(
                    "No changes in '{}'",
                    path.display()
                )));
            }

            // Add ANSI coloring to the diff for terminal display
            let colored_diff = colorize_diff(&diff);

            if dry_run {
                return Ok(ToolResult::success(format!(
                    "[DRY RUN] Changes preview for '{}':\n{}\n\n(No changes written — set dry_run=false to apply)",
                    path.display(),
                    colored_diff
                )));
            }

            // Create git checkpoint before mutation
            let checkpoint_tag = crate::git_checkpoint::create_checkpoint(&path);
            if checkpoint_tag.is_some() {
                crate::git_checkpoint::cleanup_old_checkpoints(&path, 10);
            }

            // Create a backup before overwriting ({filename}.bak)
            let bak_path = PathBuf::from(format!("{}.bak", path.display()));
            fs::copy(&path, &bak_path)
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: "edit_file".to_string(),
                    message: format!("Failed to create backup: {}", e),
                })?;

            // Write updated content
            fs::write(&path, &updated)
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: "edit_file".to_string(),
                    message: format!("Failed to write file: {}", e),
                })?;

            let replaced_count = if replace_all { count } else { 1 };
            let mut result = ToolResult::success(format!(
                "Edited '{}' ({} replacement{}):\n{}",
                path.display(),
                replaced_count,
                if replaced_count > 1 { "s" } else { "" },
                colored_diff
            ))
            .with_meta("backup_path", bak_path.to_string_lossy().to_string())
            .with_meta("original_size", original.len().to_string())
            .with_meta("updated_size", updated.len().to_string());

            if let Some(tag) = checkpoint_tag {
                result = result.with_meta("git_checkpoint", tag);
            }

            Ok(result)
        })
    }
}

/// Find line numbers where `needle` occurs in `haystack`
fn find_occurrence_lines(haystack: &str, needle: &str) -> Vec<usize> {
    let mut lines = Vec::new();
    let mut search_from = 0;
    while let Some(pos) = haystack[search_from..].find(needle) {
        let abs_pos = search_from + pos;
        let line_num = haystack[..abs_pos].lines().count() + 1;
        lines.push(line_num);
        search_from = abs_pos + needle.len().max(1);
    }
    lines
}

/// Try to find a similar substring to help the LLM debug mismatches.
fn find_nearby_match(old_content: &str, original: &str) -> Option<String> {
    let first_line = old_content.lines().next()?;
    if first_line.trim().is_empty() {
        return None;
    }

    let truncated_line: String;
    let search_term = if first_line.chars().count() > 30 {
        truncated_line = first_line.chars().take(30).collect();
        &truncated_line
    } else {
        first_line
    };

    let mut matches = Vec::new();
    for (i, line) in original.lines().enumerate() {
        if line.contains(search_term.trim())
            || bigram_similarity(line.trim(), search_term.trim()) > 0.6
        {
            let start = i.saturating_sub(2);
            let end = (i + 3).min(original.lines().count());
            let snippet: Vec<String> = original
                .lines()
                .skip(start)
                .take(end - start)
                .enumerate()
                .map(|(j, l)| format!("  {}: {}", start + j + 1, l))
                .collect();
            matches.push(snippet.join("\n"));
            if matches.len() >= 2 {
                break;
            }
        }
    }

    if matches.is_empty() {
        None
    } else {
        Some(matches.join("\n---\n"))
    }
}

/// Jaccard similarity on character bigrams
fn bigram_similarity(a: &str, b: &str) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }

    fn bigrams(s: &str) -> HashSet<(char, char)> {
        let chars: Vec<char> = s.chars().collect();
        chars.windows(2).map(|w| (w[0], w[1])).collect()
    }

    let ba = bigrams(a);
    let bb = bigrams(b);
    let intersection = ba.intersection(&bb).count() as f64;
    let union = ba.union(&bb).count() as f64;
    if union == 0.0 {
        0.0
    } else {
        intersection / union
    }
}

/// Add ANSI colors to a unified diff string for terminal display.
///
/// - File headers (`---`/`+++`) → bold
/// - Hunk headers (`@@`) → cyan
/// - Added lines (`+`) → green
/// - Removed lines (`-`) → red
/// - Context lines → unchanged
fn colorize_diff(diff: &str) -> String {
    let mut out = String::with_capacity(diff.len() + diff.len() / 4);
    for line in diff.lines() {
        if line.starts_with("---") || line.starts_with("+++") {
            out.push_str("\x1b[1m");
            out.push_str(line);
            out.push_str("\x1b[0m");
        } else if line.starts_with("@@") {
            out.push_str("\x1b[36m");
            out.push_str(line);
            out.push_str("\x1b[0m");
        } else if line.starts_with('+') {
            out.push_str("\x1b[32m");
            out.push_str(line);
            out.push_str("\x1b[0m");
        } else if line.starts_with('-') {
            out.push_str("\x1b[31m");
            out.push_str(line);
            out.push_str("\x1b[0m");
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_occurrence_lines() {
        let text = "line1\nline2\nline1\nline3\nline1\n";
        let lines = find_occurrence_lines(text, "line1");
        assert_eq!(lines, vec![1, 3, 5]);
    }

    #[test]
    fn test_find_occurrence_lines_multiline() {
        let text =
            "fn main() {\n    println!(\"hi\");\n}\n\nfn main() {\n    println!(\"bye\");\n}\n";
        let lines = find_occurrence_lines(text, "fn main() {");
        assert_eq!(lines, vec![1, 5]);
    }

    #[test]
    fn test_bigram_similarity() {
        assert!(bigram_similarity("hello world", "hello world") > 0.9);
        assert!(bigram_similarity("hello world", "hello rust") > 0.3);
        assert!(bigram_similarity("abc", "xyz") < 0.3);
    }

    #[test]
    fn test_edit_produces_diff() {
        let original = "hello\nworld\nfoo\n";
        let updated = "hello\nrust\nfoo\n";
        let diff = udiff::unified_diff(
            similar::Algorithm::default(),
            original,
            updated,
            3,
            Some(("test.txt", "test.txt")),
        );
        assert!(diff.contains("-world"));
        assert!(diff.contains("+rust"));
    }

    #[test]
    fn test_edit_no_change() {
        let original = "hello\nworld\n";
        let diff = udiff::unified_diff(
            similar::Algorithm::default(),
            original,
            original,
            3,
            Some(("test.txt", "test.txt")),
        );
        assert!(diff.is_empty());
    }

    #[test]
    fn test_colorize_diff() {
        let raw = "--- a/test.txt\n+++ b/test.txt\n@@ -1,2 +1,2 @@\n hello\n-world\n+rust\n";
        let colored = colorize_diff(raw);
        assert!(colored.contains("\x1b[1m")); // bold header
        assert!(colored.contains("\x1b[36m")); // cyan hunk
        assert!(colored.contains("\x1b[31m")); // red removed
        assert!(colored.contains("\x1b[32m")); // green added
        // Context lines should not have ANSI codes
        assert!(colored.contains(" hello\n"));
    }
}
