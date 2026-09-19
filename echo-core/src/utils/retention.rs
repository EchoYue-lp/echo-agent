//! Durable-content redaction and bounded-retention policy.

use regex::Regex;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::LazyLock;

const TRUNCATED_OBJECT_MARKER: &str = "[TRUNCATED OBJECT]";

static SECRET_PATTERNS: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    [
        // Match the complete body before shorter token patterns can overlap it.
        ("Private Key", r"(?s)-----BEGIN (?:(?:RSA|EC|DSA|OPENSSH|ENCRYPTED) )?PRIVATE KEY-----.*?(?:-----END (?:(?:RSA|EC|DSA|OPENSSH|ENCRYPTED) )?PRIVATE KEY-----|\z)"),
        ("AWS Access Key", r"AKIA[0-9A-Z]{16}"),
        ("AWS Secret Key", r"(?i)aws.?secret.?key[\s:=]+[A-Za-z0-9/+=]{40}"),
        ("GitHub Token", r"gh[pousr]_[A-Za-z0-9_]{36,}"),
        ("GitHub PAT", r"github_pat_[A-Za-z0-9_]{22,}"),
        ("SSH Key", r"ssh-rsa\s+AAAA[A-Za-z0-9+/=]+"),
        ("Anthropic API Key", r"sk-ant-[A-Za-z0-9_-]{20,}"),
        ("OpenAI API Key", r"sk-[A-Za-z0-9_-]{20,}"),
        ("Stripe Live Key", r"sk_live_[0-9a-zA-Z]{24,}"),
        ("npm Token", r"npm_[A-Za-z0-9]{36}"),
        ("JWT Token", r"eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}"),
        ("Slack Token", r"xox[baprs]-[A-Za-z0-9-]{10,}"),
        ("HuggingFace Token", r"hf_[A-Za-z0-9]{34}"),
        ("Google API Key", r"AIza[0-9A-Za-z\-_]{35}"),
        ("GitLab PAT", r"glpat-[A-Za-z0-9\-_]{26}"),
        ("Bearer Token", r"(?i)\bBearer\s+[A-Za-z0-9._~+/-]+={0,2}"),
        ("Assigned Secret", r#"(?i)(api[_-]?key|apikey|token|secret|password|passwd)[\s:=]+["'](?:\\.|[^"'\\\r\n])*"#),
        ("Generic Secret", r"(?i)(api[_-]?key|apikey|token|secret|password|passwd)[\s:=]+[A-Za-z0-9_\-!@#$%^&*+/=]{8,}"),
        ("JSON Secret", r#"(?i)"(?:\\.|[^"\\])*(?:api[_-]?key|apikey|token|secret|password|passwd)(?:\\.|[^"\\])*"\s*:\s*"(?:\\.|[^"\\])*""#),
        ("Truncated JSON Secret", r#"(?i)"(?:\\.|[^"\\])*(?:api[_-]?key|apikey|token|secret|password|passwd)(?:\\.|[^"\\])*"\s*:\s*"(?:\\.|[^"\\])*\z"#),
        ("Password in URL", r"://[^:\s]+:[^@\s]+@"),
        ("DB Connection String", r"(?i)(postgres(ql)?|mysql)://[^@\s]+:[^@\s]+@"),
    ]
    .into_iter()
    .filter_map(|(name, pattern)| Regex::new(pattern).ok().map(|regex| (name, regex)))
    .collect()
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretMatch {
    pub secret_type: &'static str,
    pub matched: String,
    pub position: usize,
}

pub fn scan_secrets(text: &str) -> Vec<SecretMatch> {
    let mut matches = Vec::new();
    for (secret_type, regex) in SECRET_PATTERNS.iter() {
        for found in regex.find_iter(text) {
            let end = found.end();
            if matches.iter().any(|existing: &SecretMatch| {
                let existing_end = existing.position.saturating_add(existing.matched.len());
                found.start() < existing_end && end > existing.position
            }) {
                continue;
            }
            matches.push(SecretMatch {
                secret_type,
                matched: found.as_str().to_string(),
                position: found.start(),
            });
        }
    }
    matches
}

pub fn contains_secrets(text: &str) -> bool {
    SECRET_PATTERNS
        .iter()
        .any(|(_, regex)| regex.is_match(text))
}

pub fn redact_secrets(text: &str) -> String {
    let mut result = text.to_string();
    let mut matches = scan_secrets(text);
    matches.sort_by_key(|item| std::cmp::Reverse(item.position));
    for item in matches {
        let end = item.position.saturating_add(item.matched.len());
        if end <= result.len()
            && result.is_char_boundary(item.position)
            && result.is_char_boundary(end)
        {
            result.replace_range(
                item.position..end,
                &format!("[REDACTED: {}]", item.secret_type),
            );
        }
    }
    result
}

/// Controls what durable diagnostic stores retain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentRetentionPolicy {
    pub max_string_chars: usize,
    pub max_array_items: usize,
}

impl Default for ContentRetentionPolicy {
    fn default() -> Self {
        Self {
            max_string_chars: 16_384,
            max_array_items: 4_096,
        }
    }
}

impl ContentRetentionPolicy {
    /// Recursively redact known secret shapes and bound strings/arrays.
    pub fn sanitize_json(&self, value: &mut Value) {
        match value {
            Value::String(text) => *text = self.sanitize_text(text),
            Value::Array(values) => {
                if values.len() > self.max_array_items {
                    values.truncate(self.max_array_items);
                    values.push(Value::String("[TRUNCATED ARRAY]".to_string()));
                }
                for value in values {
                    self.sanitize_json(value);
                }
            }
            Value::Object(values) => {
                let original = std::mem::take(values);
                let marker_key = self.sanitize_json_key(TRUNCATED_OBJECT_MARKER);
                let marker_collision_key =
                    collision_json_key(TRUNCATED_OBJECT_MARKER, self.max_string_chars);
                let mut had_marker = false;
                let retained = original
                    .into_iter()
                    .filter(|(key, value)| {
                        let is_marker = value.as_str() == Some(TRUNCATED_OBJECT_MARKER)
                            && (key == &marker_key || key == &marker_collision_key);
                        had_marker |= is_marker;
                        !is_marker
                    })
                    .collect::<Vec<_>>();
                let mut truncated = had_marker || retained.len() > self.max_array_items;
                for (key, mut value) in retained.into_iter().take(self.max_array_items) {
                    if is_sensitive_key(&key) {
                        value = Value::String("[REDACTED]".to_string());
                    } else {
                        self.sanitize_json(&mut value);
                    }
                    let sanitized_key = self.sanitize_json_key(&key);
                    if let Some(unique_key) =
                        unique_json_key(values, sanitized_key, &key, self.max_string_chars)
                    {
                        values.insert(unique_key, value);
                    } else {
                        truncated = true;
                    }
                }
                if truncated
                    && let Some(marker_key) = unique_json_key(
                        values,
                        self.sanitize_json_key(TRUNCATED_OBJECT_MARKER),
                        TRUNCATED_OBJECT_MARKER,
                        self.max_string_chars,
                    )
                {
                    values.insert(
                        marker_key,
                        Value::String(TRUNCATED_OBJECT_MARKER.to_string()),
                    );
                }
            }
            Value::Null | Value::Bool(_) | Value::Number(_) => {}
        }
    }

    pub fn sanitize_text(&self, text: &str) -> String {
        // These are bounded synthetic values emitted by this policy. Keeping
        // them stable makes a persisted record safe to read and sanitize again.
        if matches!(
            text,
            "[REDACTED]" | "[TRUNCATED ARRAY]" | TRUNCATED_OBJECT_MARKER
        ) {
            return text.to_string();
        }
        let mut redacted = text.to_string();
        for (_, pattern) in SECRET_PATTERNS.iter() {
            redacted = pattern.replace_all(&redacted, "[REDACTED]").into_owned();
        }
        if redacted.chars().count() <= self.max_string_chars {
            return redacted;
        }
        let mut bounded: String = redacted.chars().take(self.max_string_chars).collect();
        bounded.push_str("...[TRUNCATED]");
        bounded
    }

    fn sanitize_json_key(&self, key: &str) -> String {
        self.sanitize_text(key)
            .chars()
            .take(self.max_string_chars)
            .collect()
    }
}

fn unique_json_key(
    values: &serde_json::Map<String, Value>,
    key: String,
    original_key: &str,
    max_chars: usize,
) -> Option<String> {
    if !values.contains_key(&key) {
        return Some(key);
    }
    if max_chars == 0 {
        return None;
    }
    let candidate = collision_json_key(original_key, max_chars);
    (!values.contains_key(&candidate)).then_some(candidate)
}

fn collision_json_key(original_key: &str, max_chars: usize) -> String {
    format!("{:x}", Sha256::digest(original_key.as_bytes()))
        .chars()
        .take(max_chars)
        .collect()
}

fn is_sensitive_key(key: &str) -> bool {
    let normalized = key.to_ascii_lowercase().replace(['-', '_'], "");
    [
        "authorization",
        "cookie",
        "setcookie",
        "password",
        "passwd",
        "secret",
        "token",
        "apikey",
        "accesstoken",
        "refreshtoken",
        "privatekey",
    ]
    .iter()
    .any(|sensitive| normalized == *sensitive || normalized.ends_with(sensitive))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recursively_redacts_and_truncates_unicode_safely() {
        let policy = ContentRetentionPolicy {
            max_string_chars: 4,
            max_array_items: 4,
        };
        let mut value = serde_json::json!({
            "nested": {"secret": "Bearer abcdefghijklmnopqrstuvwxyz"},
            "long": "中文字符很长很长",
            "many": ["one", "two", "three", "four", "five"]
        });
        policy.sanitize_json(&mut value);
        assert!(!value.to_string().contains("abcdefghijklmnopqrstuvwxyz"));
        assert!(
            value
                .get("long")
                .and_then(Value::as_str)
                .is_some_and(|text| text.ends_with("[TRUNCATED]"))
        );
        assert_eq!(
            value.get("many").and_then(Value::as_array).map(Vec::len),
            Some(5)
        );
        let nested = value.as_object().and_then(|root| {
            root.iter()
                .find(|(key, _)| key.starts_with("nest"))
                .and_then(|(_, value)| value.as_object())
        });
        assert!(nested.is_some_and(|entries| {
            entries.len() == 1
                && entries
                    .values()
                    .any(|value| value.as_str() == Some("[REDACTED]"))
        }));
    }

    #[test]
    fn redacts_structured_and_embedded_json_secret_fields() {
        let policy = ContentRetentionPolicy::default();
        let mut value = serde_json::json!({
            "OPENAI_API_KEY": "short-value",
            "headers": {"Authorization": "custom credential"},
            "ordinary": "visible"
        });
        policy.sanitize_json(&mut value);
        assert_eq!(value["OPENAI_API_KEY"], "[REDACTED]");
        assert_eq!(value["headers"]["Authorization"], "[REDACTED]");
        assert_eq!(value["ordinary"], "visible");

        let text = policy.sanitize_text(r#"{"password":"short","name":"ok"}"#);
        assert!(!text.contains("short"));
    }

    #[test]
    fn embedded_json_redaction_handles_escaped_quotes_and_newlines() {
        let policy = ContentRetentionPolicy::default();
        let text = policy
            .sanitize_text("prefix {\"password\"\n:\n\"alpha\\\"omega\",\"visible\":true} suffix");
        assert!(!text.contains("alpha"));
        assert!(!text.contains("omega"));
        assert!(text.contains("\"visible\":true"));
        assert!(text.contains("prefix"));
        assert!(text.contains("suffix"));
    }

    #[test]
    fn embedded_json_redaction_preserves_escaped_non_secret_fields() {
        let policy = ContentRetentionPolicy::default();
        let text = r#"{"message":"alpha\"omega","visible":true}"#;
        assert_eq!(policy.sanitize_text(text), text);
    }

    #[test]
    fn redacts_secret_json_keys_and_preserves_colliding_entries() {
        let policy = ContentRetentionPolicy {
            max_string_chars: 16,
            max_array_items: 8,
        };
        let token = "npm_abcdefghijklmnopqrstuvwxyz1234567890";
        let mut value = serde_json::json!({
            (token): "first",
            "npm_ABCDEFGHIJKLMNOPQRSTUVWXYZ1234567890": "second",
            "ordinary-key-that-is-too-long": "visible"
        });

        policy.sanitize_json(&mut value);

        let serialized = value.to_string();
        assert!(!serialized.contains("npm_"));
        assert!(!serialized.contains("ordinary-key-that-is-too-long"));
        assert_eq!(value.as_object().map(serde_json::Map::len), Some(3));
        assert!(serialized.contains("[REDACTED]"));
        assert!(
            value
                .as_object()
                .is_some_and(|entries| { entries.keys().all(|key| key.chars().count() <= 16) })
        );
    }

    #[test]
    fn retention_scanner_covers_all_runtime_secret_families() {
        let policy = ContentRetentionPolicy::default();
        assert_eq!(
            SECRET_PATTERNS.len(),
            22,
            "a secret pattern failed to compile"
        );
        let stripe_live_key = format!("{}{}", "sk_", format!("live_{}", "x".repeat(24)));
        for secret in [
            "aws_secret_key=ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmn",
            "ssh-rsa AAAAabcdefghijklmnopqrstuvwxyz1234567890",
            stripe_live_key.as_str(),
            "npm_abcdefghijklmnopqrstuvwxyz1234567890",
            "Bearer opaque-token-1234567890",
        ] {
            assert_eq!(policy.sanitize_text(secret), "[REDACTED]");
        }
    }

    #[test]
    fn private_key_body_is_redacted_in_complete_and_truncated_pem_blocks() {
        let policy = ContentRetentionPolicy::default();
        let complete = "before -----BEGIN PRIVATE KEY-----\nMIIEvQIBADANBgkqhkiG9w0B\n-----END PRIVATE KEY----- after";
        assert_eq!(policy.sanitize_text(complete), "before [REDACTED] after");
        assert!(!redact_secrets(complete).contains("MIIEvQIBADAN"));

        let truncated = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAA";
        assert_eq!(policy.sanitize_text(truncated), "[REDACTED]");
        let matches = scan_secrets(truncated);
        assert!(
            matches
                .iter()
                .any(|item| { item.secret_type == "Private Key" && item.matched == truncated })
        );
    }

    #[test]
    fn synthetic_retention_markers_survive_repeated_zero_limit_passes() {
        let policy = ContentRetentionPolicy {
            max_string_chars: 0,
            max_array_items: 0,
        };
        let mut value = serde_json::json!({
            "password": "supersecret",
            "items": ["one", "two"]
        });
        policy.sanitize_json(&mut value);
        let once = value.clone();
        policy.sanitize_json(&mut value);
        assert_eq!(value, once);
        let serialized = value.to_string();
        assert!(serialized.contains("[TRUNCATED OBJECT]"));
        assert!(!serialized.contains("supersecret"));
    }

    #[test]
    fn object_retention_is_bounded_and_collision_keys_remain_bounded() {
        let policy = ContentRetentionPolicy {
            max_string_chars: 4,
            max_array_items: 32,
        };
        let mut object = serde_json::Map::new();
        for index in 0..10_000usize {
            object.insert(
                format!("same-prefix-{index}"),
                Value::String(format!("value-{index}")),
            );
        }
        let mut value = Value::Object(object);
        policy.sanitize_json(&mut value);
        let retained = value.as_object();
        assert!(retained.is_some_and(|entries| entries.len() <= 33));
        assert!(
            retained.is_some_and(|entries| { entries.keys().all(|key| key.chars().count() <= 4) })
        );
        let serialized = value.to_string();
        assert!(!serialized.contains("value-9999"));
        assert!(serialized.contains("[TRUNCATED OBJECT]"));
    }

    #[test]
    fn object_retention_is_idempotent_with_a_nonzero_limit_and_marker_collision() {
        let policy = ContentRetentionPolicy {
            max_string_chars: 32,
            max_array_items: 2,
        };
        let mut value = serde_json::json!({
            "[TRUNCATED OBJECT]": "user value",
            "alpha": 1,
            "beta": 2,
            "gamma": 3
        });
        policy.sanitize_json(&mut value);
        let once = value.clone();
        policy.sanitize_json(&mut value);

        assert_eq!(value, once);
        assert!(value.as_object().is_some_and(|entries| entries.len() == 3));
        assert_eq!(
            value.as_object().map(|entries| {
                entries
                    .values()
                    .filter(|value| value.as_str() == Some(TRUNCATED_OBJECT_MARKER))
                    .count()
            }),
            Some(1)
        );
    }

    #[test]
    fn ordinary_marker_text_value_is_preserved() {
        let policy = ContentRetentionPolicy {
            max_string_chars: 32,
            max_array_items: 4,
        };
        let mut value = serde_json::json!({
            "status": TRUNCATED_OBJECT_MARKER,
            "other": "visible"
        });
        let original = value.clone();

        policy.sanitize_json(&mut value);
        assert_eq!(value, original);
        policy.sanitize_json(&mut value);
        assert_eq!(value, original);
    }

    #[test]
    fn sensitive_quoted_values_fail_closed_when_truncated() {
        let policy = ContentRetentionPolicy::default();
        for text in [
            r#"password="supersecret"#,
            r#"{"password":"alpha\"omega"#,
            "token: 'tiny",
        ] {
            let sanitized = policy.sanitize_text(text);
            assert!(!sanitized.contains("supersecret"));
            assert!(!sanitized.contains("alpha"));
            assert!(!sanitized.contains("omega"));
            assert!(!sanitized.contains("tiny"));
            assert!(sanitized.contains("[REDACTED]"));
        }
    }
}
