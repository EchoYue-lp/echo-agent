//! Summary verifier — post-compression quality checks.
//!
//! Runs lightweight rule-based checks against compression output to verify
//! that critical information was not lost. No LLM calls needed.

use crate::compression::CompressionCheckpoint;
use echo_core::llm::types::{Message, Role};

/// Result of running all verification checks against a compression output.
#[derive(Debug, Clone)]
pub struct SummaryVerification {
    /// Overall pass/fail (all P0 checks must pass).
    pub passed: bool,
    /// Individual check results.
    pub checks: Vec<VerificationCheck>,
}

/// A single verification check result.
#[derive(Debug, Clone)]
pub struct VerificationCheck {
    pub name: String,
    pub passed: bool,
    pub priority: CheckPriority,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckPriority {
    /// Must-pass checks — compression is considered broken if these fail.
    P0,
    /// Should-pass checks — warnings but don't block.
    P1,
}

/// Run all verification checks against the compressed output.
///
/// `compressed_messages` — the messages after compression (what will be sent to LLM).
/// `checkpoint` — the compression checkpoint with token stats and summary.
/// `original_messages` — the full message buffer before compression (for context).
pub fn verify_compression(
    compressed_messages: &[Message],
    checkpoint: &CompressionCheckpoint,
    original_messages: &[Message],
) -> SummaryVerification {
    let mut checks = Vec::new();

    // P0: Token target met
    checks.push(check_token_target(checkpoint));

    // P0: Summary not empty (if an LLM compressor was used)
    if checkpoint.summary.is_some() && !checkpoint.summary.as_deref().unwrap_or("").is_empty() {
        checks.push(check_summary_not_empty(checkpoint));
    }

    // P0: Last user query keywords in summary or retained messages
    checks.push(check_last_query_presence(
        original_messages,
        compressed_messages,
        checkpoint,
    ));

    // P0: Recent file paths preserved
    checks.push(check_file_paths_presence(
        original_messages,
        compressed_messages,
        checkpoint,
    ));

    // P1: Pending tasks preserved
    checks.push(check_pending_tasks(
        original_messages,
        compressed_messages,
        checkpoint,
    ));

    // P1: Errors preserved
    checks.push(check_error_presence(
        original_messages,
        compressed_messages,
        checkpoint,
    ));

    // P1: User preferences preserved
    checks.push(check_user_preferences(
        original_messages,
        compressed_messages,
        checkpoint,
    ));

    let passed = checks
        .iter()
        .filter(|c| c.priority == CheckPriority::P0)
        .all(|c| c.passed);

    SummaryVerification { passed, checks }
}

/// P0: Post-compression tokens must not exceed pre-compression tokens.
/// (A stronger "within target limit" check requires model context info not available here.)
fn check_token_target(checkpoint: &CompressionCheckpoint) -> VerificationCheck {
    let after = checkpoint.token_after;
    let before = checkpoint.token_before;
    let reduction = if before > 0 {
        (before.saturating_sub(after)) as f64 / before as f64
    } else {
        0.0
    };

    VerificationCheck {
        name: "tokens_not_increased".to_string(),
        passed: after <= before,
        priority: CheckPriority::P0,
        detail: format!(
            "Tokens: {} → {} (reduction: {:.1}%)",
            before,
            after,
            reduction * 100.0
        ),
    }
}

/// P0: Summary should not be trivially short.
fn check_summary_not_empty(checkpoint: &CompressionCheckpoint) -> VerificationCheck {
    let len = checkpoint.summary.as_deref().unwrap_or("").len();
    VerificationCheck {
        name: "summary_not_empty".to_string(),
        passed: len > 50,
        priority: CheckPriority::P0,
        detail: format!("Summary length: {} chars", len),
    }
}

/// P0: Last user query keywords should appear in the compressed output.
fn check_last_query_presence(
    original: &[Message],
    compressed: &[Message],
    checkpoint: &CompressionCheckpoint,
) -> VerificationCheck {
    // Find the last user message in original
    let last_user = original.iter().rev().find(|m| m.role == Role::User);

    match last_user {
        Some(msg) => {
            let text = msg.content.as_text().unwrap_or_default();
            // Extract key words (longest words are most likely to be distinctive)
            let keywords: Vec<&str> = text.split_whitespace().filter(|w| w.len() > 3).collect();

            if keywords.is_empty() {
                return VerificationCheck {
                    name: "last_query_presence".to_string(),
                    passed: true, // can't check if no keywords
                    priority: CheckPriority::P0,
                    detail: "No distinctive keywords found in last query".to_string(),
                };
            }

            // Check if at least 50% of keywords appear in compressed messages or summary
            let combined = format!(
                "{} {}",
                compressed
                    .iter()
                    .filter_map(|m| m.content.as_text())
                    .collect::<Vec<_>>()
                    .join(" "),
                checkpoint.summary.as_deref().unwrap_or("")
            );

            let found = keywords.iter().filter(|kw| combined.contains(*kw)).count();
            let ratio = found as f64 / keywords.len() as f64;

            VerificationCheck {
                name: "last_query_presence".to_string(),
                passed: ratio >= 0.5,
                priority: CheckPriority::P0,
                detail: format!(
                    "Last query keyword presence: {}/{} ({:.0}%)",
                    found,
                    keywords.len(),
                    ratio * 100.0
                ),
            }
        }
        None => VerificationCheck {
            name: "last_query_presence".to_string(),
            passed: true,
            priority: CheckPriority::P0,
            detail: "No user message found".to_string(),
        },
    }
}

/// P0: Recent file paths from tool calls should be preserved.
fn check_file_paths_presence(
    original: &[Message],
    compressed: &[Message],
    checkpoint: &CompressionCheckpoint,
) -> VerificationCheck {
    // Extract file paths from the last 3 turns of tool messages
    let file_paths: Vec<String> = original
        .iter()
        .rev()
        .filter(|m| m.role == Role::Tool)
        .take(6) // last ~3 turns of tool results
        .filter_map(|m| m.content.as_text())
        .flat_map(|text| extract_file_paths_from_text(&text))
        .collect();

    if file_paths.is_empty() {
        return VerificationCheck {
            name: "file_paths_presence".to_string(),
            passed: true, // no file paths to check
            priority: CheckPriority::P0,
            detail: "No file paths found in recent tool messages".to_string(),
        };
    }

    let combined = format!(
        "{} {}",
        compressed
            .iter()
            .filter_map(|m| m.content.as_text())
            .collect::<Vec<_>>()
            .join(" "),
        checkpoint.summary.as_deref().unwrap_or("")
    );

    let found = file_paths
        .iter()
        .filter(|fp| combined.contains(fp.as_str()))
        .count();
    let ratio = found as f64 / file_paths.len() as f64;

    VerificationCheck {
        name: "file_paths_presence".to_string(),
        passed: ratio >= 0.5,
        priority: CheckPriority::P0,
        detail: format!(
            "File path preservation: {}/{} ({:.0}%)",
            found,
            file_paths.len(),
            ratio * 100.0
        ),
    }
}

/// Extract likely file paths from tool output text.
///
/// Matches common source code file patterns: `src/auth.rs`, `/path/to/file`, `Cargo.toml`.
/// Filters out version numbers (1.2.3), URLs, and generic dotted words.
fn extract_file_paths_from_text(text: &str) -> Vec<String> {
    // Common source code extensions to reduce false positives
    let code_extensions = [
        ".rs", ".py", ".js", ".ts", ".go", ".java", ".c", ".cpp", ".h", ".hpp", ".toml", ".yaml",
        ".yml", ".json", ".md", ".txt", ".lock", ".sql", ".sh", ".css", ".html", ".xml", ".rb",
        ".php", ".swift", ".kt", ".scala", ".r", ".vue", ".svelte", ".tsx", ".jsx", ".tf",
        ".proto", ".graphql", ".gql",
    ];
    let mut paths = Vec::new();
    for word in text.split_whitespace() {
        let trimmed = word.trim_matches(|c: char| {
            c == '"'
                || c == '\''
                || c == '`'
                || c == ','
                || c == '('
                || c == ')'
                || c == '['
                || c == ']'
        });
        // Must contain a known extension or a path separator
        let has_code_ext = code_extensions.iter().any(|ext| trimmed.ends_with(ext));
        let has_path_sep = trimmed.contains('/');
        if (has_code_ext || has_path_sep)
            && trimmed.len() > 2
            && trimmed.len() < 200
            && !trimmed.starts_with("http")
            && !trimmed.starts_with("https")
            // Filter out version numbers and dotted numbers
            && !trimmed.chars().all(|c| c.is_ascii_digit() || c == '.')
        {
            paths.push(trimmed.to_string());
        }
    }
    paths
}

/// P1: TODO/pending task information should survive compression.
fn check_pending_tasks(
    original: &[Message],
    compressed: &[Message],
    checkpoint: &CompressionCheckpoint,
) -> VerificationCheck {
    let todo_patterns = ["TODO", "FIXME", "HACK", "PENDING", "待处理", "未完成"];

    // Find messages with TODO in the original (from the evicted range)
    let todos_in_original: Vec<String> = original
        .iter()
        .filter(|m| {
            let text = m.content.as_text().unwrap_or_default();
            todo_patterns.iter().any(|p| text.contains(p))
        })
        .filter_map(|m| m.content.as_text())
        .collect();

    if todos_in_original.is_empty() {
        return VerificationCheck {
            name: "pending_tasks_presence".to_string(),
            passed: true,
            priority: CheckPriority::P1,
            detail: "No TODO items found".to_string(),
        };
    }

    let combined = format!(
        "{} {}",
        compressed
            .iter()
            .filter_map(|m| m.content.as_text())
            .collect::<Vec<_>>()
            .join(" "),
        checkpoint.summary.as_deref().unwrap_or("")
    );

    let found = todos_in_original
        .iter()
        .filter(|todo| combined.contains(todo.as_str()))
        .count();

    VerificationCheck {
        name: "pending_tasks_presence".to_string(),
        passed: found > 0 || combined.contains("TODO"),
        priority: CheckPriority::P1,
        detail: format!(
            "TODO items: {}/{} found in compressed output",
            found,
            todos_in_original.len()
        ),
    }
}

/// P1: Error information should survive compression.
fn check_error_presence(
    original: &[Message],
    compressed: &[Message],
    checkpoint: &CompressionCheckpoint,
) -> VerificationCheck {
    let error_patterns = [
        "error",
        "Error",
        "ERROR",
        "failed",
        "Failed",
        "panic",
        "Panic",
        "exception",
        "失败",
        "错误",
    ];

    let errors_in_original: Vec<String> = original
        .iter()
        .filter(|m| {
            let text = m.content.as_text().unwrap_or_default();
            error_patterns.iter().any(|p| text.contains(p))
        })
        .filter_map(|m| m.content.as_text())
        .collect();

    if errors_in_original.is_empty() {
        return VerificationCheck {
            name: "error_presence".to_string(),
            passed: true,
            priority: CheckPriority::P1,
            detail: "No errors found in original messages".to_string(),
        };
    }

    let combined = format!(
        "{} {}",
        compressed
            .iter()
            .filter_map(|m| m.content.as_text())
            .collect::<Vec<_>>()
            .join(" "),
        checkpoint.summary.as_deref().unwrap_or("")
    );

    let found = errors_in_original
        .iter()
        .any(|e| combined.contains(e.as_str()));

    VerificationCheck {
        name: "error_presence".to_string(),
        passed: found,
        priority: CheckPriority::P1,
        detail: format!("{} error messages to check", errors_in_original.len()),
    }
}

/// P1: User preferences/constraints should survive compression.
fn check_user_preferences(
    original: &[Message],
    compressed: &[Message],
    checkpoint: &CompressionCheckpoint,
) -> VerificationCheck {
    let pref_patterns = [
        "不要",
        "必须",
        "prefer",
        "important",
        "重要",
        "don't",
        "must",
        "should",
        "always",
        "never",
    ];

    let prefs_in_original: Vec<String> = original
        .iter()
        .filter(|m| m.role == Role::User)
        .filter(|m| {
            let text = m.content.as_text().unwrap_or_default();
            pref_patterns
                .iter()
                .any(|p| text.to_lowercase().contains(&p.to_lowercase()))
        })
        .filter_map(|m| m.content.as_text())
        .collect();

    if prefs_in_original.is_empty() {
        return VerificationCheck {
            name: "user_preference_presence".to_string(),
            passed: true,
            priority: CheckPriority::P1,
            detail: "No preference patterns found".to_string(),
        };
    }

    let combined = format!(
        "{} {}",
        compressed
            .iter()
            .filter_map(|m| m.content.as_text())
            .collect::<Vec<_>>()
            .join(" "),
        checkpoint.summary.as_deref().unwrap_or("")
    );

    let found = prefs_in_original
        .iter()
        .filter(|p| combined.contains(p.as_str()))
        .count();

    VerificationCheck {
        name: "user_preference_presence".to_string(),
        passed: found as f64 / prefs_in_original.len() as f64 >= 0.5,
        priority: CheckPriority::P1,
        detail: format!(
            "Preferences: {}/{} preserved",
            found,
            prefs_in_original.len()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compression::CompressionCheckpoint;
    use echo_core::llm::types::{Message, MessageContent, Role};

    #[test]
    fn test_verifier_all_pass_when_no_issues() {
        let compressed = vec![
            Message::system("system".to_string()),
            Message::user("fix auth".to_string()),
            Message::assistant("done".to_string()),
        ];
        let original = compressed.clone();
        let cp = CompressionCheckpoint::new("SlidingWindow")
            .with_counts(3, 2)
            .with_tokens(500, 200);

        let verification = verify_compression(&compressed, &cp, &original);
        assert!(verification.passed);
    }

    #[test]
    fn test_token_target_check_fails_when_tokens_increase() {
        let msgs = vec![Message::user("hello".to_string())];
        let cp = CompressionCheckpoint::new("test")
            .with_counts(1, 0)
            .with_tokens(100, 200); // token increase = failure

        let verification = verify_compression(&msgs, &cp, &msgs);
        let token_check = verification
            .checks
            .iter()
            .find(|c| c.name == "tokens_not_increased")
            .unwrap();
        assert!(!token_check.passed);
    }

    #[test]
    fn test_file_paths_detected() {
        let paths = extract_file_paths_from_text("Modified src/auth.rs and Cargo.toml");
        assert!(!paths.is_empty());
        assert!(paths.contains(&"src/auth.rs".to_string()));
        assert!(paths.contains(&"Cargo.toml".to_string()));
    }
}
