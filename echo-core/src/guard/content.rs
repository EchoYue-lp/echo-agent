//! 内容安全护栏 —— PII 检测、敏感信息过滤
//!
//! 提供内容级安全检查，检测并处理：
//! - 中国手机号码
//! - 中国身份证号（18位）
//! - 邮箱地址
//! - API Key（OpenAI / GitHub / Anthropic 格式）
//! - 信用卡号（Visa / MasterCard）
//!
//! # 三种处理模式
//! - **Detect**: 只检测，记录日志，不修改内容
//! - **Redact**: 自动替换为 `[REDACTED]`
//! - **Reject**: 拒绝包含敏感信息的内容

use crate::error::Result;
use regex::Regex;

/// PII 类型
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PiiType {
    /// 中国手机号（1xx-xxxx-xxxx）
    PhoneCn,
    /// 中国身份证号（18 位）
    IdCardCn,
    /// 邮箱地址
    Email,
    /// API Key（OpenAI sk-...、GitHub ghp_...、Anthropic sk-ant-...）
    ApiKey,
    /// 信用卡号（Visa 4xxx、MasterCard 5xxx）
    CreditCard,
}

impl std::fmt::Display for PiiType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PiiType::PhoneCn => write!(f, "phone_cn"),
            PiiType::IdCardCn => write!(f, "id_card_cn"),
            PiiType::Email => write!(f, "email"),
            PiiType::ApiKey => write!(f, "api_key"),
            PiiType::CreditCard => write!(f, "credit_card"),
        }
    }
}

/// 检测到的敏感信息实例
#[derive(Debug, Clone)]
pub struct PiiMatch {
    pub pii_type: PiiType,
    /// 起始字节位置
    pub start: usize,
    /// 结束字节位置
    pub end: usize,
    /// 匹配的文本（可能已截断）
    pub matched: String,
}

/// 处理模式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentGuardMode {
    /// 只记录，不干预
    Detect,
    /// 自动替换为 `[REDACTED]`
    Redact,
    /// 拒绝整条内容
    Reject,
}

/// 内容护栏
pub struct ContentGuard {
    patterns: Vec<(PiiType, Regex)>,
    mode: ContentGuardMode,
}

impl ContentGuard {
    /// 创建带默认 PII 模式的内容护栏
    pub fn new(mode: ContentGuardMode) -> Self {
        Self {
            patterns: build_default_patterns(),
            mode,
        }
    }

    /// 检测内容中的所有 PII 匹配
    pub fn detect(&self, content: &str) -> Vec<PiiMatch> {
        let mut matches = Vec::new();
        for (pii_type, re) in &self.patterns {
            for m in re.find_iter(content) {
                matches.push(PiiMatch {
                    pii_type: pii_type.clone(),
                    start: m.start(),
                    end: m.end(),
                    matched: m.as_str().to_string(),
                });
            }
        }
        // 按起始位置排序
        matches.sort_by_key(|a| a.start);
        matches
    }

    /// 脱敏：将所有 PII 替换为 `[REDACTED]`
    pub fn redact(&self, content: &str) -> String {
        let mut result = content.to_string();
        let matches = self.detect(content);
        // 从后往前替换以保持位置正确
        for m in matches.iter().rev() {
            let replacement = match m.pii_type {
                PiiType::ApiKey => "[REDACTED:API_KEY]",
                PiiType::PhoneCn => "[REDACTED:PHONE]",
                PiiType::Email => "[REDACTED:EMAIL]",
                PiiType::IdCardCn => "[REDACTED:ID_CARD]",
                PiiType::CreditCard => "[REDACTED:CARD]",
            };
            result.replace_range(m.start..m.end, replacement);
        }
        result
    }

    /// 检查内容是否包含敏感信息。返回是否通过检查。
    pub fn is_clean(&self, content: &str) -> bool {
        self.detect(content).is_empty()
    }

    /// 执行内容检查。根据 mode 返回 `Reject` 或执行脱敏。
    pub fn check(&self, content: &str) -> Result<ContentGuardResult> {
        let matches = self.detect(content);

        if matches.is_empty() {
            return Ok(ContentGuardResult::Pass);
        }

        match self.mode {
            ContentGuardMode::Detect => {
                let types: Vec<String> = matches.iter().map(|m| m.pii_type.to_string()).collect();
                Ok(ContentGuardResult::Detected { pii_types: types })
            }
            ContentGuardMode::Reject => {
                let types: Vec<String> = matches.iter().map(|m| m.pii_type.to_string()).collect();
                Ok(ContentGuardResult::Rejected { pii_types: types })
            }
            ContentGuardMode::Redact => {
                let redacted = self.redact(content);
                Ok(ContentGuardResult::Redacted(redacted))
            }
        }
    }
}

