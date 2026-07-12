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
use std::collections::{HashMap, HashSet};

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

    /// Supports an explicit `tool_choice=none` request control. Providers
    /// without it can still implement final-only mode by exposing no tools.
    pub supports_tool_choice_none: bool,

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
            supports_tool_choice_none: true,
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
            supports_tool_choice_none: false,
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
            supports_tool_choice_none: false,
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

    /// Whether the provider/model accepts explicit `tool_choice=none`.
    pub supports_tool_choice_none: bool,

    /// Known context window in tokens.
    pub context_window: Option<u32>,

    /// Harness-level tool exclusions for this model.
    pub excluded_tools: HashSet<String>,

    /// Stable model-specific system prompt suffix.
    pub prompt_suffix: Option<String>,

    /// Tokenizer name for accurate token counting (None if unknown).
    pub tokenizer_name: Option<&'static str>,
}

/// 根据厂商和模型名称推断上下文窗口大小。
/// 未匹配到已知模式时返回 None。
pub fn infer_context_window(_provider: &str, model_name: &str) -> Option<u32> {
    let lower = model_name.to_ascii_lowercase();
    if lower.starts_with("gpt-5.6") {
        // GPT-5.6 Sol/Terra/Luna expose a 1.05M context window.
        Some(1_050_000)
    } else if lower.starts_with("claude-fable-5")
        || lower.starts_with("claude-opus-4-8")
        || lower.starts_with("claude-sonnet-5")
    {
        Some(1_000_000)
    } else if lower.starts_with("deepseek-v4") {
        Some(1_000_000)
    } else if lower.starts_with("qwen3.7-max") {
        Some(1_000_000)
    } else if lower.starts_with("qwen3.7-plus") {
        Some(1_000_000)
    } else if lower.starts_with("kimi-k2.7") || lower.starts_with("kimi-k2.6") {
        Some(256_000)
    } else if lower.starts_with("glm-5.2") {
        Some(1_000_000)
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
            supports_tool_choice_none: capabilities.supports_tool_choice_none,
            context_window: infer_context_window(provider, model_name),
            excluded_tools: HashSet::new(),
            prompt_suffix: None,
            tokenizer_name,
        }
    }

    /// Build a profile from a provider name and model name.
    pub fn from_provider_name(model_name: &str, provider: &str) -> Self {
        let capabilities = ProviderCapabilities::from_provider_name(provider);
        Self::new(model_name, provider, capabilities)
    }
}

/// Consumer-provided harness overrides for a provider or exact model.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelProfileOverride {
    pub supports_parallel_tool_calls: Option<bool>,
    pub supports_tool_choice_none: Option<bool>,
    pub supports_structured_output: Option<bool>,
    pub context_window: Option<u32>,
    pub excluded_tools: HashSet<String>,
    pub prompt_suffix: Option<String>,
}

impl ModelProfileOverride {
    fn apply_to(&self, profile: &mut ModelProfile) {
        if let Some(value) = self.supports_parallel_tool_calls {
            profile.supports_parallel_tool_calls = value;
        }
        if let Some(value) = self.supports_tool_choice_none {
            profile.supports_tool_choice_none = value;
        }
        if let Some(value) = self.supports_structured_output {
            profile.capabilities.structured_output = value;
        }
        if let Some(value) = self.context_window {
            profile.context_window = Some(value);
        }
        profile
            .excluded_tools
            .extend(self.excluded_tools.iter().cloned());
        if let Some(value) = &self.prompt_suffix {
            profile.prompt_suffix = Some(value.clone());
        }
    }
}

/// Resolves a base [`ModelProfile`] plus consumer-registered overrides.
///
/// Provider defaults are applied first. An exact normalized `provider:model`
/// entry is then applied and therefore has higher precedence.
#[derive(Debug, Clone, Default)]
pub struct ModelProfileResolver {
    provider_defaults: HashMap<String, ModelProfileOverride>,
    exact_models: HashMap<String, ModelProfileOverride>,
}

impl ModelProfileResolver {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_provider_default(
        mut self,
        provider: impl AsRef<str>,
        profile: ModelProfileOverride,
    ) -> Self {
        self.provider_defaults
            .insert(normalize_selector_part(provider.as_ref()), profile);
        self
    }

    pub fn register_exact(
        mut self,
        provider: impl AsRef<str>,
        model: impl AsRef<str>,
        profile: ModelProfileOverride,
    ) -> Self {
        self.exact_models
            .insert(selector_key(provider.as_ref(), model.as_ref()), profile);
        self
    }

