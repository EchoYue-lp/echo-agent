//! Shared thinking-config → OpenAI-compatible wire-field translation.
//!
//! Used by the OpenAI, Azure, Gemini (OpenAI-compat endpoint), Default, and
//! DeepSeek LLM clients. Anthropic has its own block-based protocol
//! (`thinking.budget_tokens` / `effort` / adaptive) translated in
//! `anthropic.rs`; Ollama ignores thinking (warns).
//!
//! All protocols verified against each vendor's official API docs (mid-2026):
//! - OpenAI / DeepSeek / Azure / Gemini-compat → `reasoning_effort`
//! - Qwen3 → `enable_thinking` + `thinking_budget`
//! - GLM-4.5/4.6 → `thinking:{type:"enabled"|"disabled"}` (on/off only)
//! - GLM-5.x → `reasoning_effort` (+ `thinking.type`)

use echo_core::llm::capabilities::{ModelProfile, ProviderCapabilities};
use echo_core::llm::types::GlmThinkingBlock;
use echo_core::llm::{ThinkingConfig, ThinkingProtocol};
use tracing::warn;

/// OpenAI-compatible wire fields derived from a [`ThinkingConfig`].
#[derive(Default, Debug)]
pub struct OpenAiCompatThinking {
    /// `reasoning_effort` (OpenAI / DeepSeek / Azure / Gemini-compat / GLM-5.x).
    pub reasoning_effort: Option<String>,
    /// `enable_thinking` (Qwen3).
    pub enable_thinking: Option<bool>,
    /// `thinking_budget` (Qwen3).
    pub thinking_budget: Option<u32>,
    /// `thinking:{type}` (GLM-4.5/4.6 on/off toggle, and GLM-5.x companion).
    pub glm_thinking: Option<GlmThinkingBlock>,
    /// Whether to drop `temperature` (o-series / GPT-5 reasoning models reject it).
    pub drop_temperature: bool,
}

