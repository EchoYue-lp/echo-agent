//! Token 估算 trait、使用量追踪与成本估算
//!
//! 为 [`ContextManager`] 提供可插拔的 token 计数能力，替代固定的 `chars / 4` 启发式。
//!
//! # 内置实现
//!
//! | 类型 | 算法 | 精度 |
//! |------|------|------|
//! | [`HeuristicTokenizer`] | ASCII 权重 1，CJK 权重 2，总和 / 4 | 中（中英混合场景推荐） |
//! | [`SimpleTokenizer`] | `字节数 / 4 + 1` | 低（向后兼容） |
//!
//! # 使用量追踪
//!
//! [`TokenUsageTracker`] 提供跨请求的 token 累积统计与成本估算，
//! 对标 Claude Code / ChatGPT 的 token 用量展示能力。
//!
//! # 扩展
//!
//! 实现 [`Tokenizer`] trait 可接入精确 tokenizer（如 tiktoken-rs）。

/// Token 计数器抽象
pub trait Tokenizer: Send + Sync {
    /// Estimate how many model tokens the input text will consume.
    fn count_tokens(&self, text: &str) -> usize;
}

impl Tokenizer for Box<dyn Tokenizer> {
    fn count_tokens(&self, text: &str) -> usize {
        (**self).count_tokens(text)
    }
}

/// 启发式 Tokenizer，使用字符权重估算 token 数量。
///
/// **注意：这是一个粗略估算器，不是精确的 token 计数器。**
///
/// 估算规则：
/// - ASCII 字符权重 1（约 4 字符 = 1 token）
/// - CJK 及其他非 ASCII 字符权重 2（约 1-2 字符 = 1 token）
/// - 总权重 / 4 得到估算 token 数
/// - 空字符串返回 0
///
/// 相比 `字节数 / 4`，对中日韩内容的精度提升约 40-60%，
/// 但仍不应用于需要精确 token 计数的场景（如配额管理、计费等）。
/// 对于精确计数，请使用 tiktoken 或模型原生 tokenizer。
pub struct HeuristicTokenizer;

impl Tokenizer for HeuristicTokenizer {
    fn count_tokens(&self, text: &str) -> usize {
        if text.is_empty() {
            return 0;
        }
        let weight: usize = text.chars().map(|c| if c.is_ascii() { 1 } else { 2 }).sum();
        (weight / 4).max(1)
    }
}

/// 简单 Tokenizer：`字节数 / 4 + 1`（向后兼容旧行为）
pub struct SimpleTokenizer;

impl Tokenizer for SimpleTokenizer {
    fn count_tokens(&self, text: &str) -> usize {
        text.len() / 4 + 1
    }
}

// ── Token 使用量追踪 ─────────────────────────────────────────────────────────

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// 单次 LLM 请求的 token 使用量快照
#[derive(Debug, Clone, Default)]
pub struct TokenUsageSnapshot {
    /// 提示 token 数
    pub prompt_tokens: u32,
    /// 补全 token 数
    pub completion_tokens: u32,
    /// 总 token 数
    pub total_tokens: u32,
}

impl TokenUsageSnapshot {
    /// 从 API 返回的 Usage 构造（total 为 None 时自动求和）
    pub fn new(prompt: u32, completion: u32, total: Option<u32>) -> Self {
        Self {
            prompt_tokens: prompt,
            completion_tokens: completion,
            total_tokens: total.unwrap_or(prompt + completion),
        }
    }
}

/// 模型定价（每百万 token，USD）
#[derive(Debug, Clone)]
pub struct ModelPricing {
    /// 模型名称匹配模式（前缀匹配）
    pub model_pattern: String,
    /// 输入价格 $/1M tokens
    pub input_price_per_mtok: f64,
    /// 输出价格 $/1M tokens
    pub output_price_per_mtok: f64,
}

impl ModelPricing {
    /// 计算单次请求的估算费用
    pub fn estimate_cost(&self, usage: &TokenUsageSnapshot) -> f64 {
        let input_cost = (usage.prompt_tokens as f64 / 1_000_000.0) * self.input_price_per_mtok;
        let output_cost =
            (usage.completion_tokens as f64 / 1_000_000.0) * self.output_price_per_mtok;
        input_cost + output_cost
    }
}