    pub fn resolve(
        &self,
        provider: &str,
        model: &str,
        capabilities: ProviderCapabilities,
    ) -> ModelProfile {
        let mut profile = ModelProfile::new(model, provider, capabilities);
        if let Some(default) = self
            .provider_defaults
            .get(&normalize_selector_part(provider))
        {
            default.apply_to(&mut profile);
        }
        if let Some(exact) = self.exact_models.get(&selector_key(provider, model)) {
            exact.apply_to(&mut profile);
        }
        profile
    }
}

fn normalize_selector_part(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn selector_key(provider: &str, model: &str) -> String {
    format!(
        "{}:{}",
        normalize_selector_part(provider),
        normalize_selector_part(model)
    )
}

/// Resolve the thinking wire-protocol from a (lowercased) model name and
/// provider. Pure function so it can be unit-tested without a runtime.
///
/// Verified against each vendor's official API docs (mid-2026):
/// - **OpenAI** GPT-5 / 5-mini (NOT 5-nano), o3 / o4-mini → `reasoning_effort`
/// - **DeepSeek** V3.2+ / R1 → `reasoning_effort` (OpenAI-compatible, NOT enable_thinking)
/// - **Claude 4.6** Sonnet/Opus → `effort` + `thinking:{type:"adaptive"}`
/// - **Claude 3.7 / 4 / 4.5** → `thinking:{type:"enabled",budget_tokens}` (+ effort on 4.5)
/// - **Claude Opus 4.7+** → adaptive-only (budget_tokens returns 400)
/// - **Qwen3** → `enable_thinking` + `thinking_budget`
/// - **GLM-4.5/4.6** → `thinking:{type:"enabled"|"disabled"}` (on/off ONLY, no depth)
/// - **GLM-5.x** → `reasoning_effort` (5.2 confirmed in official OpenAPI; max/xhigh/high/...)
/// - **Kimi** kimi-k2.7-* → thinking always on for k2.7-code; NO request-side
///   depth knob → `None` (depth is chosen by model selection, not a parameter)
fn resolve_thinking_protocol(lower_model: &str, provider: &str) -> ThinkingProtocol {
    use ThinkingProtocol as T;
    let provider_lower = provider.to_ascii_lowercase();

    // ── Alibaba Cloud Model Studio (Bailian / DashScope) ──
    // IMPORTANT: on Bailian, ALL thinking-capable models (Qwen3+, AND DeepSeek
    // hosted on Bailian) use `enable_thinking` + `thinking_budget` — regardless
    // of the model family. This is the OpenAI-compatible Chat Completions entry.
    // DeepSeek hosted on api.deepseek.com (the `deepseek` provider) is the ONLY
    // path that uses `reasoning_effort`. So provider takes precedence over model
    // name here.
    // Verified: https://help.aliyun.com/zh/model-studio/deep-thinking
    if matches!(
        provider_lower.as_str(),
        "dashscope" | "qwen" | "aliyun" | "alibaba" | "modelstudio" | "bailian"
    ) {
        return T::EnableThinkingFlag;
    }

    // ── Anthropic family ──
    if provider_lower == "anthropic" || lower_model.starts_with("claude-") {
        // Claude names put the version in varied positions
        // (`claude-4.5-sonnet`, `claude-opus-4.7`, `claude-5-sonnet`). Scan
        // every dash-segment for the first one that parses as a version.
        if let Some(rest) = lower_model.strip_prefix("claude-") {
            for seg in rest.split('-') {
                // Try X.Y form first (e.g. "4.7").
                if let Some((maj_s, min_s)) = seg.split_once('.') {
                    if let (Ok(maj), Ok(min)) = (maj_s.parse::<u32>(), min_s.parse::<u32>()) {
                        if maj > 4 || (maj == 4 && min >= 7) {
                            return T::AnthropicAdaptive;
                        }
                        if maj == 4 && min == 6 {
                            return T::AnthropicEffort;
                        }
                        break;
                    }
                }
                // Bare integer major (e.g. "5" in `claude-5-sonnet`).
                if let Ok(maj) = seg.parse::<u32>() {
                    if maj > 4 {
                        return T::AnthropicAdaptive;
                    }
                    if maj == 4 {
                        // `claude-4-sonnet` (no minor) → treat as 4.0 (budget).
                        break;
                    }
                }
            }
        }
        return T::AnthropicThinkingBudget;
    }

    // ── DeepSeek (OpenAI-compatible reasoning_effort, NOT enable_thinking) ──
    // DeepSeek-V3.2+ and deepseek-reasoner expose `reasoning_effort`.
    if lower_model.starts_with("deepseek-") {
        return T::OpenaiReasoningEffort;
    }

    // ── OpenAI reasoning family (GPT-5, o-series) ──
    if lower_model.starts_with("gpt-5")
        || lower_model.starts_with("o3")
        || lower_model.starts_with("o4")
    {
        return T::OpenaiReasoningEffort;
    }

    // ── GLM family ──
    // GLM-5.x (5.2 confirmed in the official OpenAPI) exposes
    // `reasoning_effort` (values include `max`, server maps low/medium→high).
    // GLM-4.5/4.6 only have an on/off `thinking:{type}` toggle — NO depth knob.
    if lower_model.starts_with("glm-5") || lower_model.starts_with("glm-5.") {
        return T::GlmReasoningEffort;
    }
    if lower_model.starts_with("glm-4.")
        || lower_model.starts_with("glm-4-")
        || lower_model == "glm-4"
    {
        return T::GlmThinkingType;
    }

    // ── Qwen3 (enable_thinking + thinking_budget) ──
    if lower_model.starts_with("qwen3-") || lower_model.starts_with("qwen-") {
        return T::EnableThinkingFlag;
    }

    T::None
}

// ── Cache Policy ──────────────────────────────────────────────────────

/// Provider-specific cache behaviour that affects how we structure prompts.
///
/// Different providers key their prompt cache on different parts of the
/// request. This policy tells the prompt assembler what to stabilize.
#[derive(Debug, Clone)]
pub struct CachePolicy {
    /// Whether sending a stable `user_id` enables KVCache partition reuse.
    /// True for DeepSeek, false for providers that don't support it.
    pub stable_user_id_enables_cache: bool,
    /// Whether the provider reports cache hit tokens in usage metadata.
    /// False means we can't measure cache effectiveness.
    pub reports_cache_metrics: bool,
    /// Whether tool definitions are part of the cache key. When true,
    /// tool list order and content must be deterministic across requests.
    pub cache_key_includes_tools: bool,
    /// Whether the system message is part of the cache key prefix.
    /// When true, any change to the system message invalidates cache.
    pub cache_key_includes_system_prompt: bool,
    /// Recommended minimum stable prefix length in tokens. Content after
    /// this point in the system prompt can vary without breaking cache.
    /// 0 = entire system prompt must be stable.
    pub recommended_stable_prefix_tokens: usize,
}

impl Default for CachePolicy {
    fn default() -> Self {
        Self {
            stable_user_id_enables_cache: false,
            reports_cache_metrics: true,
            cache_key_includes_tools: true,
            cache_key_includes_system_prompt: true,
            recommended_stable_prefix_tokens: 0,
        }
    }
}

impl CachePolicy {
    /// Cache policy for DeepSeek (KVCache, user_id-based isolation).
    pub fn deepseek() -> Self {
        Self {
            stable_user_id_enables_cache: true,
            reports_cache_metrics: true,
            cache_key_includes_tools: true,
            cache_key_includes_system_prompt: true,
            recommended_stable_prefix_tokens: 2000,
        }
    }

    /// Cache policy for Anthropic (prompt caching, explicit cache breakpoints).
    pub fn anthropic() -> Self {
        Self {
            stable_user_id_enables_cache: false,
            reports_cache_metrics: true,
            cache_key_includes_tools: true,
            cache_key_includes_system_prompt: true,
            recommended_stable_prefix_tokens: 1024,
        }
    }

    /// Cache policy for OpenAI (prefix caching, automatic).
    pub fn openai() -> Self {
        Self {
            stable_user_id_enables_cache: false,
            reports_cache_metrics: true,
            cache_key_includes_tools: true,
            cache_key_includes_system_prompt: true,
            recommended_stable_prefix_tokens: 1024,
        }
    }

    /// Resolve cache policy from the provider name.
    pub fn from_provider(provider: &str) -> Self {
        match provider.to_ascii_lowercase().as_str() {
            "deepseek" => Self::deepseek(),
            "anthropic" => Self::anthropic(),
            "openai" => Self::openai(),
            _ => Self::default(),
        }
    }
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
    fn infers_current_frontier_context_windows() {
        assert_eq!(
            infer_context_window("openai", "gpt-5.6-sol"),
            Some(1_050_000)
        );
        assert_eq!(
            infer_context_window("openai", "gpt-5.6-terra"),
            Some(1_050_000)
        );
        assert_eq!(
            infer_context_window("anthropic", "claude-sonnet-5"),
            Some(1_000_000)
        );
        assert_eq!(
            infer_context_window("anthropic", "claude-opus-4-8"),
            Some(1_000_000)
        );
        assert_eq!(
            infer_context_window("deepseek", "deepseek-v4-pro"),
            Some(1_000_000)
        );
        assert_eq!(
            infer_context_window("dashscope", "qwen3.7-max"),
            Some(1_000_000)
        );
        assert_eq!(
            infer_context_window("dashscope", "qwen3.7-plus"),
            Some(1_000_000)
        );
        assert_eq!(
            infer_context_window("moonshot", "kimi-k2.7-code"),
            Some(256_000)
        );
        assert_eq!(infer_context_window("zhipu", "glm-5.2"), Some(1_000_000));
        assert_eq!(infer_context_window("custom", "unknown-model"), None);
    }

    #[test]
    fn test_anthropic_thinking_budget_vs_adaptive() {
        // Claude 3.7 / 4 / 4.5 → legacy budget block (+ effort on 4.5).
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
        // Claude 4.6 → effort + adaptive thinking block.
        assert_eq!(
            resolve_thinking_protocol("claude-4.6-sonnet", "anthropic"),
            T::AnthropicEffort
        );
        assert_eq!(
            resolve_thinking_protocol("claude-opus-4.6", "anthropic"),
            T::AnthropicEffort
        );
        // Claude 4.7+ → adaptive-only (budget_tokens 400s).
        assert_eq!(
            resolve_thinking_protocol("claude-opus-4.7", "anthropic"),
            T::AnthropicAdaptive
        );
        assert_eq!(
            resolve_thinking_protocol("claude-5-sonnet", "anthropic"),
            T::AnthropicAdaptive
        );
    }

    #[test]
    fn test_cn_reasoning_models() {
        // Qwen3 → enable_thinking + thinking_budget.
        assert_eq!(
            resolve_thinking_protocol("qwen3-235b-a22b", "dashscope"),
            T::EnableThinkingFlag
        );
        assert_eq!(
            resolve_thinking_protocol("qwen3-max", "dashscope"),
            T::EnableThinkingFlag
        );
        // GLM-4.5/4.6 → thinking:{type:enabled|disabled} (on/off ONLY, no depth).
        assert_eq!(
            resolve_thinking_protocol("glm-4.6", "zhipu"),
            T::GlmThinkingType
        );
        assert_eq!(
            resolve_thinking_protocol("glm-4.5", "zhipu"),
            T::GlmThinkingType
        );
        // GLM-5.x → reasoning_effort (depth knob; 5.2 confirmed in OpenAPI).
        assert_eq!(
            resolve_thinking_protocol("glm-5.2", "zhipu"),
            T::GlmReasoningEffort
        );
        // DeepSeek → OpenAI-compatible reasoning_effort (NOT enable_thinking!).
        assert_eq!(
            resolve_thinking_protocol("deepseek-r1", "deepseek"),
            T::OpenaiReasoningEffort
        );
        assert_eq!(
            resolve_thinking_protocol("deepseek-v3.2", "deepseek"),
            T::OpenaiReasoningEffort
        );
    }

    #[test]
    fn test_non_reasoning_models() {
        assert_eq!(resolve_thinking_protocol("gpt-4o", "openai"), T::None);
        assert_eq!(resolve_thinking_protocol("gpt-4-turbo", "openai"), T::None);
        assert_eq!(resolve_thinking_protocol("llama-3", "ollama"), T::None);
        // Kimi K2 Thinking has no request-side depth knob (always on).
        assert_eq!(
            resolve_thinking_protocol("kimi-k2-thinking", "moonshot"),
            T::None
        );
    }

    #[test]
    fn exact_model_override_wins_over_provider_default() {
        let resolver = ModelProfileResolver::new()
            .register_provider_default(
                " OpenAI ",
                ModelProfileOverride {
                    supports_parallel_tool_calls: Some(false),
                    supports_tool_choice_none: Some(false),
                    excluded_tools: HashSet::from(["shell".to_string()]),
                    prompt_suffix: Some("provider suffix".to_string()),
                    ..Default::default()
                },
            )
            .register_exact(
                "openai",
                "GPT-5-CODEX",
                ModelProfileOverride {
                    supports_parallel_tool_calls: Some(true),
                    supports_tool_choice_none: Some(true),
                    context_window: Some(400_000),
                    excluded_tools: HashSet::from(["browser".to_string()]),
                    prompt_suffix: Some("exact suffix".to_string()),
                    ..Default::default()
                },
            );

        let profile = resolver.resolve(
            "OPENAI",
            "gpt-5-codex",
            ProviderCapabilities::openai_compatible(),
        );
        assert!(profile.supports_parallel_tool_calls);
        assert!(profile.supports_tool_choice_none);
        assert_eq!(profile.context_window, Some(400_000));
        assert_eq!(profile.prompt_suffix.as_deref(), Some("exact suffix"));
        assert_eq!(
            profile.excluded_tools,
            HashSet::from(["shell".to_string(), "browser".to_string()])
        );
    }

    #[test]
    fn provider_default_applies_without_fuzzy_model_matching() {
        let resolver = ModelProfileResolver::new().register_provider_default(
            "ollama",
            ModelProfileOverride {
                supports_structured_output: Some(true),
                ..Default::default()
            },
        );
        let profile = resolver.resolve("ollama", "local/custom", ProviderCapabilities::ollama());
        assert!(profile.capabilities.structured_output);
        assert!(!profile.supports_tool_choice_none);
    }
}
