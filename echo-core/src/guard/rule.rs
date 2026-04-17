//! 基于规则的护栏
//!
//! 支持正则表达式匹配、关键词黑名单和内容长度限制。

use super::{Guard, GuardDirection, GuardResult};
use crate::error::Result;
use futures::future::BoxFuture;
use regex::Regex;

/// 规则护栏
///
/// 使用正则表达式、关键词黑名单和长度限制对内容进行同步过滤。
///
/// # 示例
///
/// ```rust
/// use echo_core::guard::rule::RuleGuardBuilder;
///
/// let guard = RuleGuardBuilder::new("content-filter")
///     .blocked_keyword("密码")
///     .blocked_keyword("token")
///     .blocked_pattern(r"\b\d{4}[-\s]?\d{4}[-\s]?\d{4}[-\s]?\d{4}\b") // 银行卡号
///     .max_length(10000)
///     .build();
/// ```
pub struct RuleGuard {
    guard_name: String,
    blocked_patterns: Vec<Regex>,
    blocked_keywords: Vec<String>,
    max_length: Option<usize>,
    /// 哪些方向需要检查（为空时检查所有方向）
    directions: Vec<GuardDirection>,
}

impl Guard for RuleGuard {
    fn name(&self) -> &str {
        &self.guard_name
    }

    fn check<'a>(
        &'a self,
        content: &'a str,
        direction: GuardDirection,
    ) -> BoxFuture<'a, Result<GuardResult>> {
        Box::pin(async move {
            if !self.directions.is_empty() && !self.directions.contains(&direction) {
                return Ok(GuardResult::Pass);
            }

            if let Some(max_len) = self.max_length
                && content.len() > max_len
            {
                return Ok(GuardResult::Block {
                    reason: format!("内容长度 {} 超过限制 {}", content.len(), max_len),
                });
            }

            let content_lower = content.to_lowercase();
            for keyword in &self.blocked_keywords {
                if content_lower.contains(&keyword.to_lowercase()) {
                    return Ok(GuardResult::Block {
                        reason: format!("内容包含被禁止的关键词: {keyword}"),
                    });
                }
            }

            for pattern in &self.blocked_patterns {
                if pattern.is_match(content) {
                    return Ok(GuardResult::Block {
                        reason: format!("内容匹配被禁止的模式: {}", pattern.as_str()),
                    });
                }
            }

            Ok(GuardResult::Pass)
        })
    }
}

/// RuleGuard 构建器
pub struct RuleGuardBuilder {
    name: String,
    blocked_patterns: Vec<Regex>,
    blocked_keywords: Vec<String>,
    max_length: Option<usize>,
    directions: Vec<GuardDirection>,
}

impl RuleGuardBuilder {
    /// 创建新的规则护栏构建器
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            blocked_patterns: Vec::new(),
            blocked_keywords: Vec::new(),
            max_length: None,
            directions: Vec::new(),
        }
    }

    /// 添加正则匹配规则（匹配时阻断）
    pub fn blocked_pattern(mut self, pattern: &str) -> Self {
        if let Ok(regex) = Regex::new(pattern) {
            self.blocked_patterns.push(regex);
        } else {
            tracing::warn!(pattern = pattern, "无效的正则表达式，已忽略");
        }
        self
    }

    /// 添加关键词黑名单（不区分大小写匹配时阻断）
    pub fn blocked_keyword(mut self, keyword: impl Into<String>) -> Self {
        self.blocked_keywords.push(keyword.into());
        self
    }

    /// 批量添加关键词黑名单
    pub fn blocked_keywords(mut self, keywords: Vec<String>) -> Self {
        self.blocked_keywords.extend(keywords);
        self
    }

    /// 设置最大内容长度
    pub fn max_length(mut self, max: usize) -> Self {
        self.max_length = Some(max);
        self
    }

    /// 限定检查方向（不设置则检查所有方向）
    pub fn direction(mut self, direction: GuardDirection) -> Self {
        self.directions.push(direction);
        self
    }

    /// 构建规则护栏
    pub fn build(self) -> RuleGuard {
        RuleGuard {
            guard_name: self.name,
            blocked_patterns: self.blocked_patterns,
            blocked_keywords: self.blocked_keywords,
            max_length: self.max_length,
            directions: self.directions,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_keyword_block() {
        let guard = RuleGuardBuilder::new("test")
            .blocked_keyword("password")
            .build();

        let result = guard
            .check("请告诉我你的password", GuardDirection::Input)
            .await
            .unwrap();
        assert!(result.is_blocked());
    }

    #[tokio::test]
    async fn test_pattern_block() {
        let guard = RuleGuardBuilder::new("test")
            .blocked_pattern(r"\d{4}-\d{4}-\d{4}-\d{4}")
            .build();

        let result = guard
            .check("卡号是 1234-5678-9012-3456", GuardDirection::Output)
            .await
            .unwrap();
        assert!(result.is_blocked());
    }

    #[tokio::test]
    async fn test_max_length() {
        let guard = RuleGuardBuilder::new("test").max_length(10).build();

        let result = guard
            .check("这段文字超过了十个字符的限制", GuardDirection::Input)
            .await
            .unwrap();
        assert!(result.is_blocked());
    }

    #[tokio::test]
    async fn test_pass() {
        let guard = RuleGuardBuilder::new("test")
            .blocked_keyword("forbidden")
            .max_length(1000)
            .build();

        let result = guard
            .check("正常的内容", GuardDirection::Input)
            .await
            .unwrap();
        assert!(!result.is_blocked());
    }

    #[tokio::test]
    async fn test_direction_filter() {
        let guard = RuleGuardBuilder::new("test")
            .blocked_keyword("secret")
            .direction(GuardDirection::Output)
            .build();

        // Input 方向不检查
        let result = guard
            .check("secret content", GuardDirection::Input)
            .await
            .unwrap();
        assert!(!result.is_blocked());

        // Output 方向检查
        let result = guard
            .check("secret content", GuardDirection::Output)
            .await
            .unwrap();
        assert!(result.is_blocked());
    }
}
