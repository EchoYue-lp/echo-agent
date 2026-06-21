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
        (Some(cfg), ThinkingProtocol::OpenaiReasoningEffort) => {
            // DeepSeek only supports `high`/`max` (and compat-maps `low`/`medium`→`high`,
            // `xhigh`→`max`). It does NOT support `minimal`/`none` — so when the
            // provider is `deepseek`, clamp Minimal/None/Disabled to `"low"` (the
            // lowest valid DeepSeek effort) instead of `"minimal"` (which would 400).
            //
            // DeepSeek ALSO supports `thinking:{type:"enabled"|"disabled"}` as an
            // on/off toggle. When the user explicitly disables thinking, we must
            // send `thinking:{type:"disabled"}` WITHOUT reasoning_effort — because
            // even `reasoning_effort:"low"` engages the thinking loop (server maps
            // low→high). For enabled modes, we send both `reasoning_effort` AND
            // `thinking:{type:"enabled"}` to match the official API spec.
            let is_deepseek = provider == "deepseek";
            if is_deepseek {
                match cfg {
                    ThinkingConfig::Disabled => OpenAiCompatThinking {
                        // DeepSeek: disable thinking completely via the toggle.
                        // Do NOT send reasoning_effort — any value engages thinking.
                        glm_thinking: Some(GlmThinkingBlock {
                            block_type: "disabled".to_string(),
                        }),
                        // Keep temperature: deepseek non-thinking mode accepts it.
                        drop_temperature: false,
                        ..Default::default()
                    },
                    _ => {
                        // Clamp reasoning_effort to deepseek's valid set: only
                        // `high` and `max`. low/medium/minimal → high, xhigh → max.
                        let raw = cfg.to_reasoning_effort().unwrap_or("high");
                        let clamped = match raw {
                            "max" | "xhigh" => "max",
                            _ => "high", // low/medium/minimal/high all → high
                        };
                        OpenAiCompatThinking {
                            reasoning_effort: Some(clamped.to_string()),
                            // DeepSeek requires thinking.type alongside reasoning_effort
                            // per the official API spec (defaults to "enabled", but
                            // sending it explicitly avoids ambiguity).
                            glm_thinking: Some(GlmThinkingBlock {
                                block_type: "enabled".to_string(),
                            }),
                            drop_temperature: true,
                            ..Default::default()
                        }
                    }
                }
            } else {
                let effort = cfg.to_reasoning_effort().map(|e| e.to_string());
                OpenAiCompatThinking {
                    reasoning_effort: effort,
                    drop_temperature: true,
                    ..Default::default()
                }
            }
        }
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
    fn deepseek_uses_reasoning_effort_and_thinking_type_enabled() {
        // DeepSeek-V3.2+ uses BOTH reasoning_effort AND thinking:{type:"enabled"}
        // per the official API spec. The thinking.type toggle is the on/off
        // switch; reasoning_effort controls intensity (high/max only).
        let r = translate(
            "deepseek-r1",
            "deepseek",
            Some(ThinkingConfig::Level(ThinkingLevel::High)),
        );
        assert_eq!(r.reasoning_effort.as_deref(), Some("high"));
        assert!(
            r.glm_thinking.is_some(),
            "DeepSeek must send thinking.type alongside reasoning_effort"
        );
        assert_eq!(
            r.glm_thinking.as_ref().unwrap().block_type,
            "enabled",
            "thinking.type must be 'enabled' for high effort"
        );
        assert!(r.enable_thinking.is_none());
        assert!(r.drop_temperature);
    }

    #[test]
    fn deepseek_disabled_sends_thinking_type_disabled_no_reasoning_effort() {
        // When thinking is disabled, DeepSeek must use thinking:{type:"disabled"}
        // WITHOUT reasoning_effort. Sending ANY reasoning_effort value (even
        // "low") engages the thinking loop because the server maps low→high.
        let r = translate(
            "deepseek-v4-pro",
            "deepseek",
            Some(ThinkingConfig::Disabled),
        );
        assert!(
            r.reasoning_effort.is_none(),
            "DeepSeek disabled: must NOT send reasoning_effort (any value engages thinking)"
        );
        assert!(
            r.glm_thinking.is_some(),
            "DeepSeek disabled: must send thinking.type"
        );
        assert_eq!(
            r.glm_thinking.as_ref().unwrap().block_type,
            "disabled",
            "thinking.type must be 'disabled'"
        );
        assert!(
            !r.drop_temperature,
            "non-thinking mode must keep temperature"
        );
    }

    #[test]
    fn deepseek_xhigh_clamps_to_max_with_thinking_enabled() {
        let r = translate(
            "deepseek-v4-pro",
            "deepseek",
            Some(ThinkingConfig::Level(ThinkingLevel::Xhigh)),
        );
        assert_eq!(r.reasoning_effort.as_deref(), Some("max"));
        assert_eq!(r.glm_thinking.as_ref().unwrap().block_type, "enabled");
        assert!(r.drop_temperature);
    }

    #[test]
    fn deepseek_low_clamps_to_high() {
        // DeepSeek only supports high/max; low→high server-side, but we clamp
        // client-side to avoid relying on server compat mapping.
        let r = translate(
            "deepseek-v4-pro",
            "deepseek",
            Some(ThinkingConfig::Level(ThinkingLevel::Low)),
        );
        assert_eq!(r.reasoning_effort.as_deref(), Some("high"));
        assert_eq!(r.glm_thinking.as_ref().unwrap().block_type, "enabled");
    }

    #[test]
    fn deepseek_medium_clamps_to_high() {
        let r = translate(
            "deepseek-v4-pro",
            "deepseek",
            Some(ThinkingConfig::Level(ThinkingLevel::Medium)),
        );
        assert_eq!(r.reasoning_effort.as_deref(), Some("high"));
        assert_eq!(r.glm_thinking.as_ref().unwrap().block_type, "enabled");
    }

    #[test]
    fn deepseek_budget_tokens_low_uses_high() {
        let r = translate(
            "deepseek-v4-pro",
            "deepseek",
            Some(ThinkingConfig::BudgetTokens(2000)),
        );
        assert_eq!(r.reasoning_effort.as_deref(), Some("high"));
        assert_eq!(r.glm_thinking.as_ref().unwrap().block_type, "enabled");
    }

    #[test]
    fn deepseek_via_dashscope_uses_enable_thinking() {
        // CRITICAL: the SAME deepseek-v4-pro model, hosted on Alibaba Cloud
        // Model Studio (Bailian / DashScope), uses `enable_thinking` — NOT
        // reasoning_effort. Provider takes precedence over model name.
        // Verified: https://help.aliyun.com/zh/model-studio/deep-thinking
        let r = translate(
            "deepseek-v4-pro",
            "dashscope",
            Some(ThinkingConfig::Level(ThinkingLevel::High)),
        );
        assert_eq!(r.enable_thinking, Some(true));
        assert!(
            r.reasoning_effort.is_none(),
            "DeepSeek via DashScope must use enable_thinking, not reasoning_effort"
        );
        assert!(!r.drop_temperature);
    }

    #[test]
    fn qwen_via_aliyun_provider_alias_uses_enable_thinking() {
        // Any alias of the Bailian provider must resolve to enable_thinking.
        let r = translate(
            "qwen3-max",
            "aliyun",
            Some(ThinkingConfig::BudgetTokens(4096)),
        );
        assert_eq!(r.enable_thinking, Some(true));
        assert_eq!(r.thinking_budget, Some(4096));
        assert!(r.reasoning_effort.is_none());
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
