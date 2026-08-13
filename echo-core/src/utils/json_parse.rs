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
/// - Fix trailing commas before `}` or `]`, but only outside quoted strings.
///
/// Single-quoted pseudo-JSON is intentionally not rewritten: a lexical global
/// replacement cannot distinguish delimiters from apostrophes in content.
pub fn clean_json(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut cleaned = String::with_capacity(s.len());
    let mut in_string = false;
    let mut escaped = false;
    let mut index = 0usize;
    while let Some(ch) = chars.get(index).copied() {
        if in_string {
            cleaned.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            index = index.saturating_add(1);
            continue;
        }
        if ch == '"' {
            in_string = true;
            cleaned.push(ch);
            index = index.saturating_add(1);
            continue;
        }
        if ch == ',' {
            let mut lookahead = index.saturating_add(1);
            while chars
                .get(lookahead)
                .is_some_and(|next| next.is_whitespace())
            {
                lookahead = lookahead.saturating_add(1);
            }
            if matches!(chars.get(lookahead), Some('}' | ']')) {
                index = index.saturating_add(1);
                continue;
            }
        }
        cleaned.push(ch);
        index = index.saturating_add(1);
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
    fn test_clean_does_not_rewrite_single_quotes() {
        assert_eq!(clean_json("{'a': 'don\'t'}"), "{'a': 'don\'t'}");
    }

    #[test]
    fn test_clean_preserves_double_quotes() {
        let input = r#"{"a": "it's fine"}"#;
        assert_eq!(clean_json(input), input);
    }

    #[test]
    fn test_clean_preserves_structural_text_inside_strings() {
        let input = r#"{"text":"keep ,} and ,] literal","escaped":"\\\" ,}",}"#;
        assert_eq!(
            clean_json(input),
            r#"{"text":"keep ,} and ,] literal","escaped":"\\\" ,}"}"#
        );
    }

    #[test]
    fn test_clean_trailing_comma_with_unicode_whitespace() {
        assert_eq!(clean_json("{\"中文\": 1, \n}"), "{\"中文\": 1 \n}");
    }
}
