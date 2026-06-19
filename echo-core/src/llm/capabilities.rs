//! Provider capabilities and model profile abstractions.
//!
//! These types allow runtime querying of what features a given LLM provider/model
//! supports, replacing hardcoded provider-specific behavior with capability checks.
//!
//! ## ProviderCapabilities
//!
//! Low-level protocol features that differ across providers (OpenAI-compatible,
//! Anthropic, Ollama). Each provider returns its static capabilities set.
//!
//! ## ModelProfile
//!
//! Higher-level model information resolved from a model name and provider config.
//! Combines provider capabilities with model-specific knowledge (context window,
//! reasoning support, multimodal support, etc.).

use crate::llm::thinking::ThinkingProtocol;

// ── ProviderCapabilities ─────────────────────────────────────────────

/// Protocol-level features that differ across LLM providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderCapabilities {
    /// Supports incremental streaming tool call deltas (OpenAI `delta.tool_calls`).
    /// When false, tool calls are only available in final message (Anthropic block events).
    pub streaming_tool_calls: bool,

    /// Uses named SSE event types (Anthropic `event: content_block_start` etc.)
    /// vs bare `data:` lines (OpenAI-compatible).
    pub named_sse_events: bool,

    /// Supports `reasoning_content` / thinking output in streaming responses
    /// (Qwen3, DeepSeek, OpenAI o-series).
    pub reasoning_content: bool,

    /// Supports image inputs (multimodal content parts).
    pub image_input: bool,

    /// System prompt sent as a top-level request field (Anthropic `system`)
    /// vs as a message in the `messages` array with `role: "system"`.
    pub system_as_top_level: bool,

    /// Uses NDJSON (one JSON object per line, `\n` delimited) for streaming
    /// instead of standard SSE `data:` lines (Ollama).
    pub ndjson_streaming: bool,

    /// Supports function/tool definitions in requests.
    pub tool_support: bool,

    /// Supports structured output (`response_format` with JSON Schema).
    pub structured_output: bool,

    /// Requires a provider-specific version header (Anthropic `anthropic-version`).
    pub requires_version_header: bool,

    /// Supports parallel tool calls (multiple tools requested in a single
    /// response). OpenAI, Anthropic, and most cloud providers support this;
    /// Ollama and local models typically do not.
    pub supports_parallel_tool_calls: bool,

    /// Tokenizer name or identifier for accurate token counting (e.g.
    /// `"cl100k_base"` for GPT-4, `"o200k_base"` for GPT-4o).
    /// `None` means the tokenizer is unknown; callers should fall back to
    /// heuristic counting.
    pub tokenizer_name: Option<&'static str>,
}

impl ProviderCapabilities {
    /// Default capabilities for OpenAI and OpenAI-compatible providers
    /// (DashScope, DeepSeek, Moonshot, Zhipu, etc.).
    pub const fn openai_compatible() -> Self {
        Self {
            streaming_tool_calls: true,
            named_sse_events: false,
            reasoning_content: true,
            image_input: true,
            system_as_top_level: false,
            ndjson_streaming: false,
            tool_support: true,
            structured_output: true,
            requires_version_header: false,
            supports_parallel_tool_calls: true,
            tokenizer_name: None,
        }
    }

    /// Capabilities for Anthropic Messages API.
    pub const fn anthropic() -> Self {
        Self {
            streaming_tool_calls: false, // uses content_block_start/stop events
            named_sse_events: true,
            reasoning_content: false, // not mapped in this implementation
            image_input: true,
            system_as_top_level: true,
            ndjson_streaming: false,
            tool_support: true,
            structured_output: false, // no JSON mode in Messages API
            requires_version_header: true,
            supports_parallel_tool_calls: true,
            tokenizer_name: Some("claude"),
        }
    }

    /// Capabilities for local Ollama.
    pub const fn ollama() -> Self {
        Self {
            streaming_tool_calls: false,
            named_sse_events: false,
            reasoning_content: false,
            image_input: false,
            system_as_top_level: false,
            ndjson_streaming: true,
            tool_support: true,
            structured_output: false,
            requires_version_header: false,
            supports_parallel_tool_calls: false,
            tokenizer_name: None,
        }
    }

    /// Resolve capabilities from a provider name string.
    pub fn from_provider_name(name: &str) -> Self {
        match name.to_ascii_lowercase().as_str() {
            "anthropic" => Self::anthropic(),
            "ollama" => Self::ollama(),
            _ => Self::openai_compatible(),
        }
    }
}

// ── ModelProfile ─────────────────────────────────────────────────────

/// Higher-level model information combining provider capabilities with
/// model-specific knowledge.
#[derive(Debug, Clone)]
pub struct ModelProfile {
    /// Provider name.
    pub provider: String,

    /// The resolved model name.
    pub model_name: String,

    /// Low-level protocol capabilities from the provider.
    pub capabilities: ProviderCapabilities,

    /// Whether this model is known to support `reasoning_content` / thinking.
    pub supports_reasoning: bool,