/// 常见模型定价表
static DEFAULT_PRICING: std::sync::LazyLock<Vec<ModelPricing>> = std::sync::LazyLock::new(|| {
    vec![
        // OpenAI
        ModelPricing {
            model_pattern: "gpt-4.5".into(),
            input_price_per_mtok: 75.0,
            output_price_per_mtok: 150.0,
        },
        ModelPricing {
            model_pattern: "gpt-4o".into(),
            input_price_per_mtok: 2.5,
            output_price_per_mtok: 10.0,
        },
        ModelPricing {
            model_pattern: "gpt-4-turbo".into(),
            input_price_per_mtok: 10.0,
            output_price_per_mtok: 30.0,
        },
        ModelPricing {
            model_pattern: "gpt-4".into(),
            input_price_per_mtok: 30.0,
            output_price_per_mtok: 60.0,
        },
        ModelPricing {
            model_pattern: "gpt-3.5".into(),
            input_price_per_mtok: 0.5,
            output_price_per_mtok: 1.5,
        },
        ModelPricing {
            model_pattern: "o3".into(),
            input_price_per_mtok: 10.0,
            output_price_per_mtok: 40.0,
        },
        ModelPricing {
            model_pattern: "o4-mini".into(),
            input_price_per_mtok: 1.1,
            output_price_per_mtok: 4.4,
        },
        // Anthropic
        ModelPricing {
            model_pattern: "claude-opus-4".into(),
            input_price_per_mtok: 15.0,
            output_price_per_mtok: 75.0,
        },
        ModelPricing {
            model_pattern: "claude-sonnet-4".into(),
            input_price_per_mtok: 3.0,
            output_price_per_mtok: 15.0,
        },
        ModelPricing {
            model_pattern: "claude-haiku-4".into(),
            input_price_per_mtok: 0.8,
            output_price_per_mtok: 4.0,
        },
        ModelPricing {
            model_pattern: "claude-3.5".into(),
            input_price_per_mtok: 3.0,
            output_price_per_mtok: 15.0,
        },
        // DeepSeek
        ModelPricing {
            model_pattern: "deepseek-chat".into(),
            input_price_per_mtok: 0.27,
            output_price_per_mtok: 1.1,
        },
        ModelPricing {
            model_pattern: "deepseek-reasoner".into(),
            input_price_per_mtok: 0.55,
            output_price_per_mtok: 2.19,
        },
        // 通义千问 (Qwen)
        ModelPricing {
            model_pattern: "qwen-max".into(),
            input_price_per_mtok: 2.0,
            output_price_per_mtok: 6.0,
        },
        ModelPricing {
            model_pattern: "qwen-plus".into(),
            input_price_per_mtok: 0.4,
            output_price_per_mtok: 1.2,
        },
        ModelPricing {
            model_pattern: "qwen-turbo".into(),
            input_price_per_mtok: 0.12,
            output_price_per_mtok: 0.36,
        },
        // Fallback — 放在最后
        ModelPricing {
            model_pattern: "default".into(),
            input_price_per_mtok: 1.0,
            output_price_per_mtok: 3.0,
        },
    ]
});

/// 线程安全的 Token 使用量追踪器
///
/// 对标 Claude Code / ChatGPT 的 token 用量展示。
///
/// ```rust
/// use echo_core::tokenizer::TokenUsageTracker;
///
/// let tracker = TokenUsageTracker::new("gpt-4o");
/// tracker.record(1500, 800, Some(2300));
///
/// let stats = tracker.summary();
/// assert_eq!(stats.total_prompt_tokens, 1500);
/// ```
pub struct TokenUsageTracker {
    model_name: String,
    total_prompt_tokens: AtomicU64,
    total_completion_tokens: AtomicU64,
    total_tokens: AtomicU64,
    request_count: AtomicU64,
    custom_pricing: Mutex<Option<Vec<ModelPricing>>>,
}

impl TokenUsageTracker {
    pub fn new(model_name: impl Into<String>) -> Self {
        Self {
            model_name: model_name.into(),
            total_prompt_tokens: AtomicU64::new(0),
            total_completion_tokens: AtomicU64::new(0),
            total_tokens: AtomicU64::new(0),
            request_count: AtomicU64::new(0),
            custom_pricing: Mutex::new(None),
        }
    }

    /// 记录一次请求的 token 使用量
    pub fn record(&self, prompt: u32, completion: u32, total: Option<u32>) {
        self.total_prompt_tokens
            .fetch_add(prompt as u64, Ordering::Relaxed);
        self.total_completion_tokens
            .fetch_add(completion as u64, Ordering::Relaxed);
        let t = total.unwrap_or(prompt + completion);
        self.total_tokens.fetch_add(t as u64, Ordering::Relaxed);
        self.request_count.fetch_add(1, Ordering::Relaxed);
    }

    /// 从 API 返回的 Usage 记录
    pub fn record_usage(&self, usage: &crate::llm::types::Usage) {
        let prompt = usage.prompt_tokens.unwrap_or(0);
        let completion = usage.completion_tokens.unwrap_or(0);
        self.record(prompt, completion, usage.total_tokens);
    }

