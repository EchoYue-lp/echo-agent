//! JSON 解析工具函数
//!
//! 提供从 LLM 输出中提取和修复 JSON 的公共函数，
//! 消除 `LlmPlanner` 和 `LlmCritic` 中的重复代码。

/// 从 markdown code block 或裸文本中提取 JSON
///
/// 支持以下格式：
/// - ````json ... ````
/// - ```` ... ````
/// - 裸 JSON 文本
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

/// 清理常见 JSON 格式问题
///
/// - 修复尾部逗号（`,}` → `}`，`,]` → `]`）
/// - 修复单引号 → 双引号（当不含双引号时）
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
