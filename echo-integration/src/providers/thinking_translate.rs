//! Shared thinking-config → OpenAI-compatible wire-field translation.
//!
//! Used by the OpenAI, Azure, Gemini (OpenAI-compat endpoint), and Default
//! LLM clients. Anthropic has its own block-based protocol
//! (`thinking.budget_tokens`) and is translated in `anthropic.rs`; Ollama
//! ignores thinking (warns).

use echo_core::llm::capabilities::{ModelProfile, ProviderCapabilities};
use echo_core::llm::{ThinkingConfig, ThinkingProtocol};
use tracing::warn;

/// Translate a [`ChatRequest::thinking`](echo_core::llm::ChatRequest::thinking)
/// into OpenAI-compatible wire fields, keyed off the model's
/// [`ThinkingProtocol`].
///
/// Returns `(reasoning_effort, enable_thinking, thinking_budget, drop_temperature)`:
/// - `drop_temperature` is `true` for o-series / GPT-5 reasoning models, which
///   reject `temperature`/`top_p` with a 400.
///
/// When `thinking` is `None` or the model speaks no thinking protocol, all
/// three wire fields are `None` and `temperature` is kept.
pub fn translate_thinking_openai_compat(
    model_name: &str,
    provider: &str,
    thinking: &Option<ThinkingConfig>,
    capabilities: ProviderCapabilities,
) -> (
    Option<String>, // reasoning_effort
    Option<bool>,   // enable_thinking
    Option<u32>,    // thinking_budget
    bool,           // drop temperature
) {
    let profile = ModelProfile::new(model_name, provider, capabilities);
    match (thinking, profile.thinking_protocol) {
        (None, _) | (Some(_), ThinkingProtocol::None) => (None, None, None, false),
        (Some(_), ThinkingProtocol::AnthropicAdaptive) => {
            warn!(
                model = model_name,
                "thinking config ignored: model uses adaptive thinking (no request field)"
            );
            (None, None, None, false)
        }
        (Some(cfg), ThinkingProtocol::OpenaiReasoningEffort) => (
            cfg.to_reasoning_effort().map(str::to_string),
            None,
            None,
            // o-series and GPT-5 reasoning models reject temperature/top_p.
            true,
        ),
        (Some(cfg), ThinkingProtocol::EnableThinkingFlag) => {
            // Qwen3 / GLM: enable_thinking + optional thinking_budget.
            let budget = if let ThinkingConfig::BudgetTokens(n) = cfg {
                Some(*n)
            } else {
                None
            };
            (None, Some(cfg.to_enable_thinking()), budget, false)
        }
        (Some(_), ThinkingProtocol::AnthropicThinkingBudget) => {
            // Not an OpenAI-compatible protocol; degrade gracefully.
            (None, None, None, false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::llm::ThinkingLevel;

    fn t(
        model: &str,
        provider: &str,
        thinking: Option<ThinkingConfig>,
    ) -> (Option<String>, Option<bool>, Option<u32>, bool) {
        translate_thinking_openai_compat(
            model,
            provider,
            &thinking,
            ProviderCapabilities::openai_compatible(),
        )
    }

    #[test]
    fn gpt5_high_maps_to_reasoning_effort_and_drops_temperature() {
        let (effort, en, budget, drop_temp) = t(
            "gpt-5",
            "openai",
            Some(ThinkingConfig::Level(ThinkingLevel::High)),
        );
        assert_eq!(effort.as_deref(), Some("high"));
        assert!(en.is_none());
        assert!(budget.is_none());
        assert!(
            drop_temp,
            "GPT-5 reasoning models must drop temperature (would 400)"
        );
    }

    #[test]
    fn o3_minimal_reasoning_effort() {
        let (effort, _, _, drop_temp) = t(
            "o3-mini",
            "openai",
            Some(ThinkingConfig::Level(ThinkingLevel::Minimal)),
        );
        assert_eq!(effort.as_deref(), Some("minimal"));
        assert!(drop_temp);
    }

    #[test]
    fn qwen3_enable_thinking_flag() {
        let (effort, en, budget, drop_temp) = t(
            "qwen3-235b-a22b",
            "dashscope",
            Some(ThinkingConfig::Level(ThinkingLevel::Medium)),
        );
        // Qwen3 speaks enable_thinking, not reasoning_effort.
        assert!(effort.is_none());
        assert_eq!(en, Some(true));
        // Level without explicit budget → no thinking_budget.
        assert!(budget.is_none());
        assert!(!drop_temp, "Qwen3 keeps temperature");
    }

    #[test]
    fn qwen3_budget_tokens_emits_thinking_budget() {
        let (_, _, budget, _) = t(
            "qwen3-max",
            "dashscope",
            Some(ThinkingConfig::BudgetTokens(8000)),
        );
        assert_eq!(budget, Some(8000));
    }

    #[test]
    fn non_reasoning_model_drops_thinking_silently() {
        // gpt-4o speaks no thinking protocol → all fields None, temp kept.
        let (effort, en, budget, drop_temp) = t("gpt-4o", "openai", Some(ThinkingConfig::medium()));
        assert!(effort.is_none());
        assert!(en.is_none());
        assert!(budget.is_none());
        assert!(!drop_temp);
    }

    #[test]
    fn none_thinking_emits_nothing() {
        let (effort, en, budget, drop_temp) = t("gpt-5", "openai", None);
        assert!(effort.is_none());
        assert!(en.is_none());
        assert!(budget.is_none());
        assert!(!drop_temp, "no thinking config must keep temperature");
    }

    #[test]
    fn claude_adaptive_drops_thinking_via_openai_compat_path() {
        // If a Claude 4.7 model were routed through the OpenAI-compat path
        // (e.g. via a proxy), the adaptive protocol must drop the config.
        let (effort, en, budget, drop_temp) = t(
            "claude-opus-4.7",
            "anthropic",
            Some(ThinkingConfig::medium()),
        );
        assert!(effort.is_none());
        assert!(en.is_none());
        assert!(budget.is_none());
        assert!(!drop_temp);
    }
}
