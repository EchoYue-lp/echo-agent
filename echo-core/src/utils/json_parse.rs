//! JSON parsing utility functions
//!
//! Provides common functions for extracting and fixing JSON from LLM output.

/// Extract JSON from markdown code blocks or bare text
///
/// Supports the following formats:
/// - ````json ... ````
/// - ```` ... ````
/// - Bare JSON text
pub fn extract_json_from_markdown(content: &str) -> String {
    if let Some(start) = content.find("```json") {
        let rest = &content[start + 7..];
        if let Some(end) = rest.find("```") {
            return rest[..end].trim().to_string();
        }
    }
    if let Some(start) = content.find("```") {
        let rest = &content[start + 3..];
        if let Some(end) = rest.find("```") {
            return rest[..end].trim().to_string();
        }
    }
    content.trim().to_string()
}

/// Clean common JSON formatting issues
///
/// - Fix trailing commas (`,}` → `}`, `,]` → `]`)
/// - Fix single quotes → double quotes (when no double quotes are present)
pub fn clean_json(s: &str) -> String {
    let mut cleaned = s.to_string();
    cleaned = cleaned.replace(",}", "}").replace(",]", "]");
    if cleaned.contains('\'') && !cleaned.contains('"') {
        cleaned = cleaned.replace('\'', "\"");
    }
    cleaned
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_from_json_block() {
        let input = "Here is the result:\n```json\n{\"key\": \"value\"}\n```\nDone.";
        assert_eq!(extract_json_from_markdown(input), "{\"key\": \"value\"}");
    }

    #[test]
    fn test_extract_from_plain_block() {
        let input = "```\n{\"key\": \"value\"}\n```";
        assert_eq!(extract_json_from_markdown(input), "{\"key\": \"value\"}");
    }

    #[test]
    fn test_extract_bare_json() {
        let input = r#"{"steps": [{"description": "hello"}]}"#;
        assert_eq!(extract_json_from_markdown(input), input.trim());
    }

    #[test]
    fn test_clean_trailing_commas() {
        assert_eq!(clean_json(r#"{"a": 1,}"#), r#"{"a": 1}"#);
        assert_eq!(clean_json(r#"[1, 2,]"#), r#"[1, 2]"#);
    }

    #[test]
    fn test_clean_single_quotes() {
        assert_eq!(clean_json("{'a': 1}"), "{\"a\": 1}");
    }

    #[test]
    fn test_clean_preserves_double_quotes() {
        let input = r#"{"a": "it's fine"}"#;
        assert_eq!(clean_json(input), input);
    }
}