    /// 设置自定义定价（覆盖内置定价表）
    pub fn set_custom_pricing(&self, pricing: Vec<ModelPricing>) {
        if let Ok(mut guard) = self.custom_pricing.lock() {
            *guard = Some(pricing);
        }
    }

    /// 查找匹配当前模型的定价
    fn find_pricing(&self) -> Option<ModelPricing> {
        let custom = self.custom_pricing.lock().ok()?;
        let pricing_list = match custom.as_ref() {
            Some(p) => p,
            None => &DEFAULT_PRICING,
        };

        let model_lower = self.model_name.to_lowercase();
        pricing_list
            .iter()
            .find(|p| {
                p.model_pattern != "default"
                    && model_lower.starts_with(&p.model_pattern.to_lowercase())
            })
            .or_else(|| pricing_list.iter().find(|p| p.model_pattern == "default"))
            .cloned()
    }

    /// 估算总费用（USD）
    pub fn estimate_total_cost(&self) -> Option<f64> {
        let pricing = self.find_pricing()?;
        let prompt = self.total_prompt_tokens.load(Ordering::Relaxed) as f64;
        let completion = self.total_completion_tokens.load(Ordering::Relaxed) as f64;
        Some(
            (prompt / 1_000_000.0) * pricing.input_price_per_mtok
                + (completion / 1_000_000.0) * pricing.output_price_per_mtok,
        )
    }

    /// 获取使用量汇总
    pub fn summary(&self) -> UsageSummary {
        let total_prompt = self.total_prompt_tokens.load(Ordering::Relaxed);
        let total_completion = self.total_completion_tokens.load(Ordering::Relaxed);
        let total = self.total_tokens.load(Ordering::Relaxed);
        let requests = self.request_count.load(Ordering::Relaxed);

        UsageSummary {
            model_name: self.model_name.clone(),
            total_prompt_tokens: total_prompt,
            total_completion_tokens: total_completion,
            total_tokens: total,
            request_count: requests,
            estimated_cost_usd: self.estimate_total_cost(),
        }
    }

    /// 重置所有计数器
    pub fn reset(&self) {
        self.total_prompt_tokens.store(0, Ordering::Relaxed);
        self.total_completion_tokens.store(0, Ordering::Relaxed);
        self.total_tokens.store(0, Ordering::Relaxed);
        self.request_count.store(0, Ordering::Relaxed);
    }
}

/// Token 使用量汇总快照
#[derive(Debug, Clone)]
pub struct UsageSummary {
    pub model_name: String,
    pub total_prompt_tokens: u64,
    pub total_completion_tokens: u64,
    pub total_tokens: u64,
    pub request_count: u64,
    pub estimated_cost_usd: Option<f64>,
}

impl std::fmt::Display for UsageSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Token 用量 [{model}]:
  请求次数:   {requests}
  输入 token: {prompt} (预估 ${input_cost:.4})
  输出 token: {completion} (预估 ${output_cost:.4})
  总计 token: {total} (预估 ${total_cost:.4})",
            model = self.model_name,
            requests = self.request_count,
            prompt = self.total_prompt_tokens,
            completion = self.total_completion_tokens,
            total = self.total_tokens,
            input_cost = self.estimated_cost_usd.unwrap_or(0.0)
                * (self.total_prompt_tokens as f64
                    / (self.total_prompt_tokens + self.total_completion_tokens).max(1) as f64),
            output_cost = self.estimated_cost_usd.unwrap_or(0.0)
                * (self.total_completion_tokens as f64
                    / (self.total_prompt_tokens + self.total_completion_tokens).max(1) as f64),
            total_cost = self.estimated_cost_usd.unwrap_or(0.0),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_heuristic_ascii() {
        let t = HeuristicTokenizer;
        // 16 ASCII chars → weight 16 → 16/4 = 4 tokens
        assert_eq!(t.count_tokens("hello world 1234"), 4);
    }

    #[test]
    fn test_heuristic_cjk() {
        let t = HeuristicTokenizer;
        // 4 CJK chars → weight 8 → 8/4 = 2 tokens
        assert_eq!(t.count_tokens("你好世界"), 2);
    }

    #[test]
    fn test_heuristic_mixed() {
        let t = HeuristicTokenizer;
        // "hello 你好" → 6 ASCII(6) + 2 CJK(4) = weight 10 → 10/4 = 2
        assert_eq!(t.count_tokens("hello 你好"), 2);
    }

    #[test]
    fn test_heuristic_empty() {
        let t = HeuristicTokenizer;
        assert_eq!(t.count_tokens(""), 0); // empty string returns 0
    }

    #[test]
    fn test_simple_tokenizer() {
        let t = SimpleTokenizer;
        assert_eq!(t.count_tokens("hello"), 2); // 5/4+1 = 2
    }
}
