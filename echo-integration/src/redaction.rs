//! Redaction shared by integration configuration Debug output and diagnostics.
//!
//! Integration configs contain credentials by design.  Following the existing
//! `LlmConfig` Debug contract, this module keeps the secret-bearing values out
//! of derived Debug output and applies the framework retention policy to raw
//! transport errors before they become logs or returned error strings.

use echo_core::utils::retention::ContentRetentionPolicy;
use serde_json::Value;
use std::collections::HashMap;

pub(crate) const REDACTED: &str = "[REDACTED]";

/// Redact credential-shaped text before it is logged or embedded in an error.
pub(crate) fn text(value: &str) -> String {
    ContentRetentionPolicy::default().sanitize_text(value)
}

/// Remove exact credentials owned by the current config before applying
/// shape-based redaction and bounded retention.  This order also covers a
/// credential that crosses the retention boundary.
pub(crate) fn text_with_secrets<S>(value: &str, secrets: impl IntoIterator<Item = S>) -> String
where
    S: AsRef<str>,
{
    let mut patterns = secrets
        .into_iter()
        .map(|secret| secret.as_ref().chars().collect::<Vec<_>>())
        .filter(|secret| !secret.is_empty())
        .collect::<Vec<_>>();
    patterns.sort_by(|left, right| right.len().cmp(&left.len()).then_with(|| left.cmp(right)));
    patterns.dedup();

    let source = value.chars().collect::<Vec<_>>();
    let mut sanitized = String::with_capacity(value.len());
    let mut position = 0usize;
    while position < source.len() {
        let matched = patterns.iter().find(|pattern| {
            pattern.iter().enumerate().all(|(offset, expected)| {
                position
                    .checked_add(offset)
                    .and_then(|index| source.get(index))
                    == Some(expected)
            })
        });
        if let Some(pattern) = matched {
            sanitized.push_str(REDACTED);
            position = position.saturating_add(pattern.len());
        } else if let Some(character) = source.get(position) {
            sanitized.push(*character);
            position = position.saturating_add(1);
        } else {
            break;
        }
    }
    text(&sanitized)
}

/// Redact nested credential fields before diagnostic formatting.
#[cfg(any(feature = "channels", test))]
pub(crate) fn json(value: &Value) -> Value {
    let mut sanitized = value.clone();
    ContentRetentionPolicy::default().sanitize_json(&mut sanitized);
    sanitized
}

/// Redact structured diagnostics plus exact values configured for the current
/// extension instance.
pub(crate) fn json_with_secrets(value: &Value, secrets: &[String]) -> Value {
    fn redact_strings(value: &mut Value, secrets: &[String]) {
        match value {
            Value::String(text) => {
                *text = text_with_secrets(text, secrets.iter());
            }
            Value::Array(values) => {
                for value in values {
                    redact_strings(value, secrets);
                }
            }
            Value::Object(values) => {
                for value in values.values_mut() {
                    redact_strings(value, secrets);
                }
            }
            Value::Null | Value::Bool(_) | Value::Number(_) => {}
        }
    }

    let mut sanitized = value.clone();
    redact_strings(&mut sanitized, secrets);
    ContentRetentionPolicy::default().sanitize_json(&mut sanitized);
    sanitized
}

/// Remove user information and query/fragment values from a URL used in a
/// diagnostic.  Transport requests continue to use the original URL.
pub(crate) fn url(value: &str) -> String {
    let Ok(mut parsed) = url::Url::parse(value) else {
        return "[REDACTED URL]".to_string();
    };
    if !parsed.username().is_empty() {
        let _ = parsed.set_username(REDACTED);
    }
    if parsed.password().is_some() {
        let _ = parsed.set_password(Some(REDACTED));
    }
    if parsed.query().is_some() {
        let keys = parsed
            .query_pairs()
            .map(|(key, _)| key.into_owned())
            .collect::<Vec<_>>();
        parsed.set_query(None);
        {
            let mut pairs = parsed.query_pairs_mut();
            for key in keys {
                pairs.append_pair(&key, REDACTED);
            }
        }
    }
    if parsed.fragment().is_some() {
        parsed.set_fragment(Some(REDACTED));
    }
    parsed.to_string()
}

/// Format a reqwest error without allowing its attached request URL to bypass
/// URL credential redaction.
pub(crate) fn request_error<S>(
    error: reqwest::Error,
    secrets: impl IntoIterator<Item = S>,
) -> String
where
    S: AsRef<str>,
{
    let secrets = secrets
        .into_iter()
        .map(|secret| secret.as_ref().to_string())
        .collect::<Vec<_>>();
    let attached_url = error.url().map(|value| url(value.as_str()));
    let mut diagnostic = error.without_url().to_string();
    if let Some(attached_url) = attached_url {
        diagnostic.push_str(" for url (");
        diagnostic.push_str(&attached_url);
        diagnostic.push(')');
    }
    text_with_secrets(&diagnostic, secrets.iter())
}

/// Preserve whether an optional secret was configured without exposing it.
#[cfg(any(feature = "channels", test))]
pub(crate) fn optional<T>(value: Option<&T>) -> Option<&'static str> {
    value.map(|_| REDACTED)
}

/// Preserve whether a secret-bearing collection was configured without
/// exposing either its keys or values.
pub(crate) fn collection_is_redacted(is_empty: bool) -> &'static str {
    if is_empty { "[]" } else { REDACTED }
}