    /// Which thinking wire-protocol this model speaks, if any. Drives the
    /// translation of `ChatRequest::thinking` in each provider implementation,
    /// and prevents sending a thinking field to a model that would reject it
    /// with a 400 (e.g. GPT-5-nano, Claude Opus 4.7+).
    pub thinking_protocol: crate::llm::thinking::ThinkingProtocol,

    /// Whether this model is multimodal-capable (accepts images).
    pub supports_images: bool,

    /// Whether this model can define and call tools.
    pub supports_tools: bool,

    /// Known maximum output tokens (None if unknown).
    pub max_output_tokens: Option<u32>,

    /// Whether this model supports streaming.
    pub supports_streaming: bool,

    /// Whether this model supports parallel tool calls (derived from provider
    /// capabilities; may be overridden for specific models).
    pub supports_parallel_tool_calls: bool,

    /// Tokenizer name for accurate token counting (None if unknown).
    pub tokenizer_name: Option<&'static str>,
}

/// 根据厂商和模型名称推断上下文窗口大小。
/// 未匹配到已知模式时返回 None。
pub fn infer_context_window(_provider: &str, model_name: &str) -> Option<u32> {
    let lower = model_name.to_ascii_lowercase();
    if lower.contains("qwen3-235b") {
        Some(131_072)
    } else if lower.starts_with("gpt-5.5") || lower.starts_with("gpt-4.5") {
        Some(128_000)
    } else if lower.starts_with("gpt-4o") || lower.contains("gpt-4o") {
        // GPT-4o / GPT-4o-mini: 128K context
        Some(128_000)
    } else if lower.starts_with("gpt-4-turbo") || lower.contains("gpt-4-turbo") {
        Some(128_000)
    } else if lower.starts_with("gpt-4") {
        // Original GPT-4 (non-turbo, non-o): 8K context
        Some(8_192)
    } else if lower.starts_with("claude-3-opus") {
        Some(200_000)
    } else if lower.starts_with("claude-3.5") || lower.starts_with("claude-4") {
        Some(200_000)
    } else if lower.starts_with("claude-") {
        Some(200_000)
    } else if lower.starts_with("deepseek-") {
        Some(128_000)
    } else if lower.starts_with("qwen-") {
        Some(131_072)
    } else {
        None
    }
}

impl ModelProfile {
    /// Build a profile from provider capabilities and a model name.
    /// Model-specific overrides are applied based on known model name patterns.
    pub fn new(model_name: &str, provider: &str, capabilities: ProviderCapabilities) -> Self {
        let lower = model_name.to_ascii_lowercase();
        let thinking_protocol = resolve_thinking_protocol(&lower, provider);

        // Model-specific reasoning detection: a model "supports reasoning" if it
        // both speaks a thinking protocol AND its provider emits reasoning_content
        // in responses. (Adaptive-thinking Claude models still report reasoning
        // content; they just don't accept a request field.)
        let supports_reasoning =
            !matches!(thinking_protocol, ThinkingProtocol::None) && capabilities.reasoning_content;

        // Model-specific image detection
        let supports_images = capabilities.image_input
            && !lower.starts_with("o3-mini") // o3-mini doesn't support images
            && !lower.starts_with("o1-mini")
            && !lower.starts_with("o1-preview");

        // Known max output tokens
        let max_output_tokens = if lower.contains("qwen3-235b") {
            Some(131_072)
        } else if lower.starts_with("gpt-5") || lower.starts_with("o3") || lower.starts_with("o4") {
            Some(16_384)
        } else if lower.starts_with("claude-") {
            Some(8_192)
        } else {
            None
        };

        // Tokenizer name mapping
        let tokenizer_name = if lower.starts_with("gpt-5") || lower.starts_with("gpt-4.5") {
            Some("o200k_base")
        } else if lower.starts_with("gpt-4") || lower.starts_with("gpt-3") {
            Some("cl100k_base")
        } else {
            capabilities.tokenizer_name
        };

        Self {
            provider: provider.to_string(),
            model_name: model_name.to_string(),
            capabilities,
            supports_reasoning,
            thinking_protocol,
            supports_images,
            supports_tools: capabilities.tool_support,
            max_output_tokens,
            supports_streaming: true, // All supported providers stream
            supports_parallel_tool_calls: capabilities.supports_parallel_tool_calls,
            tokenizer_name,
        }
    }

    /// Build a profile from a provider name and model name.
    pub fn from_provider_name(model_name: &str, provider: &str) -> Self {
        let capabilities = ProviderCapabilities::from_provider_name(provider);
        Self::new(model_name, provider, capabilities)
    }
}

