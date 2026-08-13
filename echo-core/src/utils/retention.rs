//! Durable-content redaction and bounded-retention policy.

use regex::Regex;
use serde_json::Value;
use std::sync::LazyLock;

static SECRET_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"AKIA[0-9A-Z]{16}",
        r"gh[pousr]_[A-Za-z0-9_]{36,}",
        r"github_pat_[A-Za-z0-9_]{22,}",
        r"sk-ant-[A-Za-z0-9_-]{20,}",
        r"sk-[A-Za-z0-9_-]{20,}",
        r"eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}",
        r"xox[baprs]-[A-Za-z0-9-]{10,}",
        r"hf_[A-Za-z0-9]{34}",
        r"AIza[0-9A-Za-z\-_]{35}",
        r"glpat-[A-Za-z0-9\-_]{26}",
        r"(?i)Bearer\s+[A-Za-z0-9\-._~+/]++=*+",
        r"(?i)(api[_-]?key|apikey|token|secret|password|passwd)[\s:=]+[A-Za-z0-9_\-!@#$%^&*+/=]{8,}",
        r"(?i)(postgres(ql)?|mysql)://[^@\s]+:[^@\s]+@",
        r"-----BEGIN (RSA |EC |DSA |OPENSSH )?PRIVATE KEY-----",
    ]
    .into_iter()
    .filter_map(|pattern| Regex::new(pattern).ok())
    .collect()
});

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
                for value in values.values_mut() {
                    self.sanitize_json(value);
                }
            }
            Value::Null | Value::Bool(_) | Value::Number(_) => {}
        }
    }

    pub fn sanitize_text(&self, text: &str) -> String {
        let mut redacted = text.to_string();
        for pattern in SECRET_PATTERNS.iter() {
            redacted = pattern.replace_all(&redacted, "[REDACTED]").into_owned();
        }
        if redacted.chars().count() <= self.max_string_chars {
            return redacted;
        }
        let mut bounded: String = redacted.chars().take(self.max_string_chars).collect();
        bounded.push_str("...[TRUNCATED]");
        bounded
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recursively_redacts_and_truncates_unicode_safely() {
        let policy = ContentRetentionPolicy {
            max_string_chars: 4,
            max_array_items: 1,
        };
        let mut value = serde_json::json!({
            "nested": {"secret": "Bearer abcdefghijklmnopqrstuvwxyz"},
            "long": "中文字符很长很长",
            "many": ["one", "two"]
        });
        policy.sanitize_json(&mut value);
        assert!(!value.to_string().contains("abcdefghijklmnopqrstuvwxyz"));
        assert!(
            value["long"]
                .as_str()
                .is_some_and(|text| text.ends_with("[TRUNCATED]"))
        );
        assert_eq!(value["many"].as_array().map(Vec::len), Some(2));
    }
}
