//! Thinking / reasoning-depth control for chat requests.
//!
//! Mainstream models expose "how hard should the model think before answering"
//! via several incompatible wire protocols. Verified against each vendor's
//! official API docs (mid-2026):
//!
//! | Family | Models | Wire field | Values |
//! |--------|--------|-----------|--------|
//! | OpenAI reasoning | GPT-5.6 | `reasoning_effort` | `none`/`low`/`medium`/`high`/`xhigh`/`max` |
//! | Anthropic effort | Claude 4.6 (Opus/Sonnet) | `effort` + `thinking:{type:"adaptive"}` | `low`/`medium`/`high`/`xhigh`/`max` |
//! | Anthropic adaptive-only | Claude 4.7+ | no request field | model decides |
//! | Qwen3 | Qwen3-* | `enable_thinking:bool` + `thinking_budget:int` | bool / int |
//! | GLM effort | GLM-5.2+ | `reasoning_effort` (+ `thinking.type`) | effective `none`/`high`/`max` |
//! | Kimi | K3 / K2.7 / K2.6 | effort / model-managed / `thinking.type` | model-specific |
//!
//! Rather than leak vendor dialects through the framework, we expose ONE
//! semantic knob ([`ThinkingConfig`]) and translate per-provider in the LLM
//! client implementation. This keeps call sites portable across models.
//!
//! [`ModelProfile`]: crate::llm::capabilities::ModelProfile

use serde::{Deserialize, Serialize};