/// Resolve the thinking wire-protocol from a (lowercased) model name and
/// provider. Pure function so it can be unit-tested without a runtime.
///
/// Naming conventions as of mid-2026:
/// - GPT-5 / 5-mini / 5-nano, o3 / o4-mini → `reasoning_effort`
///   (note: GPT-5-nano reportedly ignores `reasoning_effort`; callers should
///   still treat the protocol as `OpenaiReasoningEffort` and let the API warn)
/// - Claude 3.7 / 4 Sonnet & Opus (≤ 4.5) → `thinking.budget_tokens`
/// - Claude 4.6 / 4.7+ → adaptive (legacy `thinking` block returns 400)
/// - Qwen3, GLM-4.6+/GLM-5, DeepSeek-R1 → `enable_thinking` (+ `thinking_budget`
///   for Qwen3)
fn resolve_thinking_protocol(lower_model: &str, provider: &str) -> ThinkingProtocol {
    use ThinkingProtocol as T;
    let provider_lower = provider.to_ascii_lowercase();

    // ── Anthropic family ──
    if provider_lower == "anthropic" || lower_model.starts_with("claude-") {
        // Claude model names put the version in different positions
        // (`claude-4.5-sonnet`, `claude-opus-4.7`, `claude-3.7-sonnet`). Scan
        // every dash-segment for the first one that looks like X.Y.
        if let Some(rest) = lower_model.strip_prefix("claude-") {
            for seg in rest.split('-') {
                if let Some((maj_s, min_s)) = seg.split_once('.') {
                    if let (Ok(maj), Ok(min)) = (maj_s.parse::<u32>(), min_s.parse::<u32>()) {
                        // 4.6+ and beyond use adaptive thinking.
                        if maj > 4 || (maj == 4 && min >= 6) {
                            return T::AnthropicAdaptive;
                        }
                        // 3.7–4.5 use the budget block; stop scanning.
                        break;
                    }
                }
            }
        }
        // Claude 3.7 through 4.5 (or unparseable versions) accept the
        // budget_tokens block.
        return T::AnthropicThinkingBudget;
    }

    // ── OpenAI reasoning family (GPT-5, o-series) ──
    if lower_model.starts_with("gpt-5")
        || lower_model.starts_with("o3")
        || lower_model.starts_with("o4")
    {
        return T::OpenaiReasoningEffort;
    }

    // ── OpenAI-compatible CN reasoning models ──
    if lower_model.starts_with("qwen3-")
        || lower_model.starts_with("qwen-")
        || lower_model.starts_with("glm-")
        || lower_model.starts_with("deepseek-r")
    {
        return T::EnableThinkingFlag;
    }

    T::None
}

#[cfg(test)]
mod tests {
    use super::*;
    use ThinkingProtocol as T;

    #[test]
    fn test_openai_reasoning_models() {
        assert_eq!(
            resolve_thinking_protocol("gpt-5", "openai"),
            T::OpenaiReasoningEffort
        );
        assert_eq!(
            resolve_thinking_protocol("gpt-5-mini", "openai"),
            T::OpenaiReasoningEffort
        );
        assert_eq!(
            resolve_thinking_protocol("gpt-5-nano", "openai"),
            T::OpenaiReasoningEffort
        );
        assert_eq!(
            resolve_thinking_protocol("o3-mini", "openai"),
            T::OpenaiReasoningEffort
        );
        assert_eq!(
            resolve_thinking_protocol("o4-mini", "openai"),
            T::OpenaiReasoningEffort
        );
    }

    #[test]
    fn test_anthropic_thinking_budget_vs_adaptive() {
        // Claude 3.7 / 4 / 4.5 → legacy budget block.
        assert_eq!(
            resolve_thinking_protocol("claude-3.7-sonnet", "anthropic"),
            T::AnthropicThinkingBudget
        );
        assert_eq!(
            resolve_thinking_protocol("claude-4-sonnet", "anthropic"),
            T::AnthropicThinkingBudget
        );
        assert_eq!(
            resolve_thinking_protocol("claude-4.5-sonnet", "anthropic"),
            T::AnthropicThinkingBudget
        );
        // Claude 4.6+ / 4.7+ / 5.x → adaptive (no request field).
        assert_eq!(
            resolve_thinking_protocol("claude-4.6-sonnet", "anthropic"),
            T::AnthropicAdaptive
        );
        assert_eq!(
            resolve_thinking_protocol("claude-opus-4.7", "anthropic"),
            T::AnthropicAdaptive
        );
    }

    #[test]
    fn test_cn_reasoning_models() {
        assert_eq!(
            resolve_thinking_protocol("qwen3-235b-a22b", "dashscope"),
            T::EnableThinkingFlag
        );
        assert_eq!(
            resolve_thinking_protocol("glm-4.6", "zhipu"),
            T::EnableThinkingFlag
        );
        assert_eq!(
            resolve_thinking_protocol("deepseek-r1", "deepseek"),
            T::EnableThinkingFlag
        );
    }

    #[test]
    fn test_non_reasoning_models() {
        assert_eq!(resolve_thinking_protocol("gpt-4o", "openai"), T::None);
        assert_eq!(resolve_thinking_protocol("gpt-4-turbo", "openai"), T::None);
        assert_eq!(resolve_thinking_protocol("llama-3", "ollama"), T::None);
    }
}
