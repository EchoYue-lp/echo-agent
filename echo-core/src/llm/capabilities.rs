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

use crate::llm::{LlmApiProtocol, ThinkingLevel, ThinkingProtocol};
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

// ── ThinkingProfile ──────────────────────────────────────────────────

const GPT_56_LEVELS: &[ThinkingLevel] = &[
    ThinkingLevel::None,
    ThinkingLevel::Low,
    ThinkingLevel::Medium,
    ThinkingLevel::High,
    ThinkingLevel::Xhigh,
    ThinkingLevel::Max,
];
const CLAUDE_46_LEVELS: &[ThinkingLevel] = &[
    ThinkingLevel::Low,
    ThinkingLevel::Medium,
    ThinkingLevel::High,
    ThinkingLevel::Xhigh,
    ThinkingLevel::Max,
];
const DEEPSEEK_V4_LEVELS: &[ThinkingLevel] = &[
    ThinkingLevel::None,
    ThinkingLevel::Low,
    ThinkingLevel::High,
    ThinkingLevel::Max,
];
const GLM_52_LEVELS: &[ThinkingLevel] =
    &[ThinkingLevel::None, ThinkingLevel::High, ThinkingLevel::Max];
const KIMI_K3_LEVELS: &[ThinkingLevel] =
    &[ThinkingLevel::Low, ThinkingLevel::High, ThinkingLevel::Max];
const GEMINI_3_LEVELS: &[ThinkingLevel] = &[
    ThinkingLevel::Minimal,
    ThinkingLevel::Low,
    ThinkingLevel::Medium,
    ThinkingLevel::High,
];
const GEMINI_25_LEVELS: &[ThinkingLevel] = &[
    ThinkingLevel::None,
    ThinkingLevel::Low,
    ThinkingLevel::Medium,
    ThinkingLevel::High,
];
const TOGGLE_LEVELS: &[ThinkingLevel] = &[ThinkingLevel::None, ThinkingLevel::High];
const OLLAMA_GPT_OSS_LEVELS: &[ThinkingLevel] = &[
    ThinkingLevel::Low,
    ThinkingLevel::Medium,
    ThinkingLevel::High,
];

/// Centrally resolved request-side thinking capabilities for one concrete
/// provider endpoint, API protocol, and model id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThinkingProfile {
    pub protocol: ThinkingProtocol,
    /// Effective choices only. The application adds `auto` separately; an
    /// empty slice means the model decides or no verified control is known.
    pub levels: &'static [ThinkingLevel],
}

impl ThinkingProfile {
    pub const fn new(protocol: ThinkingProtocol, levels: &'static [ThinkingLevel]) -> Self {
        Self { protocol, levels }
    }

    pub const fn unknown() -> Self {
        Self::new(ThinkingProtocol::None, &[])
    }

