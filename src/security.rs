//! Secret scanner and redaction — prevent credential leakage.
//!
//! TODO(security): Split patterns into high-confidence (AWS/GH/Anthropic key prefixes)
//! vs heuristic/generic (token/password patterns with high false-positive rate).
//! High-confidence matches should block; generic matches should warn only.
//!
//! TODO(security): "Password in URL" pattern matches harmless examples in docs.
//! Add boundary checks or exclude lines starting with "//" or "#" (comment lines).
//!
//! Scans text for secrets (API keys, tokens, passwords, private keys) and
//! either redacts them or warns the user. Used by the output guard pipeline
//! and the /commit flow.

use regex::Regex;
use std::sync::LazyLock;

/// A detected secret with its type and location.
#[derive(Debug, Clone)]
pub struct SecretMatch {
    /// Type of secret (e.g. "AWS Access Key", "GitHub Token").
    pub secret_type: &'static str,
    /// The matched text (for redaction).
    pub matched: String,
    /// Start position in the original text.
    pub position: usize,
}

/// Pre-compiled secret patterns.
static SECRET_PATTERNS: LazyLock<Vec<(&str, Regex)>> = LazyLock::new(|| {
    vec![
        ("AWS Access Key", Regex::new(r"AKIA[0-9A-Z]{16}").unwrap()),
        (
            "AWS Secret Key",
            Regex::new(r"(?i)aws.?secret.?key[\s:=]+[A-Za-z0-9/+=]{40}").unwrap(),
        ),
        (
            "GitHub Token",
            Regex::new(r"gh[pousr]_[A-Za-z0-9_]{36,}").unwrap(),
        ),
        (
            "GitHub PAT",
            Regex::new(r"github_pat_[A-Za-z0-9_]{22,}").unwrap(),
        ),
        (
            "Anthropic API Key",
            Regex::new(r"sk-ant-[A-Za-z0-9_-]{20,}").unwrap(),
        ),
        (
            "OpenAI API Key",
            Regex::new(r"sk-[A-Za-z0-9_-]{20,}").unwrap(),
        ),
        (
            "Generic API Key",
            Regex::new(r"(?i)(api[_-]?key|apikey)[\s:=]+[A-Za-z0-9_\-]{20,}").unwrap(),
        ),
        (
            "Private Key",
            Regex::new(r"-----BEGIN (RSA |EC |DSA |OPENSSH )?PRIVATE KEY-----").unwrap(),
        ),
        (
            "JWT Token",
            Regex::new(r"eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}").unwrap(),
        ),
        ("Password in URL", Regex::new(r"://[^:]+:[^@]+@").unwrap()),
        (
            "Slack Token",
            Regex::new(r"xox[baprs]-[A-Za-z0-9-]{10,}").unwrap(),
        ),
        (
            "Generic Token",
            Regex::new(r"(?i)(token|secret|password|passwd)[\s:=]+[A-Za-z0-9_\-!@#$%^&*]{8,}")
                .unwrap(),
        ),
    ]
});

/// Scan text for secrets. Returns all matches found.
/// Overlapping matches are deduplicated: when two patterns match the same text range,
/// the earlier (more specific) pattern in SECRET_PATTERNS wins.
pub fn scan_secrets(text: &str) -> Vec<SecretMatch> {
    let mut matches = Vec::new();
    for (secret_type, regex) in SECRET_PATTERNS.iter() {
        for m in regex.find_iter(text) {
            let end = m.end();
            // Skip if this range overlaps with an already-matched (higher-priority) result
            if matches.iter().any(|existing: &SecretMatch| {
                let existing_end = existing.position + existing.matched.len();
                m.start() < existing_end && end > existing.position
            }) {
                continue;
            }
            matches.push(SecretMatch {
                secret_type,
                matched: m.as_str().to_string(),
                position: m.start(),
            });
        }
    }
    matches
}

/// Redact secrets in text by replacing them with `[REDACTED: <type>]`.
pub fn redact_secrets(text: &str) -> String {
    let mut result = text.to_string();
    let mut matches = scan_secrets(text);
    // Process matches in reverse order to preserve positions
    matches.sort_by_key(|m| std::cmp::Reverse(m.position));
    for m in &matches {
        let replacement = format!("[REDACTED: {}]", m.secret_type);
        let end = m.position + m.matched.len();
        if end <= result.len() {
            result.replace_range(m.position..end, &replacement);
        }
    }
    result
}

/// Check if text contains any secrets. Faster than `scan_secrets` —
/// stops at the first match.
pub fn contains_secrets(text: &str) -> bool {
    SECRET_PATTERNS
        .iter()
        .any(|(_, regex)| regex.is_match(text))
}

/// Scan and return a summary suitable for CLI display.
pub fn scan_summary(text: &str) -> Vec<String> {
    let matches = scan_secrets(text);
    if matches.is_empty() {
        return vec!["No secrets detected.".into()];
    }
    let mut by_type: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for m in &matches {
        *by_type.entry(m.secret_type).or_default() += 1;
    }
    let mut summary: Vec<String> = vec![format!("Found {} potential secret(s):", matches.len())];
    for (st, count) in &by_type {
        summary.push(format!("  {} x {}", count, st));
    }
    summary.push("Consider using environment variables or a secrets manager.".into());
    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scan_openai_key() {
        let text = "export OPENAI_API_KEY=sk-proj12345678901234567890abcdef";
        let matches = scan_secrets(text);
        assert!(matches.iter().any(|m| m.secret_type == "OpenAI API Key"));
    }

    #[test]
    fn test_redact_secrets() {
        let text = "token: sk-ant-api03-abc123def456ghi789";
        let redacted = redact_secrets(text);
        assert!(redacted.contains("[REDACTED:"));
        assert!(!redacted.contains("sk-ant-api03"));
    }

    #[test]
    fn test_no_secrets() {
        let text = "This is normal code without any secrets.";
        assert!(!contains_secrets(text));
        assert!(scan_secrets(text).is_empty());
    }

    #[test]
    fn test_github_token() {
        let text = "GITHUB_TOKEN=ghp_1234567890abcdefghijklmnopqrstuvwxyz123456";
        let matches = scan_secrets(text);
        assert!(matches.iter().any(|m| m.secret_type == "GitHub Token"));
    }

    #[test]
    fn test_private_key() {
        let text =
            "-----BEGIN PRIVATE KEY-----\nMIIEvQIBADANBgkqhkiG9w0B\n-----END PRIVATE KEY-----";
        assert!(contains_secrets(text));
    }
}
