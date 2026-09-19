//! Secret scanner and redaction — prevent credential leakage.
//!
//! TODO(security, v0.3): Split patterns into high-confidence (AWS/GH/Anthropic key prefixes)
//! vs heuristic/generic (token/password patterns with high false-positive rate).
//! High-confidence matches should block execution; generic matches should warn only.
//! Priority: start with AWS (AKIA prefix) and GitHub (ghp_/github_pat_) as they have
//! near-zero false-positive rates, then expand to other providers.
//!
//! TODO(security, v0.3): "Password in URL" pattern matches harmless examples in docs
//! and code comments. Add boundary checks: exclude lines starting with "//" or "#"
//! (comment lines), and require the match to appear in a configuration context
//! (e.g., inside a connection string or URI assignment).
//!
//! Scans text for secrets (API keys, tokens, passwords, private keys) and
//! either redacts them or warns the user. Used by the output guard pipeline
//! and the /commit flow.

pub use echo_core::utils::retention::{
    SecretMatch, contains_secrets, redact_secrets, scan_secrets,
};

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

    // ── Regression: newly-added patterns (P1-16 / 2.7) ────────────────────

    #[test]
    fn test_huggingface_token() {
        assert!(contains_secrets(
            "HF_TOKEN=hf_abcdefghijklmnopqrstuvwxyz123456789"
        ));
    }

    #[test]
    fn test_google_api_key() {
        assert!(contains_secrets(
            "key=AIzaSyDkLmNpQrStUvWxYz1234567890abcdefg"
        ));
    }

    #[test]
    fn test_gitlab_pat() {
        assert!(contains_secrets(
            "GITLAB_TOKEN=glpat-abcdefghijklmnopqrstuvwx"
        ));
    }

    #[test]
    fn test_stripe_live_key() {
        assert!(contains_secrets(concat!(
            "sk_",
            "live_51AbCdEfGhIjKlMnOpQrStUvWxYz"
        )));
    }

    #[test]
    fn test_npm_token() {
        assert!(contains_secrets(
            "NPM_TOKEN=npm_abcdefghijklmnopqrstuvwxyz123456789"
        ));
    }

    #[test]
    fn test_db_connection_string() {
        assert!(contains_secrets(
            "postgresql://user:secret_password@db.internal:5432/mydb"
        ));
        assert!(contains_secrets("mysql://root:pass123@localhost/mydb"));
    }
}