    pub fn supports_manual_control(self) -> bool {
        self.protocol.emits_field() && !self.levels.is_empty()
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

    /// Verified effective levels for the resolved thinking protocol.
    pub thinking_levels: &'static [ThinkingLevel],

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
pub fn infer_context_window(provider: &str, model_name: &str) -> Option<u32> {
    let provider = provider.trim().to_ascii_lowercase();
    let lower = model_name.to_ascii_lowercase();
    if matches!(provider.as_str(), "openai" | "azure-openai") && lower.starts_with("gpt-5.6") {
        // GPT-5.6 Sol/Terra/Luna expose a 1.05M context window.
        Some(1_050_000)
    } else if provider == "anthropic"
        && (lower.starts_with("claude-fable-5")
            || lower.starts_with("claude-opus-4-8")
            || lower.starts_with("claude-sonnet-5"))
        || provider == "deepseek" && lower.starts_with("deepseek-v4")
        || matches!(
            provider.as_str(),
            "dashscope" | "qwen" | "aliyun" | "alibaba"
        ) && (lower.starts_with("qwen3.7-max") || lower.starts_with("qwen3.7-plus"))
        || provider == "zhipu" && lower.starts_with("glm-5.2")
    {
        Some(1_000_000)
    } else if provider == "moonshot"
        && (lower.starts_with("kimi-k2.7") || lower.starts_with("kimi-k2.6"))
    {
        Some(256_000)
    } else {
        None
    }
}

impl ModelProfile {
    /// Build a profile from provider capabilities and a model name.
    /// Model-specific overrides are applied based on known model name patterns.
    pub fn new(model_name: &str, provider: &str, capabilities: ProviderCapabilities) -> Self {
        let lower = model_name.to_ascii_lowercase();
        let thinking_profile =
            resolve_thinking_profile(provider, model_name, LlmApiProtocol::ChatCompletions, None);
        let thinking_protocol = thinking_profile.protocol;

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
            thinking_levels: thinking_profile.levels,
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

/// Resolve the single authoritative thinking profile for a runtime model.
///
/// The endpoint/provider dialect is evaluated before the model family because
/// gateways can expose the same model through different wire fields. Unknown
/// models remain fully usable and simply receive no manual thinking control.
pub fn resolve_thinking_profile(
    provider: &str,
    model: &str,
    api_protocol: LlmApiProtocol,
    endpoint: Option<&str>,
) -> ThinkingProfile {
    use ThinkingProtocol as T;

    let provider = provider.trim().to_ascii_lowercase();
    let model = model.trim().to_ascii_lowercase();
    let endpoint = endpoint.unwrap_or_default().to_ascii_lowercase();
    let is_dashscope = matches!(
        provider.as_str(),
        "dashscope" | "qwen" | "aliyun" | "alibaba" | "modelstudio" | "bailian"
    ) || endpoint.contains("dashscope.aliyuncs.com");
    let is_ollama = provider == "ollama"
        || endpoint.contains("localhost:11434")
        || endpoint.contains("127.0.0.1:11434");

    if model.starts_with("claude-") {
        let Some((major, minor)) = model_family_version(&model, "claude-") else {
            return ThinkingProfile::unknown();
        };
        if major < 4 || (major == 4 && minor < 6) {
            return ThinkingProfile::unknown();
        }
        if major == 4 && minor == 6 {
            return if api_protocol == LlmApiProtocol::Anthropic {
                ThinkingProfile::new(T::AnthropicEffort, CLAUDE_46_LEVELS)
            } else {
                ThinkingProfile::new(T::OpenaiReasoningEffort, CLAUDE_46_LEVELS)
            };
        }
        return ThinkingProfile::new(T::AnthropicAdaptive, &[]);
    }

    if api_protocol == LlmApiProtocol::Anthropic {
        return ThinkingProfile::unknown();
    }

    if is_ollama && api_protocol == LlmApiProtocol::ChatCompletions {
        if model.starts_with("gpt-oss") {
            return ThinkingProfile::new(T::OllamaThink, OLLAMA_GPT_OSS_LEVELS);
        }
        if model.starts_with("qwen3")
            || model.starts_with("deepseek-r1")
            || model.starts_with("deepseek-v3")
            || model.starts_with("deepseek-v4")
            || model.starts_with("magistral")
        {
            return ThinkingProfile::new(T::OllamaThink, TOGGLE_LEVELS);
        }
        return ThinkingProfile::unknown();
    }

    if model.starts_with("gpt-5.6") || model.starts_with("gpt-5-6") {
        return ThinkingProfile::new(T::OpenaiReasoningEffort, GPT_56_LEVELS);
    }

    if model.starts_with("deepseek-v4") {
        if is_dashscope && api_protocol == LlmApiProtocol::ChatCompletions {
            return ThinkingProfile::new(T::EnableThinkingFlag, TOGGLE_LEVELS);
        }
        return ThinkingProfile::new(T::DeepseekReasoningEffort, DEEPSEEK_V4_LEVELS);
    }

    if let Some((major, minor)) = model_family_version(&model, "glm-")
        && (major > 5 || (major == 5 && minor >= 2))
        && api_protocol == LlmApiProtocol::ChatCompletions
    {
        return ThinkingProfile::new(T::GlmReasoningEffort, GLM_52_LEVELS);
    }

    if model.starts_with("kimi-k3") && api_protocol == LlmApiProtocol::ChatCompletions {
        return ThinkingProfile::new(T::OpenaiReasoningEffort, KIMI_K3_LEVELS);
    }
    if model.starts_with("kimi-k2.7") {
        return ThinkingProfile::new(T::ModelManaged, &[]);
    }
    if model.starts_with("kimi-k2.6") && api_protocol == LlmApiProtocol::ChatCompletions {
        return ThinkingProfile::new(T::ThinkingType, TOGGLE_LEVELS);
    }

    if model.starts_with("qwen3") && api_protocol == LlmApiProtocol::ChatCompletions {
        return ThinkingProfile::new(T::EnableThinkingFlag, TOGGLE_LEVELS);
    }

    if (model.starts_with("gemini-3") || model.starts_with("gemini-3."))
        && api_protocol == LlmApiProtocol::ChatCompletions
    {
        return ThinkingProfile::new(T::OpenaiReasoningEffort, GEMINI_3_LEVELS);
    }
    if model.starts_with("gemini-2.5") && api_protocol == LlmApiProtocol::ChatCompletions {
        return ThinkingProfile::new(T::OpenaiReasoningEffort, GEMINI_25_LEVELS);
    }

    ThinkingProfile::unknown()
}

fn model_family_version(model: &str, prefix: &str) -> Option<(u32, u32)> {
    let rest = model.strip_prefix(prefix)?;
    let mut segments = rest.split('-').peekable();
    while let Some(segment) = segments.next() {
        if let Some((major, minor)) = segment.split_once('.')
            && let (Ok(major), Ok(minor)) = (major.parse::<u32>(), minor.parse::<u32>())
            && (3..=9).contains(&major)
        {
            return Some((major, minor));
        }
        if let Ok(major) = segment.parse::<u32>()
            && (3..=9).contains(&major)
        {
            let minor = segments
                .peek()
                .and_then(|value| value.parse::<u32>().ok())
                .filter(|value| *value <= 9)
                .unwrap_or(0);
            return Some((major, minor));
        }
    }
    None
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

    fn chat_profile(provider: &str, model: &str) -> ThinkingProfile {
        resolve_thinking_profile(provider, model, LlmApiProtocol::ChatCompletions, None)
    }

    #[test]
    fn gpt_56_has_six_distinct_levels() {
        let profile = chat_profile("openai", "gpt-5.6-sol");
        assert_eq!(profile.protocol, T::OpenaiReasoningEffort);
        assert_eq!(profile.levels, GPT_56_LEVELS);
        assert_eq!(profile.levels.len(), 6);
        assert!(!profile.levels.contains(&ThinkingLevel::Minimal));
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
        assert_eq!(infer_context_window("custom", "gpt-5.6-sol"), None);
        assert_eq!(infer_context_window("openai", "claude-sonnet-5"), None);
        assert_eq!(infer_context_window("custom", "unknown-model"), None);
    }

    #[test]
    fn claude_support_starts_at_46() {
        assert_eq!(
            chat_profile("anthropic", "claude-4.5-sonnet").protocol,
            T::None
        );
        let direct = resolve_thinking_profile(
            "anthropic",
            "claude-opus-4-6",
            LlmApiProtocol::Anthropic,
            Some("https://api.anthropic.com/v1/messages"),
        );
        assert_eq!(direct.protocol, T::AnthropicEffort);
        assert_eq!(direct.levels, CLAUDE_46_LEVELS);
        assert_eq!(
            chat_profile("gateway", "claude-opus-4.6").protocol,
            T::OpenaiReasoningEffort
        );
        assert_eq!(
            resolve_thinking_profile(
                "anthropic",
                "claude-opus-4.7",
                LlmApiProtocol::Anthropic,
                None,
            )
            .protocol,
            T::AnthropicAdaptive
        );
    }

    #[test]
    fn glm_support_starts_at_52() {
        assert_eq!(chat_profile("zhipu", "glm-5.1").protocol, T::None);
        assert_eq!(chat_profile("zhipu", "glm-4.6").protocol, T::None);
        let profile = chat_profile("zhipu", "glm-5.2");
        assert_eq!(profile.protocol, T::GlmReasoningEffort);
        assert_eq!(profile.levels, GLM_52_LEVELS);
    }

    #[test]
    fn common_model_profiles_are_explicit() {
        assert_eq!(
            chat_profile("deepseek", "deepseek-v4").protocol,
            T::DeepseekReasoningEffort
        );
        assert_eq!(chat_profile("moonshot", "kimi-k3").levels, KIMI_K3_LEVELS);
        assert_eq!(
            chat_profile("moonshot", "kimi-k2.7-code").protocol,
            T::ModelManaged
        );
        assert_eq!(
            chat_profile("moonshot", "kimi-k2.6").protocol,
            T::ThinkingType
        );
        assert_eq!(
            chat_profile("dashscope", "qwen3-max").protocol,
            T::EnableThinkingFlag
        );
        assert_eq!(
            chat_profile("google", "gemini-3-pro").levels,
            GEMINI_3_LEVELS
        );
        assert_eq!(
            chat_profile("google", "gemini-2.5-pro").levels,
            GEMINI_25_LEVELS
        );
        assert_eq!(
            chat_profile("ollama", "gpt-oss:20b").levels,
            OLLAMA_GPT_OSS_LEVELS
        );
        assert_eq!(chat_profile("ollama", "qwen3:32b").protocol, T::OllamaThink);
        assert_eq!(chat_profile("ollama", "llama3.1").protocol, T::None);
    }

    #[test]
    fn provider_dialect_precedes_model_family() {
        assert_eq!(
            resolve_thinking_profile(
                "custom",
                "deepseek-v4",
                LlmApiProtocol::ChatCompletions,
                Some("https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions"),
            )
            .protocol,
            T::EnableThinkingFlag
        );
        assert_eq!(
            chat_profile("deepseek", "deepseek-v4").protocol,
            T::DeepseekReasoningEffort
        );
    }

    #[test]
    fn unknown_models_remain_uncontrolled() {
        let profile = chat_profile("custom", "future-model");
        assert_eq!(profile, ThinkingProfile::unknown());
        assert!(!profile.supports_manual_control());
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