/// Unified reasoning-depth knob, translated to per-provider wire formats by
/// each `LlmClient` implementation.
///
/// `None` on [`crate::llm::ChatRequest`] means "use the model's default
/// behavior" — no thinking field is sent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingConfig {
    /// Disable thinking entirely where the model supports a toggle
    /// (Qwen3 `enable_thinking:false`, GLM `thinking.type:"disabled"`,
    /// OpenAI `reasoning_effort:"none"` on GPT-5.1+). For models with no
    /// off-switch, this is treated as the lowest available level.
    Disabled,
    /// Discrete reasoning level. Maps to OpenAI `reasoning_effort` and
    /// Anthropic `effort`; for budget-based protocols it converts to a
    /// fraction of `max_tokens`.
    Level(ThinkingLevel),
    /// Exact token budget for internal reasoning. Targets Anthropic
    /// `budget_tokens` (Claude 3.7–4.5) and Qwen3 `thinking_budget`; other
    /// families approximate to the nearest level.
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
    /// - `"auto"` / `""` → returns `None` (use model default)
    /// - `"disabled"` / `"off"` / `"none"` → [`ThinkingConfig::Disabled`]
    /// - `"minimal"` / `"low"` / `"medium"` / `"high"` / `"xhigh"` / `"max"` → [`ThinkingConfig::Level`]
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

    /// The OpenAI `reasoning_effort` string this config maps to, or `None`.
    ///
    /// Used by OpenAI-compatible providers (OpenAI, Azure, DeepSeek-V3.2+,
    /// Gemini OpenAI-compat). `Disabled` maps to `"none"` (GPT-5.1+) which older
    /// models treat as `"minimal"` at the API; providers may downshift.
    pub fn to_reasoning_effort(&self) -> Option<&'static str> {
        match self {
            Self::Disabled => Some("minimal"),
            Self::Level(ThinkingLevel::None) => Some("none"),
            Self::Level(ThinkingLevel::Minimal) => Some("minimal"),
            Self::Level(ThinkingLevel::Low) => Some("low"),
            Self::Level(ThinkingLevel::Medium) => Some("medium"),
            Self::Level(ThinkingLevel::High) => Some("high"),
            Self::Level(ThinkingLevel::Xhigh) => Some("xhigh"),
            Self::Level(ThinkingLevel::Max) => Some("max"),
            // A raw budget maps to the nearest effort level: <4k ≈ low,
            // <12k ≈ medium, <24k ≈ high, <48k ≈ xhigh, otherwise max.
            Self::BudgetTokens(n) => Some(if *n < 4_000 {
                "low"
            } else if *n < 12_000 {
                "medium"
            } else if *n < 24_000 {
                "high"
            } else if *n < 48_000 {
                "xhigh"
            } else {
                "max"
            }),
        }
    }

    /// The Anthropic `effort` string (Claude 4.6+) this config maps to, or
    /// `None` when thinking is disabled.
    ///
    /// Claude 4.6 accepts: `low`/`medium`/`high`/`xhigh`/`max`.
    ///
    /// `Minimal` → `None` (关闭思考,不是 `low`),与"最低档 = 极速响应"定位一致。
    /// `Disabled` → `None` (不发 effort 字段 + 不发 thinking block = 完全关闭)。
    pub fn to_anthropic_effort(&self) -> Option<&'static str> {
        match self {
            Self::Disabled => None,
            Self::Level(ThinkingLevel::None) => None,
            Self::Level(ThinkingLevel::Minimal) => None,
            Self::Level(ThinkingLevel::Low) => Some("low"),
            Self::Level(ThinkingLevel::Medium) => Some("medium"),
            Self::Level(ThinkingLevel::High) => Some("high"),
            Self::Level(ThinkingLevel::Xhigh) => Some("xhigh"),
            Self::Level(ThinkingLevel::Max) => Some("max"),
            Self::BudgetTokens(n) => Some(if *n < 4_000 {
                "low"
            } else if *n < 12_000 {
                "medium"
            } else if *n < 24_000 {
                "high"
            } else if *n < 48_000 {
                "xhigh"
            } else {
                "max"
            }),
        }
    }

    /// Anthropic `budget_tokens` (Claude 3.7–4.5), given the request's
    /// `max_tokens` (the budget MUST be strictly less than `max_tokens`).
    ///
    /// `Disabled` / `None` / `Minimal` → `None` (关闭思考,极速响应)。
    /// Returns `None` when the resolved budget would be >= max_tokens.
    pub fn to_anthropic_budget(&self, max_tokens: u32) -> Option<u32> {
        let budget = match self {
            Self::Disabled => return None,
            Self::Level(ThinkingLevel::None) => return None,
            Self::Level(ThinkingLevel::Minimal) => return None,
            Self::BudgetTokens(n) => *n,
            Self::Level(level) => {
                let frac = match level {
                    ThinkingLevel::Low => 0.25,
                    ThinkingLevel::Medium => 0.5,
                    ThinkingLevel::High => 0.8,
                    ThinkingLevel::Xhigh => 0.95,
                    ThinkingLevel::Max => 0.98,
                    // None / Minimal unreachable (returned above)
                    ThinkingLevel::None | ThinkingLevel::Minimal => return None,
                };
                ((max_tokens as f64) * frac).round() as u32
            }
        };
        if max_tokens <= 1 {
            return None;
        }
        Some(budget.min(max_tokens - 1))
    }

    /// The Qwen3/GLM `enable_thinking` boolean this config maps to.
    ///
    /// `Minimal` returns `false` because "最低档" = 关闭思考(极速响应,最省
    /// 成本),与 `Disabled` 一致。只有 Low/Medium/High/Xhigh 才开启思考。
    pub fn to_enable_thinking(&self) -> bool {
        !matches!(
            self,
            Self::Disabled | Self::Level(ThinkingLevel::None) | Self::Level(ThinkingLevel::Minimal)
        )
    }

    /// The GLM `thinking.type` value (`"enabled"` or `"disabled"`).
    ///
    /// `Minimal` → `"disabled"` (最低档 = 关闭思考,极速响应)。
    pub fn to_glm_thinking_type(&self) -> &'static str {
        if matches!(
            self,
            Self::Disabled | Self::Level(ThinkingLevel::None) | Self::Level(ThinkingLevel::Minimal)
        ) {
            "disabled"
        } else {
            "enabled"
        }
    }

    /// The GLM-5.x `reasoning_effort` string this config maps to, or `None`.
    ///
    /// GLM-5.2 accepts `max`/`xhigh`/`high`/`medium`/`low`/`minimal`/`none`.
    /// (The server maps `low`/`medium` → `high`, but we send the literal value
    /// so the user's intent is preserved in the request log.)
    pub fn to_glm_reasoning_effort(&self) -> Option<&'static str> {
        match self {
            Self::Disabled => Some("none"),
            Self::Level(ThinkingLevel::None) => Some("none"),
            Self::Level(ThinkingLevel::Minimal) => Some("minimal"),
            Self::Level(ThinkingLevel::Low) => Some("low"),
            Self::Level(ThinkingLevel::Medium) => Some("medium"),
            Self::Level(ThinkingLevel::High) => Some("high"),
            Self::Level(ThinkingLevel::Xhigh) => Some("xhigh"),
            Self::Level(ThinkingLevel::Max) => Some("max"),
            Self::BudgetTokens(n) => Some(if *n < 4_000 {
                "low"
            } else if *n < 12_000 {
                "medium"
            } else if *n < 24_000 {
                "high"
            } else {
                "max"
            }),
        }
    }
}