/// 内容护栏检查结果
#[derive(Debug, Clone)]
pub enum ContentGuardResult {
    /// 无敏感信息
    Pass,
    /// 检测到敏感信息（Detect 模式）
    Detected { pii_types: Vec<String> },
    /// 已拒绝（Reject 模式）
    Rejected { pii_types: Vec<String> },
    /// 已脱敏（Redact 模式），返回脱敏后的内容
    Redacted(String),
}

impl ContentGuardResult {
    pub fn is_rejected(&self) -> bool {
        matches!(self, ContentGuardResult::Rejected { .. })
    }
}

// ── 默认正则模式 ──────────────────────────────────────────────────────────

fn build_default_patterns() -> Vec<(PiiType, Regex)> {
    [
        // 中国手机号：1[3-9]xxxxxxxxx
        (PiiType::PhoneCn, r"1[3-9]\d{9}"),
        // 中国身份证号（18位）
        (
            PiiType::IdCardCn,
            r"\b[1-9]\d{5}(?:19|20)\d{2}(?:0[1-9]|1[0-2])(?:0[1-9]|[12]\d|3[01])\d{3}[\dXx]\b",
        ),
        // 邮箱
        (
            PiiType::Email,
            r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b",
        ),
        // API Key：OpenAI (sk-), Anthropic (sk-ant-), GitHub (ghp_)
        (
            PiiType::ApiKey,
            r"\b(sk-(?:ant-)?[A-Za-z0-9]{16,}|ghp_[A-Za-z0-9]{36,}|xai-[A-Za-z0-9]{16,})\b",
        ),
        // 信用卡号：Visa (4xxx) / MasterCard (5xxx)，16位
        (
            PiiType::CreditCard,
            r"\b[45]\d{3}[\s-]?\d{4}[\s-]?\d{4}[\s-]?\d{4}\b",
        ),
    ]
    .into_iter()
    .filter_map(|(pii_type, pattern)| Regex::new(pattern).ok().map(|regex| (pii_type, regex)))
    .collect()
}

// ── Guard trait 实现 ─────────────────────────────────────────────────────

impl crate::guard::Guard for ContentGuard {
    fn name(&self) -> &str {
        "content_guard"
    }

    fn check<'a>(
        &'a self,
        content: &'a str,
        _direction: crate::guard::GuardDirection,
    ) -> futures::future::BoxFuture<'a, crate::error::Result<crate::guard::GuardResult>> {
        Box::pin(async move {
            let result = self.check(content)?;
            Ok(match result {
                ContentGuardResult::Pass => crate::guard::GuardResult::Pass,
                ContentGuardResult::Detected { pii_types } => crate::guard::GuardResult::Warn {
                    reasons: vec![format!("检测到敏感信息: {}", pii_types.join(", "))],
                },
                ContentGuardResult::Rejected { pii_types } => crate::guard::GuardResult::Block {
                    reason: format!("内容包含敏感信息: {}", pii_types.join(", ")),
                },
                ContentGuardResult::Redacted(redacted) => crate::guard::GuardResult::Transform {
                    content: redacted,
                    reasons: vec!["内容包含敏感信息，已脱敏处理".to_string()],
                },
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_phone() {
        let guard = ContentGuard::new(ContentGuardMode::Detect);
        let matches = guard.detect("请联系 13812345678 获取帮助");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].pii_type, PiiType::PhoneCn);
    }

    #[test]
    fn test_detect_email() {
        let guard = ContentGuard::new(ContentGuardMode::Detect);
        let matches = guard.detect("邮箱: user@example.com");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].pii_type, PiiType::Email);
    }

    #[test]
    fn test_detect_api_key() {
        let guard = ContentGuard::new(ContentGuardMode::Detect);
        let matches = guard.detect("export OPENAI_API_KEY=sk-proj1234567890abcdef");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].pii_type, PiiType::ApiKey);
    }

    #[test]
    fn test_redact() {
        let guard = ContentGuard::new(ContentGuardMode::Redact);
        let redacted = guard.redact("电话: 13812345678, 邮箱: user@example.com");
        assert!(!redacted.contains("13812345678"));
        assert!(!redacted.contains("user@example.com"));
        assert!(redacted.contains("[REDACTED:PHONE]"));
        assert!(redacted.contains("[REDACTED:EMAIL]"));
    }

    #[test]
    fn test_reject_mode() {
        let guard = ContentGuard::new(ContentGuardMode::Reject);
        let result = guard.check("我的邮箱是 test@gmail.com").unwrap();
        assert!(result.is_rejected());
    }

    #[test]
    fn test_clean_content_passes() {
        let guard = ContentGuard::new(ContentGuardMode::Reject);
        let result = guard.check("这是一段正常的内容没有敏感信息").unwrap();
        assert!(!result.is_rejected());
    }
}