/// Translate a [`ChatRequest::thinking`](echo_core::llm::ChatRequest::thinking)
/// into OpenAI-compatible wire fields, keyed off the model's
/// [`ThinkingProtocol`].
///
/// When `thinking` is `None` or the model speaks no thinking protocol, all
/// fields are `None` and `temperature` is kept.
pub fn translate_thinking_openai_compat(
    model_name: &str,
    provider: &str,
    thinking: &Option<ThinkingConfig>,
    capabilities: ProviderCapabilities,
) -> OpenAiCompatThinking {
    let profile = ModelProfile::new(model_name, provider, capabilities);
    match (thinking, profile.thinking_protocol) {
        (None, _) | (Some(_), ThinkingProtocol::None) => OpenAiCompatThinking::default(),
        (Some(_), ThinkingProtocol::AnthropicAdaptive) => {
            warn!(
                model = model_name,
                "thinking config ignored: model uses adaptive thinking (no request field)"
            );
            OpenAiCompatThinking::default()
        }
        (Some(cfg), ThinkingProtocol::OpenaiReasoningEffort) => OpenAiCompatThinking {
            reasoning_effort: cfg.to_reasoning_effort().map(str::to_string),
            drop_temperature: true,
            ..Default::default()
        },
        (Some(cfg), ThinkingProtocol::GlmReasoningEffort) => {
            // GLM-5.x: reasoning_effort + the thinking.type toggle (enabled).
            OpenAiCompatThinking {
                reasoning_effort: cfg.to_glm_reasoning_effort().map(str::to_string),
                glm_thinking: Some(GlmThinkingBlock {
                    block_type: cfg.to_glm_thinking_type().to_string(),
                }),
                ..Default::default()
            }
        }
        (Some(cfg), ThinkingProtocol::EnableThinkingFlag) => {
            // Qwen3: enable_thinking + optional thinking_budget.
            let budget = if let ThinkingConfig::BudgetTokens(n) = cfg {
                Some(*n)
            } else {
                None
            };
            OpenAiCompatThinking {
                enable_thinking: Some(cfg.to_enable_thinking()),
                thinking_budget: budget,
                ..Default::default()
            }
        }
        (Some(cfg), ThinkingProtocol::GlmThinkingType) => OpenAiCompatThinking {
            // GLM-4.5/4.6: on/off only — any level collapses to enabled/disabled.
            glm_thinking: Some(GlmThinkingBlock {
                block_type: cfg.to_glm_thinking_type().to_string(),
            }),
            ..Default::default()
        },
        (Some(_), ThinkingProtocol::AnthropicEffort)
        | (Some(_), ThinkingProtocol::AnthropicThinkingBudget) => {
            // Not OpenAI-compatible protocols; degrade gracefully.
            OpenAiCompatThinking::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::llm::ThinkingLevel;

    fn translate(
        model: &str,
        provider: &str,
        thinking: Option<ThinkingConfig>,
    ) -> OpenAiCompatThinking {
        translate_thinking_openai_compat(
            model,
            provider,
            &thinking,
            ProviderCapabilities::openai_compatible(),
        )
    }

    #[test]
    fn gpt5_high_maps_to_reasoning_effort_and_drops_temperature() {
        let r = translate(
            "gpt-5",
            "openai",
            Some(ThinkingConfig::Level(ThinkingLevel::High)),
        );
        assert_eq!(r.reasoning_effort.as_deref(), Some("high"));
        assert!(r.enable_thinking.is_none());
        assert!(r.thinking_budget.is_none());
        assert!(r.glm_thinking.is_none());
        assert!(
            r.drop_temperature,
            "GPT-5 reasoning models must drop temperature (would 400)"
        );
    }

    #[test]
    fn o3_maps_to_reasoning_effort() {
        let r = translate(
            "o3-mini",
            "openai",
            Some(ThinkingConfig::Level(ThinkingLevel::Minimal)),
        );
        assert_eq!(r.reasoning_effort.as_deref(), Some("minimal"));
        assert!(r.drop_temperature);
    }

    #[test]
    fn openai_xhigh_effort() {
        let r = translate(
            "gpt-5",
            "openai",
            Some(ThinkingConfig::Level(ThinkingLevel::Xhigh)),
        );
        assert_eq!(r.reasoning_effort.as_deref(), Some("xhigh"));
    }

    #[test]
    fn deepseek_uses_reasoning_effort_not_enable_thinking() {
        // DeepSeek-V3.2+ / R1 are OpenAI-compatible: reasoning_effort, NOT
        // enable_thinking. Correctness regression for the original impl bug.
        let r = translate(
            "deepseek-r1",
            "deepseek",
            Some(ThinkingConfig::Level(ThinkingLevel::High)),
        );
        assert_eq!(r.reasoning_effort.as_deref(), Some("high"));
        assert!(
            r.enable_thinking.is_none(),
            "DeepSeek must NOT use enable_thinking (no such param)"
        );
        assert!(r.drop_temperature);
    }

    #[test]
    fn qwen3_enable_thinking_flag() {
        let r = translate(
            "qwen3-235b-a22b",
            "dashscope",
            Some(ThinkingConfig::Level(ThinkingLevel::Medium)),
        );
        assert!(r.reasoning_effort.is_none());
        assert_eq!(r.enable_thinking, Some(true));
        assert!(r.thinking_budget.is_none());
        assert!(!r.drop_temperature);
    }

    #[test]
    fn qwen3_budget_tokens_emits_thinking_budget() {
        let r = translate(
            "qwen3-max",
            "dashscope",
            Some(ThinkingConfig::BudgetTokens(8000)),
        );
        assert_eq!(r.thinking_budget, Some(8000));
        assert_eq!(r.enable_thinking, Some(true));
    }

    #[test]
    fn glm_46_uses_thinking_type_block_only() {
        // GLM-4.6: thinking:{type} on/off toggle, NO reasoning_effort.
        let r = translate(
            "glm-4.6",
            "zhipu",
            Some(ThinkingConfig::Level(ThinkingLevel::High)),
        );
        assert!(r.glm_thinking.is_some(), "GLM-4.6 must emit thinking.type");
        assert_eq!(r.glm_thinking.as_ref().unwrap().block_type, "enabled");
        assert!(
            r.reasoning_effort.is_none(),
            "GLM-4.6 has NO reasoning_effort (depth knob is 5.x-only)"
        );
        assert!(r.enable_thinking.is_none());
        assert!(!r.drop_temperature);
    }

    #[test]
    fn glm_46_disabled_emits_disabled_type() {
        let r = translate("glm-4.6", "zhipu", Some(ThinkingConfig::Disabled));
        assert_eq!(r.glm_thinking.as_ref().unwrap().block_type, "disabled");
    }

    #[test]
    fn glm_52_uses_reasoning_effort_and_thinking_type() {
        // GLM-5.2: reasoning_effort (max/xhigh/high/...) + thinking.type.
        let r = translate(
            "glm-5.2",
            "zhipu",
            Some(ThinkingConfig::Level(ThinkingLevel::Xhigh)),
        );
        assert_eq!(r.reasoning_effort.as_deref(), Some("xhigh"));
        assert!(r.glm_thinking.is_some());
        assert_eq!(r.glm_thinking.as_ref().unwrap().block_type, "enabled");
    }

    #[test]
    fn non_reasoning_model_drops_thinking_silently() {
        let r = translate("gpt-4o", "openai", Some(ThinkingConfig::medium()));
        assert!(r.reasoning_effort.is_none());
        assert!(r.enable_thinking.is_none());
        assert!(r.thinking_budget.is_none());
        assert!(r.glm_thinking.is_none());
        assert!(!r.drop_temperature);
    }

    #[test]
    fn none_thinking_emits_nothing() {
        let r = translate("gpt-5", "openai", None);
        assert!(r.reasoning_effort.is_none());
        assert!(
            !r.drop_temperature,
            "no thinking config must keep temperature"
        );
    }
}