/// Discrete reasoning level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingLevel {
    /// No thinking (GPT-5.1+ `reasoning_effort:"none"`). On models without a
    /// true off-switch this rounds down to Minimal.
    None,
    /// Minimal thinking (fastest, cheapest). Only models that accept
    /// `reasoning_effort: "minimal"` (GPT-5 family) honor this precisely.
    Minimal,
    Low,
    Medium,
    High,
    /// Extended-high reasoning effort.
    Xhigh,
    /// Maximum reasoning effort. This is distinct from `xhigh` for models such
    /// as GPT-5.6 and Claude 4.6.
    Max,
}

impl ThinkingLevel {
    /// Parse from a user-facing string (case-insensitive). Returns `None` for
    /// unrecognized input rather than panicking.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "none" | "off" => Some(Self::None),
            "minimal" | "min" => Some(Self::Minimal),
            "low" => Some(Self::Low),
            "medium" | "med" | "normal" => Some(Self::Medium),
            "high" => Some(Self::High),
            "xhigh" => Some(Self::Xhigh),
            "max" => Some(Self::Max),
            _ => None,
        }
    }
}

/// Which thinking wire-protocol a model speaks, if any.
///
/// Used to decide (a) whether to emit a thinking field at all, and (b) which
/// translation to apply. It is resolved centrally from the provider endpoint,
/// API protocol, and model id rather than persisted in user configuration.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingProtocol {
    /// Model does not support a thinking control. Any `ThinkingConfig` on the
    /// request is silently dropped (with a `warn!`).
    #[default]
    None,
    /// The model reasons automatically and exposes no request-side control.
    ModelManaged,
    /// OpenAI `reasoning_effort` (GPT-5 family, o-series, **DeepSeek-V3.2+**).
    /// o-series forbids `temperature`/`top_p`.
    OpenaiReasoningEffort,
    /// DeepSeek `reasoning_effort` plus `thinking:{type}`. Its accepted effort
    /// values and disable semantics differ from OpenAI even though both use an
    /// OpenAI-compatible transport.
    DeepseekReasoningEffort,
    /// Anthropic `effort` + `thinking:{type:"adaptive"}` (Claude 4.6 Sonnet/Opus).
    /// On Opus 4.7+ the `thinking` block is dropped entirely (adaptive-only).
    AnthropicEffort,
    /// Anthropic `thinking:{type:"enabled", budget_tokens}` (Claude 3.7 – 4.5).
    /// These models also accept the `effort` param as of 4.5.
    AnthropicThinkingBudget,
    /// Claude Opus 4.7+ — adaptive thinking only. Sending any thinking field
    /// returns a 400, so we drop the config with a `warn!`.
    AnthropicAdaptive,
    /// Qwen3 `enable_thinking` boolean + optional `thinking_budget` integer.
    EnableThinkingFlag,
    /// Generic `thinking:{type:"enabled"|"disabled"}` toggle used by Kimi 2.6
    /// and compatible chat endpoints.
    ThinkingType,
    /// GLM-5.2+ `reasoning_effort`.
    /// Accepts `max`/`xhigh`/`high`/`medium`/`low`/`minimal`/`none`; `low`/
    /// `medium` are server-mapped to `high`, so we still send them faithfully.
    GlmReasoningEffort,
    /// Ollama `think` boolean or level string.
    OllamaThink,
}

