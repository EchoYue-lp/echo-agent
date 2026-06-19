//! Thinking / reasoning-depth control for chat requests.
//!
//! Mainstream models expose "how hard should the model think before answering"
//! in three incompatible ways:
//!
//! | Family | Models | Wire field | Values |
//! |--------|--------|-----------|--------|
//! | OpenAI reasoning | GPT-5 / 5-mini / 5-nano, o3 / o4-mini | `reasoning_effort` | `minimal`/`low`/`medium`/`high` |
//! | Anthropic thinking | Claude 3.7 / 4 Sonnet & Opus (NOT 4.6+/4.7+) | `thinking: {type:"enabled", budget_tokens: N}` | integer `< max_tokens` |
//! | OpenAI-compatible (CN) | Qwen3 / GLM / DeepSeek | `enable_thinking: bool` / `thinking_budget: int` | bool or int |
//!
//! Rather than leak three vendor dialects through the framework, we expose ONE
//! semantic knob ([`ThinkingConfig`]) and translate per-provider in the LLM
//! client implementation. This keeps call sites portable across models.
//!
//! ## Capability detection
//!
//! Before sending a thinking config, the caller should consult
//! [`ThinkingProtocol`] for the resolved model (via [`ModelProfile`]) so a
//! thinking request is never sent to a model that rejects it with a 400 (the
//! most common foot-gun — e.g. GPT-5-nano does not accept `reasoning_effort`,
//! Claude Opus 4.7+ rejects the legacy `thinking` block).
//!
//! [`ModelProfile`]: crate::llm::capabilities::ModelProfile

use serde::{Deserialize, Serialize};

/// Unified reasoning-depth knob, translated to per-provider wire formats by
/// each `LlmClient` implementation.
///
/// `None` on [`crate::llm::ChatRequest`] means "use the model's default
/// behavior" — no thinking field is sent.
///
/// Serialization: tagged so config files can express it as e.g.
/// `"medium"`, `"disabled"`, `{"level":"high"}`, or `{"budget_tokens":4000}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingConfig {
    /// Disable thinking entirely where the model supports a toggle
    /// (Qwen3/GLM `enable_thinking:false`). For models with no off-switch
    /// (o-series), this is treated as the lowest available level.
    Disabled,
    /// Discrete reasoning level. Maps to OpenAI `reasoning_effort`; for
    /// Anthropic it is converted to a budget proportion of `max_tokens`.
    ///
    /// Serialized either as a bare level string (`"high"`) via
    /// [`ThinkingConfig::from_spec`] / [`ThinkingConfig::parse_spec`], or as
    /// `{"level":"high"}` when going through serde directly.
    Level(ThinkingLevel),
    /// Exact token budget for internal reasoning. Primarily targets Anthropic
    /// (`budget_tokens`); other families approximate to the nearest level.
    BudgetTokens(#[serde(rename = "budget_tokens")] u32),
}

impl ThinkingConfig {
    /// Convenience: medium level (a sensible default when the user wants
    /// "think more" without picking specifics).
    pub fn medium() -> Self {
        Self::Level(ThinkingLevel::Medium)
    }

    /// Parse a flexible user/config-facing spec into a `ThinkingConfig`.
    ///
    /// Accepts:
    /// - `"auto"` / `""` → returns `None` (use model default); the caller
    ///   decides whether to treat this as "no config".
    /// - `"disabled"` / `"off"` / `"none"` → [`ThinkingConfig::Disabled`]
    /// - `"minimal"` / `"low"` / `"medium"` / `"high"` → [`ThinkingConfig::Level`]
    /// - A bare integer string (`"4000"`) → [`ThinkingConfig::BudgetTokens`]
    ///
    /// Returns `Ok(None)` for `"auto"`/empty, `Err` for unrecognized strings
    /// (so config typos surface loudly rather than silently defaulting).
    pub fn parse_spec(s: &str) -> std::result::Result<Option<Self>, String> {
        let trimmed = s.trim();
        match trimmed.to_ascii_lowercase().as_str() {
            "" | "auto" | "default" => Ok(None),
            "disabled" | "off" | "false" => Ok(Some(Self::Disabled)),
            other => {
                if let Ok(n) = other.parse::<u32>() {
                    return Ok(Some(Self::BudgetTokens(n)));
                }
                ThinkingLevel::parse(other)
                    .map(|lvl| Some(Self::Level(lvl)))
                    .ok_or_else(|| format!("unrecognized thinking spec: '{s}'"))
            }
        }
    }