/// Return exact values that must be removed from HTTP diagnostics.  For an
/// Authorization header, include both the full wire value and the credential
/// after its scheme because servers commonly echo only the payload.
pub(crate) fn header_secrets(headers: &HashMap<String, String>) -> Vec<String> {
    let mut secrets = Vec::new();
    for (name, value) in headers {
        if !value.is_empty() {
            secrets.push(value.clone());
        }
        if name.eq_ignore_ascii_case("authorization")
            && let Some((_, credential)) = value.split_once(char::is_whitespace)
        {
            let credential = credential.trim();
            if !credential.is_empty() {
                secrets.push(credential.to_string());
            }
        }
    }
    secrets
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_extension_credential_assignments() {
        let secrets = [
            "abc123",
            "client-value",
            "app-value",
            "verify-value",
            "sign-value",
        ];
        let sanitized = text_with_secrets(
            "Authorization: Bearer abc123, client_secret=client-value, \
             app_secret=app-value, verification_token=verify-value, \
             signing_key=sign-value",
            secrets,
        );
        for secret in secrets {
            assert!(!sanitized.contains(secret), "secret survived: {sanitized}");
        }
        assert_eq!(url("://host?ticket=invalid-url-secret"), "[REDACTED URL]");
    }

    #[test]
    fn optional_and_collection_redaction_keep_shape_only() {
        assert_eq!(optional(Some(&"secret")), Some(REDACTED));
        assert_eq!(optional::<&str>(None), None);
        assert_eq!(collection_is_redacted(true), "[]");
        assert_eq!(collection_is_redacted(false), REDACTED);
    }

    #[test]
    fn authorization_secret_set_includes_scheme_and_payload() {
        let headers = HashMap::from([(
            "authorization".to_string(),
            "Bearer opaque-credential".to_string(),
        )]);
        let secrets = header_secrets(&headers);
        let diagnostic = text_with_secrets("credential opaque-credential rejected", secrets.iter());
        assert!(!diagnostic.contains("opaque-credential"));
        assert!(
            secrets
                .iter()
                .any(|secret| secret == "Bearer opaque-credential")
        );
        assert!(secrets.iter().any(|secret| secret == "opaque-credential"));
    }

    #[test]
    fn configured_values_are_redacted_without_field_names() {
        let sanitized = text_with_secrets(
            "server echoed opaque-value and another-value",
            ["opaque-value", "another-value"],
        );
        assert!(!sanitized.contains("opaque-value"));
        assert!(!sanitized.contains("another-value"));
        assert_eq!(text_with_secrets("unchanged", [""]), "unchanged");

        let overlapping = text_with_secrets(
            "opaque-credential and RED",
            ["opaque", "opaque-credential", "RED"],
        );
        assert_eq!(overlapping, "[REDACTED] and [REDACTED]");

        let crossing_secret = "opaque-crossing-secret";
        let input = format!("{}{}", "x".repeat(16_380), crossing_secret);
        let sanitized = text_with_secrets(&input, [crossing_secret]);
        assert!(!sanitized.contains("opaque"));
    }

    #[test]
    fn redacts_nested_json_and_url_credentials() {
        let value = serde_json::json!({
            "headers": {"Authorization": "Bearer header-secret"},
            "env": {"TOKEN": "env-secret"}
        });
        let sanitized = json(&value).to_string();
        assert!(!sanitized.contains("header-secret"));
        assert!(!sanitized.contains("env-secret"));

        let exact = json_with_secrets(
            &serde_json::json!({"message": "server echoed opaque-value"}),
            &["opaque-value".to_string()],
        );
        assert!(!exact.to_string().contains("opaque-value"));

        let crossing_secret = "opaque-crossing-secret";
        let exact = json_with_secrets(
            &serde_json::json!({
                "message": format!("{}{}", "x".repeat(16_380), crossing_secret)
            }),
            &[crossing_secret.to_string()],
        );
        let exact = exact.to_string();
        assert!(!exact.contains("opaque"));

        let sanitized_url = url(
            "https://client:password@example.invalid/mcp?session_id=session-secret&ticket=ticket-secret#fragment-secret",
        );
        for secret in [
            "client",
            "password",
            "session-secret",
            "ticket-secret",
            "fragment-secret",
        ] {
            assert!(
                !sanitized_url.contains(secret),
                "URL secret survived: {sanitized_url}"
            );
        }
    }

    #[tokio::test]
    async fn reqwest_error_redacts_attached_url() -> Result<(), Box<dyn std::error::Error>> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await?;
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request).await?;
            stream
                .write_all(b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\n\r\n")
                .await?;
            Ok::<(), std::io::Error>(())
        });
        let error = reqwest::Client::new()
            .get(format!(
                "http://{address}/mcp?ticket=opaque-ticket&session=opaque-session"
            ))
            .send()
            .await?
            .error_for_status()
            .err()
            .ok_or("expected HTTP 500 to fail")?;
        server.await??;
        let diagnostic = request_error(error, std::iter::empty::<&str>());
        assert!(!diagnostic.contains("opaque-ticket"));
        assert!(!diagnostic.contains("opaque-session"));
        assert!(diagnostic.contains("ticket="));
        Ok(())
    }
}