impl ThinkingProtocol {
    /// True iff a non-`None` thinking field should actually be emitted for this
    /// protocol. `None` and `AnthropicAdaptive` both mean "don't send anything".
    pub fn emits_field(&self) -> bool {
        matches!(
            self,
            Self::OpenaiReasoningEffort
                | Self::DeepseekReasoningEffort
                | Self::AnthropicEffort
                | Self::AnthropicThinkingBudget
                | Self::EnableThinkingFlag
                | Self::ThinkingType
                | Self::GlmReasoningEffort
                | Self::OllamaThink
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_level_parse() {
        assert_eq!(ThinkingLevel::parse("high"), Some(ThinkingLevel::High));
        assert_eq!(ThinkingLevel::parse("XHIGH"), Some(ThinkingLevel::Xhigh));
        assert_eq!(ThinkingLevel::parse("max"), Some(ThinkingLevel::Max));
        assert_eq!(ThinkingLevel::parse("ultra"), None);
        assert_eq!(ThinkingLevel::parse("none"), Some(ThinkingLevel::None));
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
            ThinkingConfig::Level(ThinkingLevel::Xhigh).to_reasoning_effort(),
            Some("xhigh")
        );
        assert_eq!(
            ThinkingConfig::Level(ThinkingLevel::Max).to_reasoning_effort(),
            Some("max")
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
            Some("max")
        );
    }

    #[test]
    fn test_to_anthropic_effort() {
        assert_eq!(
            ThinkingConfig::Level(ThinkingLevel::Medium).to_anthropic_effort(),
            Some("medium")
        );
        assert_eq!(
            ThinkingConfig::Level(ThinkingLevel::Xhigh).to_anthropic_effort(),
            Some("xhigh")
        );
        assert_eq!(
            ThinkingConfig::Level(ThinkingLevel::Max).to_anthropic_effort(),
            Some("max")
        );
        // Disabled → None (4.6+ models treat absence as model-decided).
        assert_eq!(ThinkingConfig::Disabled.to_anthropic_effort(), None);
    }

    #[test]
    fn test_anthropic_budget_levels() {
        assert_eq!(
            ThinkingConfig::Level(ThinkingLevel::Medium).to_anthropic_budget(8000),
            Some(4000)
        );
        assert_eq!(
            ThinkingConfig::Level(ThinkingLevel::High).to_anthropic_budget(1000),
            Some(800)
        );
        assert_eq!(ThinkingConfig::Disabled.to_anthropic_budget(8000), None);
    }

    #[test]
    fn test_anthropic_budget_must_be_below_max() {
        assert_eq!(
            ThinkingConfig::BudgetTokens(10_000).to_anthropic_budget(4000),
            Some(3999)
        );
        assert_eq!(ThinkingConfig::medium().to_anthropic_budget(1), None);
    }

    #[test]
    fn test_enable_thinking_flag() {
        assert!(!ThinkingConfig::Disabled.to_enable_thinking());
        assert!(ThinkingConfig::medium().to_enable_thinking());
    }

    #[test]
    fn test_glm_thinking_type() {
        assert_eq!(ThinkingConfig::Disabled.to_glm_thinking_type(), "disabled");
        assert_eq!(ThinkingConfig::medium().to_glm_thinking_type(), "enabled");
        assert_eq!(
            ThinkingConfig::Level(ThinkingLevel::High).to_glm_thinking_type(),
            "enabled"
        );
    }

    #[test]
    fn test_emits_field() {
        assert!(!ThinkingProtocol::None.emits_field());
        assert!(!ThinkingProtocol::ModelManaged.emits_field());
        assert!(!ThinkingProtocol::AnthropicAdaptive.emits_field());
        assert!(ThinkingProtocol::OpenaiReasoningEffort.emits_field());
        assert!(ThinkingProtocol::DeepseekReasoningEffort.emits_field());
        assert!(ThinkingProtocol::AnthropicEffort.emits_field());
        assert!(ThinkingProtocol::AnthropicThinkingBudget.emits_field());
        assert!(ThinkingProtocol::EnableThinkingFlag.emits_field());
        assert!(ThinkingProtocol::ThinkingType.emits_field());
        assert!(ThinkingProtocol::GlmReasoningEffort.emits_field());
        assert!(ThinkingProtocol::OllamaThink.emits_field());
    }

    #[test]
    fn test_to_glm_reasoning_effort() {
        assert_eq!(
            ThinkingConfig::Level(ThinkingLevel::Xhigh).to_glm_reasoning_effort(),
            Some("xhigh")
        );
        assert_eq!(
            ThinkingConfig::BudgetTokens(50_000).to_glm_reasoning_effort(),
            Some("max")
        );
        assert_eq!(
            ThinkingConfig::Disabled.to_glm_reasoning_effort(),
            Some("none")
        );
    }

    #[test]
    fn test_parse_spec() {
        assert_eq!(ThinkingConfig::parse_spec("auto"), Ok(None));
        assert_eq!(ThinkingConfig::parse_spec(""), Ok(None));
        assert_eq!(
            ThinkingConfig::parse_spec("disabled"),
            Ok(Some(ThinkingConfig::Disabled))
        );
        assert_eq!(
            ThinkingConfig::parse_spec("xhigh"),
            Ok(Some(ThinkingConfig::Level(ThinkingLevel::Xhigh)))
        );
        assert_eq!(
            ThinkingConfig::parse_spec("max"),
            Ok(Some(ThinkingConfig::Level(ThinkingLevel::Max)))
        );
        assert_eq!(
            ThinkingConfig::parse_spec("4000"),
            Ok(Some(ThinkingConfig::BudgetTokens(4000)))
        );
        assert!(ThinkingConfig::parse_spec("bogus").is_err());
    }
}