    /// The OpenAI `reasoning_effort` string this config maps to, or `None` if
    /// the config has no level equivalent (e.g. a raw budget on a non-Anthropic
    /// model). Used by OpenAI-compatible providers.
    pub fn to_reasoning_effort(&self) -> Option<&'static str> {
        match self {
            Self::Disabled => Some("minimal"),
            Self::Level(ThinkingLevel::Minimal) => Some("minimal"),
            Self::Level(ThinkingLevel::Low) => Some("low"),
            Self::Level(ThinkingLevel::Medium) => Some("medium"),
            Self::Level(ThinkingLevel::High) => Some("high"),
            // A raw budget maps to the nearest effort level for OpenAI-style
            // APIs: <4k ≈ low, <12k ≈ medium, otherwise high.
            Self::BudgetTokens(n) => Some(if *n < 4_000 {
                "low"
            } else if *n < 12_000 {
                "medium"
            } else {
                "high"
            }),
        }
    }

    /// Anthropic `budget_tokens` for this config, given the request's
    /// `max_tokens` (the budget MUST be strictly less than `max_tokens`).
    ///
    /// Returns `None` when thinking is disabled, or when the resolved budget
    /// would be >= max_tokens (which Anthropic rejects with a 400).
    pub fn to_anthropic_budget(&self, max_tokens: u32) -> Option<u32> {
        let budget = match self {
            Self::Disabled => return None,
            Self::BudgetTokens(n) => *n,
            // Levels map to a fraction of the output budget, clamped to a
            // sensible floor/ceiling.
            Self::Level(level) => {
                let frac = match level {
                    ThinkingLevel::Minimal => 0.1,
                    ThinkingLevel::Low => 0.25,
                    ThinkingLevel::Medium => 0.5,
                    ThinkingLevel::High => 0.8,
                };
                ((max_tokens as f64) * frac).round() as u32
            }
        };
        // Anthropic requires budget_tokens < max_tokens. Clamp to max-1 if a
        // caller over-specified; skip entirely if max is too small to think.
        if max_tokens <= 1 {
            return None;
        }
        Some(budget.min(max_tokens - 1))
    }

    /// The Qwen3/GLM `enable_thinking` boolean this config maps to.
    pub fn to_enable_thinking(&self) -> bool {
        !matches!(self, Self::Disabled)
    }
}

/// Discrete reasoning level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingLevel {
    /// Minimal thinking (fastest, cheapest). Only models that accept
    /// `reasoning_effort: "minimal"` (GPT-5 family) honor this precisely;
    /// others round up to `Low`.
    Minimal,
    Low,
    Medium,
    High,
}

impl ThinkingLevel {
    /// Parse from a user-facing string (case-insensitive). Used by config
    /// loading. Returns `None` for unrecognized input rather than panicking.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "minimal" | "min" => Some(Self::Minimal),
            "low" => Some(Self::Low),
            "medium" | "med" | "normal" | "default" => Some(Self::Medium),
            "high" => Some(Self::High),
            _ => None,
        }
    }
}

/// Which thinking wire-protocol a model speaks, if any.
///
/// Used to decide (a) whether to emit a thinking field at all, and (b) which
/// translation to apply. Resolved from the model name in
/// [`ModelProfile`][crate::llm::capabilities::ModelProfile].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingProtocol {
    /// Model does not support a thinking control. Any `ThinkingConfig` on the
    /// request is silently dropped (with a `warn!`) so callers don't have to
    /// branch per-model.
    None,
    /// OpenAI `reasoning_effort` (GPT-5 family, o-series). Requires
    /// `max_completion_tokens` instead of `max_tokens` and forbids
    /// `temperature`/`top_p` for o-series.
    OpenaiReasoningEffort,
    /// Anthropic `thinking: {type:"enabled", budget_tokens}` (Claude 3.7 – 4.5).
    AnthropicThinkingBudget,
    /// Claude 4.6+ / 4.7+ "adaptive thinking": the legacy `thinking` block is
    /// deprecated and returns a 400. The model decides its own depth; we send
    /// no thinking field (any `ThinkingConfig` is dropped with a `warn!`).
    AnthropicAdaptive,
    /// Qwen3 / GLM `enable_thinking` boolean toggle (and optional
    /// `thinking_budget` integer for Qwen3).
    EnableThinkingFlag,
}

