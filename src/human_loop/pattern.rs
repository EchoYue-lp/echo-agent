//! 统一的工具名模式匹配
//!
//! 提供单一的模式匹配函数，供 `ApprovalRule`、`RuleClassifier` 等共用，
//! 避免三处重复实现。

/// 检查工具名是否匹配指定的模式
///
/// 支持的匹配方式：
/// - `"*"` — 匹配所有工具
/// - `"Bash"` — 精确匹配 `Bash`，前缀匹配 `Bash(rm:*)`
/// - `"Bash(rm:*)"` — 前缀匹配 `Bash(rm:rf)` 等
pub fn matches_tool_pattern(pattern: &str, tool_name: &str) -> bool {
    // 通配符
    if pattern == "*" {
        return true;
    }
    // 精确匹配
    if tool_name == pattern {
        return true;
    }
    // 通配符后缀: "Bash(rm:*)" 匹配 "Bash(rm:rf)"
    if let Some(prefix) = pattern.strip_suffix("*)")
        && tool_name.starts_with(prefix)
    {
        return true;
    }
    // 工具前缀: "Bash" 匹配 "Bash(rm:*)"
    if tool_name.starts_with(pattern)
        && tool_name.len() > pattern.len()
        && tool_name.as_bytes()[pattern.len()] == b'('
    {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wildcard() {
        assert!(matches_tool_pattern("*", "Bash"));
        assert!(matches_tool_pattern("*", "Read"));
        assert!(matches_tool_pattern("*", "Anything"));
    }

    #[test]
    fn test_exact_match() {
        assert!(matches_tool_pattern("Bash", "Bash"));
        assert!(!matches_tool_pattern("Bash", "Read"));
        assert!(!matches_tool_pattern("Bash", "BashExtra"));
    }

    #[test]
    fn test_prefix_with_paren() {
        assert!(matches_tool_pattern("Bash", "Bash(rm:*)"));
        assert!(matches_tool_pattern("Bash", "Bash(git:status)"));
        assert!(!matches_tool_pattern("Bash", "BashExtra"));
    }

    #[test]
    fn test_wildcard_suffix() {
        assert!(matches_tool_pattern("Bash(rm:*)", "Bash(rm:rf)"));
        assert!(matches_tool_pattern("Bash(rm:*)", "Bash(rm:*)"));
        assert!(!matches_tool_pattern("Bash(rm:*)", "Bash(git:status)"));
    }
}