impl ThinkingProtocol {
    /// True iff a non-`None` thinking field should actually be emitted for this
    /// protocol. `None` and `AnthropicAdaptive` both mean "don't send anything".
    pub fn emits_field(&self) -> bool {
        matches!(
            self,
            Self::OpenaiReasoningEffort | Self::AnthropicThinkingBudget | Self::EnableThinkingFlag
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_level_parse() {
        assert_eq!(ThinkingLevel::parse("high"), Some(ThinkingLevel::High));
        assert_eq!(ThinkingLevel::parse("MIN"), Some(ThinkingLevel::Minimal));
        assert_eq!(ThinkingLevel::parse("medium"), Some(ThinkingLevel::Medium));
        assert_eq!(ThinkingLevel::parse("bogus"), None);
        assert_eq!(ThinkingLevel::parse(""), None);
    }

    #[test]
    fn test_to_reasoning_effort() {
        assert_eq!(
            ThinkingConfig::Level(ThinkingLevel::High).to_reasoning_effort(),
            Some("high")
        );
        assert_eq!(
            ThinkingConfig::Disabled.to_reasoning_effort(),
            Some("minimal")
        );
        assert_eq!(
            ThinkingConfig::BudgetTokens(2000).to_reasoning_effort(),
            Some("low")
        );
        assert_eq!(
            ThinkingConfig::BudgetTokens(50_000).to_reasoning_effort(),
            Some("high")
        );
    }

    #[test]
    fn test_anthropic_budget_levels() {
        // 50% of 8000 = 4000 budget.
        assert_eq!(
            ThinkingConfig::Level(ThinkingLevel::Medium).to_anthropic_budget(8000),
            Some(4000)
        );
        // High (80%) clamped to < max.
        assert_eq!(
            ThinkingConfig::Level(ThinkingLevel::High).to_anthropic_budget(1000),
            Some(800)
        );
        // Disabled → no budget.
        assert_eq!(ThinkingConfig::Disabled.to_anthropic_budget(8000), None);
    }

    #[test]
    fn test_anthropic_budget_must_be_below_max() {
        // Explicit budget larger than max_tokens is clamped to max-1, not sent
        // as-is (which Anthropic would reject with 400).
        assert_eq!(
            ThinkingConfig::BudgetTokens(10_000).to_anthropic_budget(4000),
            Some(3999)
        );
        // max_tokens == 0/1 → skip entirely (cannot think with no room).
        assert_eq!(ThinkingConfig::medium().to_anthropic_budget(1), None);
    }

    #[test]
    fn test_enable_thinking_flag() {
        assert!(!ThinkingConfig::Disabled.to_enable_thinking());
        assert!(ThinkingConfig::medium().to_enable_thinking());
        assert!(ThinkingConfig::BudgetTokens(1000).to_enable_thinking());
    }

    #[test]
    fn test_emits_field() {
        assert!(!ThinkingProtocol::None.emits_field());
        assert!(!ThinkingProtocol::AnthropicAdaptive.emits_field());
        assert!(ThinkingProtocol::OpenaiReasoningEffort.emits_field());
        assert!(ThinkingProtocol::AnthropicThinkingBudget.emits_field());
        assert!(ThinkingProtocol::EnableThinkingFlag.emits_field());
    }

    #[test]
    fn test_parse_spec() {
        // auto / empty → None
        assert_eq!(ThinkingConfig::parse_spec("auto").unwrap(), None);
        assert_eq!(ThinkingConfig::parse_spec("").unwrap(), None);
        assert_eq!(ThinkingConfig::parse_spec("DEFAULT").unwrap(), None);

        // disabled / off
        assert_eq!(
            ThinkingConfig::parse_spec("disabled").unwrap(),
            Some(ThinkingConfig::Disabled)
        );
        assert_eq!(
            ThinkingConfig::parse_spec("off").unwrap(),
            Some(ThinkingConfig::Disabled)
        );

        // levels
        assert_eq!(
            ThinkingConfig::parse_spec("high").unwrap(),
            Some(ThinkingConfig::Level(ThinkingLevel::High))
        );
        assert_eq!(
            ThinkingConfig::parse_spec("MINIMAL").unwrap(),
            Some(ThinkingConfig::Level(ThinkingLevel::Minimal))
        );

        // numeric budget
        assert_eq!(
            ThinkingConfig::parse_spec("4000").unwrap(),
            Some(ThinkingConfig::BudgetTokens(4000))
        );

        // invalid
        assert!(ThinkingConfig::parse_spec("bogus").is_err());
    }
}
