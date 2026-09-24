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
use std::time::{Duration, SystemTime};

/// Version of the conservative model facts compiled into this crate.
pub const BUILT_IN_MODEL_FACTS_VERSION: &str = "echo-core-model-facts-2026-09-15";
const BUILT_IN_MODEL_FACTS_OBSERVED_AT: u64 = 1_789_430_400;

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
    /// Capabilities used when no fresh provider or model evidence is available.
    ///
    /// Unknown clients must opt in to features rather than inheriting another
    /// provider's optimistic behavior.
    pub const fn conservative_unknown() -> Self {
        Self {
            streaming_tool_calls: false,
            named_sse_events: false,
            reasoning_content: false,
            image_input: false,
            system_as_top_level: false,
            ndjson_streaming: false,
            tool_support: false,
            structured_output: false,
            requires_version_header: false,
            supports_parallel_tool_calls: false,
            supports_tool_choice_none: false,
            tokenizer_name: None,
        }
    }

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
        Self::built_in_for_provider(name).unwrap_or_else(Self::conservative_unknown)
    }

    /// Return a versioned built-in capability fallback for a known provider.
    pub fn built_in_for_provider(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "openai" | "azure-openai" => Some(Self::openai_compatible()),
            "anthropic" => Some(Self::anthropic()),
            "ollama" => Some(Self::ollama()),
            // These labels have historically used the OpenAI-compatible
            // integration path. Keep that compatibility while making truly
            // unknown labels conservative.
            "deepseek" | "dashscope" | "qwen" | "aliyun" | "alibaba" | "modelstudio"
            | "bailian" | "zhipu" | "moonshot" | "google" | "gemini" => {
                Some(Self::openai_compatible())
            }
            _ => None,
        }
    }

    /// Return built-in facts only when the provider label and selected wire
    /// protocol describe the same adapter family. A label alone must not make
    /// an Anthropic profile authoritative for an OpenAI-compatible request.
    pub fn built_in_for_protocol(name: &str, protocol: LlmApiProtocol) -> Option<Self> {
        let normalized = name.trim().to_ascii_lowercase();
        match protocol {
            LlmApiProtocol::Anthropic => (normalized == "anthropic").then_some(Self::anthropic()),
            LlmApiProtocol::ChatCompletions => {
                if normalized == "anthropic" {
                    None
                } else if normalized == "ollama" {
                    Some(Self::ollama())
                } else {
                    Self::built_in_for_provider(&normalized)
                        .filter(|capabilities| *capabilities != Self::ollama())
                }
            }
            LlmApiProtocol::Responses => {
                if normalized == "anthropic" || normalized == "ollama" {
                    None
                } else {
                    Self::built_in_for_provider(&normalized)
                }
            }
        }
    }
}

/// Partial provider capability facts. `None` preserves the lower-precedence value.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ProviderCapabilityOverride {
    pub streaming_tool_calls: Option<bool>,
    pub named_sse_events: Option<bool>,
    pub reasoning_content: Option<bool>,
    pub image_input: Option<bool>,
    pub system_as_top_level: Option<bool>,
    pub ndjson_streaming: Option<bool>,
    pub tool_support: Option<bool>,
    pub structured_output: Option<bool>,
    pub requires_version_header: Option<bool>,
    pub supports_parallel_tool_calls: Option<bool>,
    pub supports_tool_choice_none: Option<bool>,
    pub tokenizer_name: Option<String>,
}

impl ProviderCapabilityOverride {
    pub fn from_capabilities(capabilities: ProviderCapabilities) -> Self {
        Self {
            streaming_tool_calls: Some(capabilities.streaming_tool_calls),
            named_sse_events: Some(capabilities.named_sse_events),
            reasoning_content: Some(capabilities.reasoning_content),
            image_input: Some(capabilities.image_input),
            system_as_top_level: Some(capabilities.system_as_top_level),
            ndjson_streaming: Some(capabilities.ndjson_streaming),
            tool_support: Some(capabilities.tool_support),
            structured_output: Some(capabilities.structured_output),
            requires_version_header: Some(capabilities.requires_version_header),
            supports_parallel_tool_calls: Some(capabilities.supports_parallel_tool_calls),
            supports_tool_choice_none: Some(capabilities.supports_tool_choice_none),
            tokenizer_name: capabilities.tokenizer_name.map(str::to_string),
        }
    }

    /// Immutable wire facts implemented by the Chat Completions adapter.
    pub fn openai_chat_protocol() -> Self {
        Self {
            streaming_tool_calls: Some(true),
            named_sse_events: Some(false),
            system_as_top_level: Some(false),
            ndjson_streaming: Some(false),
            requires_version_header: Some(false),
            ..Self::default()
        }
    }

    /// Immutable wire facts implemented by the Responses adapter.
    pub fn openai_responses_protocol() -> Self {
        Self::openai_chat_protocol()
    }

    /// Immutable wire facts implemented by the Anthropic Messages adapter.
    pub fn anthropic_protocol() -> Self {
        Self {
            streaming_tool_calls: Some(false),
            named_sse_events: Some(true),
            image_input: Some(true),
            system_as_top_level: Some(true),
            ndjson_streaming: Some(false),
            tool_support: Some(true),
            structured_output: Some(false),
            requires_version_header: Some(true),
            supports_parallel_tool_calls: Some(true),
            supports_tool_choice_none: Some(false),
            tokenizer_name: Some("claude".to_string()),
            ..Self::default()
        }
    }

    pub fn for_protocol(protocol: LlmApiProtocol) -> Self {
        match protocol {
            LlmApiProtocol::ChatCompletions => Self::openai_chat_protocol(),
            LlmApiProtocol::Responses => Self::openai_responses_protocol(),
            LlmApiProtocol::Anthropic => Self::anthropic_protocol(),
        }
    }

    fn is_empty(&self) -> bool {
        self.streaming_tool_calls.is_none()
            && self.named_sse_events.is_none()
            && self.reasoning_content.is_none()
            && self.image_input.is_none()
            && self.system_as_top_level.is_none()
            && self.ndjson_streaming.is_none()
            && self.tool_support.is_none()
            && self.structured_output.is_none()
            && self.requires_version_header.is_none()
            && self.supports_parallel_tool_calls.is_none()
            && self.supports_tool_choice_none.is_none()
            && self.tokenizer_name.is_none()
    }

    fn apply_to(&self, profile: &mut ModelProfile) {
        macro_rules! apply_bool {
            ($field:ident) => {
                if let Some(value) = self.$field {
                    profile.capabilities.$field = value;
                }
            };
        }
        apply_bool!(streaming_tool_calls);
        apply_bool!(named_sse_events);
        apply_bool!(reasoning_content);
        apply_bool!(image_input);
        apply_bool!(system_as_top_level);
        apply_bool!(ndjson_streaming);
        apply_bool!(tool_support);
        apply_bool!(structured_output);
        apply_bool!(requires_version_header);
        apply_bool!(supports_parallel_tool_calls);
        apply_bool!(supports_tool_choice_none);
        profile.reconcile_derived_capabilities();
        if let Some(tokenizer_name) = self
            .tokenizer_name
            .as_deref()
            .and_then(known_tokenizer_name)
        {
            profile.capabilities.tokenizer_name = Some(tokenizer_name);
            profile.tokenizer_name = Some(tokenizer_name);
        }
    }

    fn apply_wire_to(&self, profile: &mut ModelProfile) {
        macro_rules! apply_wire_bool {
            ($field:ident) => {
                if let Some(value) = self.$field {
                    profile.capabilities.$field = value;
                }
            };
        }
        apply_wire_bool!(streaming_tool_calls);
        apply_wire_bool!(named_sse_events);
        apply_wire_bool!(system_as_top_level);
        apply_wire_bool!(ndjson_streaming);
        apply_wire_bool!(requires_version_header);
    }
}

fn known_tokenizer_name(name: &str) -> Option<&'static str> {
    match name {
        "cl100k_base" => Some("cl100k_base"),
        "o200k_base" => Some("o200k_base"),
        "claude" => Some("claude"),
        _ => None,
    }
}

// ── Model fact provenance ───────────────────────────────────────────

/// Source class for one provider/model fact set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelFactSource {
    /// Conservative absence of unverified capabilities.
    ConservativeUnknown,
    /// Small versioned catalog compiled into `echo-core`.
    BuiltInCatalog,
    /// Protocol and provider facts supplied by a concrete adapter.
    ProviderAdapter,
    /// Facts observed for one exact provider/model identity.
    ExactModel,
    /// Explicit caller configuration for one exact provider/model identity.
    CallerOverride,
}

/// Bounded confidence percentage for a fact source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
#[serde(transparent)]
pub struct ModelFactConfidence(u8);

impl ModelFactConfidence {
    pub const UNKNOWN: Self = Self(0);
    pub const VERIFIED: Self = Self(100);

    /// Construct a confidence value, saturating values above 100 percent.
    pub const fn from_percent_saturating(percent: u8) -> Self {
        if percent > 100 {
            Self::VERIFIED
        } else {
            Self(percent)
        }
    }

    pub const fn percent(self) -> u8 {
        self.0
    }
}

impl<'de> serde::Deserialize<'de> for ModelFactConfidence {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let percent = <u8 as serde::Deserialize>::deserialize(deserializer)?;
        Ok(Self::from_percent_saturating(percent))
    }
}

/// Provenance and freshness metadata shared by every model fact set.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ModelFactMetadata {
    pub source: ModelFactSource,
    pub provenance: String,
    pub version: String,
    pub observed_at: SystemTime,
    /// `None` is reserved for caller-owned or versioned built-in facts whose
    /// lifetime is controlled by replacing their source version.
    pub expires_at: Option<SystemTime>,
    pub confidence: ModelFactConfidence,
}

impl ModelFactMetadata {
    pub fn new(
        source: ModelFactSource,
        provenance: impl Into<String>,
        version: impl Into<String>,
        observed_at: SystemTime,
        expires_at: Option<SystemTime>,
        confidence: ModelFactConfidence,
    ) -> Self {
        Self {
            source,
            provenance: provenance.into(),
            version: version.into(),
            observed_at,
            expires_at,
            confidence,
        }
    }

    /// Whether this source can participate in resolution at `now`.
    pub fn is_fresh_at(&self, now: SystemTime) -> bool {
        self.observed_at <= now && self.expires_at.is_none_or(|expires_at| now <= expires_at)
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
    ///
    /// This compatibility constructor immediately enters the canonical resolver.
    /// Use [`ModelProfileResolver::resolve_at`] when provenance must be retained.
    pub fn new(model_name: &str, provider: &str, capabilities: ProviderCapabilities) -> Self {
        ModelProfileResolver::new().resolve(provider, model_name, capabilities)
    }

    fn new_for_protocol(
        model_name: &str,
        provider: &str,
        capabilities: ProviderCapabilities,
        api_protocol: LlmApiProtocol,
        endpoint: Option<&str>,
    ) -> Self {
        let lower = model_name.to_ascii_lowercase();
        let thinking_profile =
            resolve_thinking_profile(provider, model_name, api_protocol, endpoint);
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
            supports_streaming: capabilities != ProviderCapabilities::conservative_unknown(),
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
        ModelProfileResolver::new()
            .resolve_at(provider, model_name, SystemTime::now())
            .profile
    }

    fn reconcile_derived_capabilities(&mut self) {
        let lower = self.model_name.to_ascii_lowercase();
        self.supports_reasoning = !matches!(self.thinking_protocol, ThinkingProtocol::None)
            && self.capabilities.reasoning_content;
        self.supports_images = self.capabilities.image_input
            && !lower.starts_with("o3-mini")
            && !lower.starts_with("o1-mini")
            && !lower.starts_with("o1-preview");
        self.supports_tools = self.capabilities.tool_support;
        self.supports_parallel_tool_calls = self.capabilities.supports_parallel_tool_calls;
        self.supports_tool_choice_none = self.capabilities.supports_tool_choice_none;
    }

    fn conservative_unknown(model_name: &str, provider: &str) -> Self {
        Self {
            provider: provider.to_string(),
            model_name: model_name.to_string(),
            capabilities: ProviderCapabilities::conservative_unknown(),
            supports_reasoning: false,
            thinking_protocol: ThinkingProtocol::None,
            thinking_levels: &[],
            supports_images: false,
            supports_tools: false,
            max_output_tokens: None,
            supports_streaming: false,
            supports_parallel_tool_calls: false,
            supports_tool_choice_none: false,
            context_window: None,
            excluded_tools: HashSet::new(),
            prompt_suffix: None,
            tokenizer_name: None,
        }
    }
}

/// Consumer-provided harness overrides for a provider or exact model.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ModelProfileOverride {
    pub supports_reasoning: Option<bool>,
    pub thinking_protocol: Option<ThinkingProtocol>,
    pub supports_images: Option<bool>,
    pub supports_tools: Option<bool>,
    pub max_output_tokens: Option<u32>,
    pub supports_streaming: Option<bool>,
    pub supports_parallel_tool_calls: Option<bool>,
    pub supports_tool_choice_none: Option<bool>,
    pub supports_structured_output: Option<bool>,
    pub context_window: Option<u32>,
    pub excluded_tools: HashSet<String>,
    pub prompt_suffix: Option<String>,
}

impl ModelProfileOverride {
    fn apply_to(&self, profile: &mut ModelProfile) {
        if let Some(value) = self.supports_reasoning {
            profile.capabilities.reasoning_content = value;
        }
        if let Some(value) = self.thinking_protocol {
            profile.thinking_protocol = value;
            profile.thinking_levels = &[];
        }
        if let Some(value) = self.supports_images {
            profile.capabilities.image_input = value;
        }
        if let Some(value) = self.supports_tools {
            profile.capabilities.tool_support = value;
        }
        if let Some(value) = self.max_output_tokens {
            profile.max_output_tokens = Some(value);
        }
        if let Some(value) = self.supports_streaming {
            profile.supports_streaming = value;
        }
        if let Some(value) = self.supports_parallel_tool_calls {
            profile.capabilities.supports_parallel_tool_calls = value;
        }
        if let Some(value) = self.supports_tool_choice_none {
            profile.capabilities.supports_tool_choice_none = value;
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
        profile.reconcile_derived_capabilities();
        // Model-level facts are more specific than the provider/model-family
        // defaults above. Re-apply them after reconciliation so an explicit
        // observation (for example, a model that disables images despite a
        // provider-wide multimodal baseline) cannot be overwritten.
        if let Some(value) = self.supports_reasoning {
            profile.supports_reasoning = value;
        }
        if let Some(value) = self.supports_images {
            profile.supports_images = value;
        }
        if let Some(value) = self.supports_tools {
            profile.supports_tools = value;
        }
        if let Some(value) = self.supports_parallel_tool_calls {
            profile.supports_parallel_tool_calls = value;
        }
        if let Some(value) = self.supports_tool_choice_none {
            profile.supports_tool_choice_none = value;
        }
    }

    fn has_values(&self) -> bool {
        self.supports_reasoning.is_some()
            || self.thinking_protocol.is_some()
            || self.supports_images.is_some()
            || self.supports_tools.is_some()
            || self.max_output_tokens.is_some()
            || self.supports_streaming.is_some()
            || self.supports_parallel_tool_calls.is_some()
            || self.supports_tool_choice_none.is_some()
            || self.supports_structured_output.is_some()
            || self.context_window.is_some()
            || !self.excluded_tools.is_empty()
            || self.prompt_suffix.is_some()
    }
}

/// One immutable, sourced set of provider/model facts.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ModelFactSet {
    pub metadata: ModelFactMetadata,
    pub capabilities: ProviderCapabilityOverride,
    pub profile: ModelProfileOverride,
}

impl ModelFactSet {
    pub fn new(
        metadata: ModelFactMetadata,
        capabilities: Option<ProviderCapabilities>,
        profile: ModelProfileOverride,
    ) -> Self {
        Self {
            metadata,
            capabilities: capabilities
                .map(ProviderCapabilityOverride::from_capabilities)
                .unwrap_or_default(),
            profile,
        }
    }

    pub fn new_partial(
        metadata: ModelFactMetadata,
        capabilities: ProviderCapabilityOverride,
        profile: ModelProfileOverride,
    ) -> Self {
        Self {
            metadata,
            capabilities,
            profile,
        }
    }

    fn from_profile(metadata: ModelFactMetadata, profile: &ModelProfile) -> Self {
        Self::new(
            metadata,
            Some(profile.capabilities),
            ModelProfileOverride {
                supports_reasoning: Some(profile.supports_reasoning),
                thinking_protocol: Some(profile.thinking_protocol),
                supports_images: Some(profile.supports_images),
                supports_tools: Some(profile.supports_tools),
                max_output_tokens: profile.max_output_tokens,
                supports_streaming: Some(profile.supports_streaming),
                supports_parallel_tool_calls: Some(profile.supports_parallel_tool_calls),
                supports_tool_choice_none: Some(profile.supports_tool_choice_none),
                supports_structured_output: Some(profile.capabilities.structured_output),
                context_window: profile.context_window,
                excluded_tools: profile.excluded_tools.clone(),
                prompt_suffix: profile.prompt_suffix.clone(),
            },
        )
    }

    pub fn is_fresh_at(&self, now: SystemTime) -> bool {
        self.metadata.is_fresh_at(now)
    }

    fn into_source(mut self, source: ModelFactSource) -> Self {
        self.metadata.source = source;
        self
    }

    fn has_values(&self) -> bool {
        !self.capabilities.is_empty() || self.profile.has_values()
    }
}

/// Serializable sidecar inputs for an existing `LlmConfig` or `ModelConfig`.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ModelFactInputs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_facts: Option<ModelFactSet>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exact_model_facts: Option<ModelFactSet>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub model_overrides: Vec<ModelFactSet>,
}

impl ModelFactInputs {
    pub fn register_with(
        &self,
        mut resolver: ModelProfileResolver,
        provider: &str,
        model: &str,
    ) -> ModelProfileResolver {
        if let Some(facts) = self.provider_facts.clone() {
            resolver = resolver.register_provider_facts(provider, facts);
        }
        if let Some(facts) = self.exact_model_facts.clone() {
            resolver = resolver.register_exact_model_facts(provider, model, facts);
        }
        for facts in self.model_overrides.clone() {
            resolver = resolver.register_explicit_override(provider, model, facts);
        }
        resolver
    }
}

/// Resolved profile plus the source records accepted or rejected by freshness.
#[derive(Debug, Clone)]
pub struct ModelProfileResolution {
    pub profile: ModelProfile,
    /// Owned tokenizer fact recorded in the resolution receipt. The framework's
    /// current calibrated tokenizer remains the runtime estimator; provider
    /// tokenizer dispatch is a separate follow-up boundary.
    pub resolved_tokenizer_name: Option<String>,
    /// Lowest-to-highest precedence records that contributed to the profile.
    pub applied_facts: Vec<ModelFactMetadata>,
    /// Registered records rejected because they were expired or observed later
    /// than the resolution time.
    pub ignored_stale_facts: Vec<ModelFactMetadata>,
    /// Original registered records retained for deterministic per-run refresh.
    pub registered_facts: Vec<ModelFactSet>,
    protocol_facts: Vec<ModelFactSet>,
    provider: String,
    model_name: String,
    api_protocol: LlmApiProtocol,
    endpoint: Option<String>,
}

impl ModelProfileResolution {
    /// Retain a legacy caller-built profile as an exact explicit fact receipt.
    pub fn from_explicit_profile(profile: ModelProfile, observed_at: SystemTime) -> Self {
        let provider = profile.provider.clone();
        let model_name = profile.model_name.clone();
        let resolved_tokenizer_name = profile.tokenizer_name.map(str::to_string);
        let facts = ModelFactSet::from_profile(
            ModelFactMetadata::new(
                ModelFactSource::CallerOverride,
                "legacy ModelProfile value",
                "legacy-model-profile-v1",
                observed_at,
                None,
                ModelFactConfidence::VERIFIED,
            ),
            &profile,
        );
        Self {
            profile,
            resolved_tokenizer_name,
            applied_facts: vec![facts.metadata.clone()],
            ignored_stale_facts: Vec::new(),
            registered_facts: vec![facts],
            protocol_facts: Vec::new(),
            provider,
            model_name,
            api_protocol: LlmApiProtocol::ChatCompletions,
            endpoint: None,
        }
    }

    /// Re-resolve the retained fact inputs at a new safe point.
    ///
    /// Run snapshots use this when no live `LlmClient` is attached, so an
    /// expired profile cannot remain authoritative merely because it was
    /// constructed earlier. The resolver re-establishes source precedence and
    /// freshness instead of mutating the old receipt in place.
    pub fn refresh_at(&self, now: SystemTime) -> Self {
        let mut resolver = ModelProfileResolver::new();
        for facts in &self.registered_facts {
            let is_protocol = self.protocol_facts.iter().any(|protocol| protocol == facts);
            if is_protocol {
                resolver = resolver.register_protocol_facts(&self.provider, facts.clone());
                continue;
            }
            match facts.metadata.source {
                ModelFactSource::ProviderAdapter => {
                    resolver = resolver.register_provider_facts(&self.provider, facts.clone());
                }
                ModelFactSource::ExactModel => {
                    resolver = resolver.register_exact_model_facts(
                        &self.provider,
                        &self.model_name,
                        facts.clone(),
                    );
                }
                ModelFactSource::CallerOverride => {
                    resolver = resolver.register_explicit_override(
                        &self.provider,
                        &self.model_name,
                        facts.clone(),
                    );
                }
                ModelFactSource::ConservativeUnknown | ModelFactSource::BuiltInCatalog => {}
            }
        }
        resolver.resolve_for_protocol_at(
            &self.provider,
            &self.model_name,
            self.api_protocol,
            self.endpoint.as_deref(),
            now,
        )
    }

    /// Supplement a fresh client resolution only where the client has no
    /// fresh fact for that source layer. Adapter protocol facts never cross
    /// clients; a current provider/model record cannot overwrite the same
    /// layer's newer client record merely because it is replayed later.
    pub fn with_missing_source_layers_from(self, current: &Self, now: SystemTime) -> Self {
        let fresh_caller_facts = self
            .registered_facts
            .iter()
            .filter(|facts| {
                facts.metadata.source == ModelFactSource::CallerOverride
                    && !self.protocol_facts.contains(facts)
                    && facts.is_fresh_at(now)
                    && facts.has_values()
            })
            .collect::<Vec<_>>();
        let fresh_has_layer = |source| {
            self.registered_facts.iter().any(|facts| {
                facts.metadata.source == source
                    && !self.protocol_facts.contains(facts)
                    && facts.is_fresh_at(now)
                    && facts.has_values()
            })
        };
        let mut resolver = ModelProfileResolver::new();
        for facts in &current.registered_facts {
            if current.protocol_facts.contains(facts) {
                continue;
            }
            if facts.metadata.source == ModelFactSource::CallerOverride {
                if let Some(complement) = caller_fact_complement(facts, &fresh_caller_facts) {
                    resolver = register_resolution_fact(
                        resolver,
                        &self.provider,
                        &self.model_name,
                        &complement,
                    );
                }
                continue;
            }
            if !fresh_has_layer(facts.metadata.source) {
                resolver =
                    register_resolution_fact(resolver, &self.provider, &self.model_name, facts);
            }
        }
        for facts in &self.registered_facts {
            if !facts.has_values() {
                continue;
            }
            resolver = if self.protocol_facts.contains(facts) {
                resolver.register_protocol_facts(&self.provider, facts.clone())
            } else {
                register_resolution_fact(resolver, &self.provider, &self.model_name, facts)
            };
        }
        resolver.resolve_for_protocol_at(
            &self.provider,
            &self.model_name,
            self.api_protocol,
            self.endpoint.as_deref(),
            now,
        )
    }

    /// Reapply higher-precedence records without freezing lower source facts.
    pub fn with_additional_facts(
        mut self,
        facts: impl IntoIterator<Item = ModelFactSet>,
        now: SystemTime,
    ) -> Self {
        for facts in facts {
            if self.registered_facts.iter().any(|existing| {
                existing.metadata == facts.metadata
                    && existing.capabilities == facts.capabilities
                    && existing.profile == facts.profile
            }) {
                continue;
            }
            apply_facts(
                &mut self.profile,
                &facts,
                now,
                &mut self.applied_facts,
                &mut self.ignored_stale_facts,
                &mut self.resolved_tokenizer_name,
            );
            self.registered_facts.push(facts);
        }
        for protocol in &self.protocol_facts {
            if protocol.is_fresh_at(now) {
                protocol.capabilities.apply_wire_to(&mut self.profile);
                if let Some(supports_streaming) = protocol.profile.supports_streaming {
                    self.profile.supports_streaming = supports_streaming;
                }
            }
        }
        self
    }
}

fn caller_fact_complement(current: &ModelFactSet, fresh: &[&ModelFactSet]) -> Option<ModelFactSet> {
    let mut capabilities = current.capabilities.clone();
    let mut profile = current.profile.clone();
    for facts in fresh {
        clear_capability_fields_covered_by(&mut capabilities, &facts.capabilities);
        clear_profile_fields_covered_by(&mut profile, &facts.profile);
    }
    let complement = ModelFactSet::new_partial(current.metadata.clone(), capabilities, profile);
    complement.has_values().then_some(complement)
}

fn clear_capability_fields_covered_by(
    current: &mut ProviderCapabilityOverride,
    fresh: &ProviderCapabilityOverride,
) {
    if fresh.streaming_tool_calls.is_some() {
        current.streaming_tool_calls = None;
    }
    if fresh.named_sse_events.is_some() {
        current.named_sse_events = None;
    }
    if fresh.reasoning_content.is_some() {
        current.reasoning_content = None;
    }
    if fresh.image_input.is_some() {
        current.image_input = None;
    }
    if fresh.system_as_top_level.is_some() {
        current.system_as_top_level = None;
    }
    if fresh.ndjson_streaming.is_some() {
        current.ndjson_streaming = None;
    }
    if fresh.tool_support.is_some() {
        current.tool_support = None;
    }
    if fresh.structured_output.is_some() {
        current.structured_output = None;
    }
    if fresh.requires_version_header.is_some() {
        current.requires_version_header = None;
    }
    if fresh.supports_parallel_tool_calls.is_some() {
        current.supports_parallel_tool_calls = None;
    }
    if fresh.supports_tool_choice_none.is_some() {
        current.supports_tool_choice_none = None;
    }
    if fresh.tokenizer_name.is_some() {
        current.tokenizer_name = None;
    }
}

fn clear_profile_fields_covered_by(
    current: &mut ModelProfileOverride,
    fresh: &ModelProfileOverride,
) {
    if fresh.supports_reasoning.is_some() {
        current.supports_reasoning = None;
    }
    if fresh.thinking_protocol.is_some() {
        current.thinking_protocol = None;
    }
    if fresh.supports_images.is_some() {
        current.supports_images = None;
    }
    if fresh.supports_tools.is_some() {
        current.supports_tools = None;
    }
    if fresh.max_output_tokens.is_some() {
        current.max_output_tokens = None;
    }
    if fresh.supports_streaming.is_some() {
        current.supports_streaming = None;
    }
    if fresh.supports_parallel_tool_calls.is_some() {
        current.supports_parallel_tool_calls = None;
    }
    if fresh.supports_tool_choice_none.is_some() {
        current.supports_tool_choice_none = None;
    }
    if fresh.supports_structured_output.is_some() {
        current.supports_structured_output = None;
    }
    if fresh.context_window.is_some() {
        current.context_window = None;
    }
    if !fresh.excluded_tools.is_empty() {
        current.excluded_tools = current
            .excluded_tools
            .difference(&fresh.excluded_tools)
            .cloned()
            .collect();
    }
    if fresh.prompt_suffix.is_some() {
        current.prompt_suffix = None;
    }
}

fn register_resolution_fact(
    resolver: ModelProfileResolver,
    provider: &str,
    model: &str,
    facts: &ModelFactSet,
) -> ModelProfileResolver {
    match facts.metadata.source {
        ModelFactSource::ProviderAdapter => {
            resolver.register_provider_facts(provider, facts.clone())
        }
        ModelFactSource::ExactModel => {
            resolver.register_exact_model_facts(provider, model, facts.clone())
        }
        ModelFactSource::CallerOverride => {
            resolver.register_explicit_override(provider, model, facts.clone())
        }
        ModelFactSource::ConservativeUnknown | ModelFactSource::BuiltInCatalog => resolver,
    }
}

/// Resolves the single authoritative model profile from sourced facts.
///
/// Precedence is conservative unknown, versioned built-in facts, fresh
/// provider facts, fresh exact-model facts, then fresh exact caller override.
#[derive(Debug, Clone, Default)]
pub struct ModelProfileResolver {
    provider_facts: HashMap<String, ModelFactSet>,
    protocol_facts: HashMap<String, ModelFactSet>,
    exact_model_facts: HashMap<String, ModelFactSet>,
    explicit_overrides: HashMap<String, Vec<ModelFactSet>>,
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
        self.provider_facts.insert(
            normalize_selector_part(provider.as_ref()),
            ModelFactSet::new(
                legacy_metadata(
                    ModelFactSource::ProviderAdapter,
                    "register_provider_default",
                ),
                None,
                profile,
            ),
        );
        self
    }

    pub fn register_exact(
        mut self,
        provider: impl AsRef<str>,
        model: impl AsRef<str>,
        profile: ModelProfileOverride,
    ) -> Self {
        self.explicit_overrides
            .entry(selector_key(provider.as_ref(), model.as_ref()))
            .or_default()
            .push(ModelFactSet::new(
                legacy_metadata(ModelFactSource::CallerOverride, "register_exact"),
                None,
                profile,
            ));
        self
    }

    /// Register a sourced provider-wide fact set.
    pub fn register_provider_facts(
        mut self,
        provider: impl AsRef<str>,
        facts: ModelFactSet,
    ) -> Self {
        self.provider_facts.insert(
            normalize_selector_part(provider.as_ref()),
            facts.into_source(ModelFactSource::ProviderAdapter),
        );
        self
    }

    /// Register immutable wire facts owned by the selected adapter.
    pub fn register_protocol_facts(
        mut self,
        provider: impl AsRef<str>,
        facts: ModelFactSet,
    ) -> Self {
        self.protocol_facts.insert(
            normalize_selector_part(provider.as_ref()),
            facts.into_source(ModelFactSource::ProviderAdapter),
        );
        self
    }

    /// Register facts observed for one exact normalized provider/model pair.
    pub fn register_exact_model_facts(
        mut self,
        provider: impl AsRef<str>,
        model: impl AsRef<str>,
        facts: ModelFactSet,
    ) -> Self {
        self.exact_model_facts.insert(
            selector_key(provider.as_ref(), model.as_ref()),
            facts.into_source(ModelFactSource::ExactModel),
        );
        self
    }

    /// Register an explicit caller override for one exact provider/model pair.
    pub fn register_explicit_override(
        mut self,
        provider: impl AsRef<str>,
        model: impl AsRef<str>,
        facts: ModelFactSet,
    ) -> Self {
        self.explicit_overrides
            .entry(selector_key(provider.as_ref(), model.as_ref()))
            .or_default()
            .push(facts.into_source(ModelFactSource::CallerOverride));
        self
    }

    /// Compatibility resolver for a caller that already holds provider facts.
    ///
    /// The bare capability value is converted immediately into a sourced,
    /// non-expiring provider record. New integrations should register a
    /// [`ModelFactSet`] and call [`Self::resolve_at`].
    pub fn resolve(
        &self,
        provider: &str,
        model: &str,
        capabilities: ProviderCapabilities,
    ) -> ModelProfile {
        let now = SystemTime::now();
        let direct_provider_facts = ModelFactSet::new(
            ModelFactMetadata::new(
                ModelFactSource::ProviderAdapter,
                "ModelProfileResolver::resolve capabilities argument",
                "legacy-direct-provider-capabilities-v1",
                now,
                None,
                ModelFactConfidence::VERIFIED,
            ),
            Some(capabilities),
            ModelProfileOverride {
                supports_streaming: Some(
                    capabilities != ProviderCapabilities::conservative_unknown(),
                ),
                ..Default::default()
            },
        );
        self.resolve_inner(
            provider,
            model,
            LlmApiProtocol::ChatCompletions,
            None,
            now,
            Some(&direct_provider_facts),
        )
        .profile
    }

    /// Resolve sourced facts at an explicit time and return provenance details.
    pub fn resolve_at(
        &self,
        provider: &str,
        model: &str,
        now: SystemTime,
    ) -> ModelProfileResolution {
        self.resolve_inner(
            provider,
            model,
            LlmApiProtocol::ChatCompletions,
            None,
            now,
            None,
        )
    }

    /// Resolve facts for a concrete provider wire protocol and endpoint.
    pub fn resolve_for_protocol_at(
        &self,
        provider: &str,
        model: &str,
        api_protocol: LlmApiProtocol,
        endpoint: Option<&str>,
        now: SystemTime,
    ) -> ModelProfileResolution {
        self.resolve_inner(provider, model, api_protocol, endpoint, now, None)
    }

    fn resolve_inner(
        &self,
        provider: &str,
        model: &str,
        api_protocol: LlmApiProtocol,
        endpoint: Option<&str>,
        now: SystemTime,
        direct_provider_facts: Option<&ModelFactSet>,
    ) -> ModelProfileResolution {
        let mut applied_facts = vec![conservative_unknown_metadata()];
        let mut ignored_stale_facts = Vec::new();
        let mut profile = ModelProfile::conservative_unknown(model, provider);
        let mut resolved_tokenizer_name = None;
        let mut registered_facts = Vec::new();
        let mut retained_protocol_facts = Vec::new();
        if has_built_in_facts(provider, model, api_protocol, endpoint) {
            let metadata = built_in_metadata();
            if metadata.is_fresh_at(now) {
                let capabilities =
                    ProviderCapabilities::built_in_for_protocol(provider, api_protocol)
                        .unwrap_or_else(ProviderCapabilities::conservative_unknown);
                profile = ModelProfile::new_for_protocol(
                    model,
                    provider,
                    capabilities,
                    api_protocol,
                    endpoint,
                );
                resolved_tokenizer_name = profile.tokenizer_name.map(str::to_string);
                applied_facts.push(metadata);
            } else {
                ignored_stale_facts.push(metadata);
            }
        }

        if let Some(facts) = direct_provider_facts {
            registered_facts.push(facts.clone());
            apply_facts(
                &mut profile,
                facts,
                now,
                &mut applied_facts,
                &mut ignored_stale_facts,
                &mut resolved_tokenizer_name,
            );
        }
        if let Some(facts) = self.provider_facts.get(&normalize_selector_part(provider)) {
            registered_facts.push(facts.clone());
            apply_facts(
                &mut profile,
                facts,
                now,
                &mut applied_facts,
                &mut ignored_stale_facts,
                &mut resolved_tokenizer_name,
            );
        }
        if let Some(facts) = self.protocol_facts.get(&normalize_selector_part(provider)) {
            registered_facts.push(facts.clone());
            retained_protocol_facts.push(facts.clone());
            apply_facts(
                &mut profile,
                facts,
                now,
                &mut applied_facts,
                &mut ignored_stale_facts,
                &mut resolved_tokenizer_name,
            );
        }
        if let Some(facts) = self.exact_model_facts.get(&selector_key(provider, model)) {
            registered_facts.push(facts.clone());
            apply_facts(
                &mut profile,
                facts,
                now,
                &mut applied_facts,
                &mut ignored_stale_facts,
                &mut resolved_tokenizer_name,
            );
        }
        if let Some(overrides) = self.explicit_overrides.get(&selector_key(provider, model)) {
            for facts in overrides {
                registered_facts.push(facts.clone());
                apply_facts(
                    &mut profile,
                    facts,
                    now,
                    &mut applied_facts,
                    &mut ignored_stale_facts,
                    &mut resolved_tokenizer_name,
                );
            }
        }
        if let Some(facts) = self.protocol_facts.get(&normalize_selector_part(provider))
            && facts.is_fresh_at(now)
        {
            facts.capabilities.apply_wire_to(&mut profile);
            if let Some(supports_streaming) = facts.profile.supports_streaming {
                profile.supports_streaming = supports_streaming;
            }
        }

        ModelProfileResolution {
            profile,
            resolved_tokenizer_name,
            applied_facts,
            ignored_stale_facts,
            registered_facts,
            protocol_facts: retained_protocol_facts,
            provider: provider.to_string(),
            model_name: model.to_string(),
            api_protocol,
            endpoint: endpoint.map(str::to_string),
        }
    }
}

fn apply_facts(
    profile: &mut ModelProfile,
    facts: &ModelFactSet,
    now: SystemTime,
    applied_facts: &mut Vec<ModelFactMetadata>,
    ignored_stale_facts: &mut Vec<ModelFactMetadata>,
    resolved_tokenizer_name: &mut Option<String>,
) {
    if !facts.has_values() {
        return;
    }
    if !facts.is_fresh_at(now) {
        ignored_stale_facts.push(facts.metadata.clone());
        return;
    }
    facts.capabilities.apply_to(profile);
    if let Some(tokenizer_name) = &facts.capabilities.tokenizer_name {
        resolved_tokenizer_name.clone_from(&Some(tokenizer_name.clone()));
    }
    facts.profile.apply_to(profile);
    applied_facts.push(facts.metadata.clone());
}

fn conservative_unknown_metadata() -> ModelFactMetadata {
    ModelFactMetadata::new(
        ModelFactSource::ConservativeUnknown,
        "echo-core conservative unknown",
        "conservative-unknown-v1",
        SystemTime::UNIX_EPOCH,
        None,
        ModelFactConfidence::UNKNOWN,
    )
}

fn built_in_metadata() -> ModelFactMetadata {
    ModelFactMetadata::new(
        ModelFactSource::BuiltInCatalog,
        "echo-core/src/llm/capabilities.rs",
        BUILT_IN_MODEL_FACTS_VERSION,
        SystemTime::UNIX_EPOCH + Duration::from_secs(BUILT_IN_MODEL_FACTS_OBSERVED_AT),
        None,
        ModelFactConfidence::from_percent_saturating(60),
    )
}

fn has_built_in_facts(
    provider: &str,
    _model: &str,
    api_protocol: LlmApiProtocol,
    _endpoint: Option<&str>,
) -> bool {
    // Model-family naming alone is not authority. A gateway with an unknown
    // provider label must submit explicit facts instead of inheriting a
    // built-in model catalog entry by string resemblance.
    if ProviderCapabilities::built_in_for_protocol(provider, api_protocol).is_none() {
        return false;
    }
    true
}

fn legacy_metadata(source: ModelFactSource, entry_point: &str) -> ModelFactMetadata {
    ModelFactMetadata::new(
        source,
        format!("ModelProfileResolver::{entry_point}"),
        "legacy-registration-v1",
        SystemTime::UNIX_EPOCH,
        None,
        ModelFactConfidence::VERIFIED,
    )
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
    use std::time::{Duration, SystemTime};

    fn timestamp(seconds: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH
            + Duration::from_secs(BUILT_IN_MODEL_FACTS_OBSERVED_AT.saturating_add(seconds))
    }

    fn metadata(
        source: ModelFactSource,
        version: &str,
        observed_at: u64,
        expires_at: Option<u64>,
    ) -> ModelFactMetadata {
        ModelFactMetadata::new(
            source,
            "test-suite",
            version,
            timestamp(observed_at),
            expires_at.map(timestamp),
            ModelFactConfidence::from_percent_saturating(90),
        )
    }

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
    fn historical_provider_labels_keep_openai_compatible_defaults() {
        for provider in [
            "deepseek",
            "dashscope",
            "qwen",
            "aliyun",
            "moonshot",
            "zhipu",
            "google",
        ] {
            assert_eq!(
                ProviderCapabilities::from_provider_name(provider),
                ProviderCapabilities::openai_compatible(),
                "provider label {provider} lost its compatibility baseline"
            );
        }
    }

    #[test]
    fn unknown_provider_does_not_activate_known_model_catalog_facts() {
        let resolution = ModelProfileResolver::new().resolve_at(
            "unclassified-gateway",
            "gpt-5.6-sol",
            timestamp(50),
        );

        assert_eq!(
            resolution.profile.capabilities,
            ProviderCapabilities::conservative_unknown()
        );
        assert_eq!(resolution.profile.thinking_protocol, ThinkingProtocol::None);
        assert_eq!(resolution.profile.max_output_tokens, None);
        assert_eq!(resolution.profile.context_window, None);
    }

    #[test]
    fn built_in_catalog_requires_provider_protocol_compatibility() {
        assert_eq!(
            ProviderCapabilities::built_in_for_protocol("anthropic", LlmApiProtocol::Anthropic),
            Some(ProviderCapabilities::anthropic())
        );
        assert_eq!(
            ProviderCapabilities::built_in_for_protocol(
                "anthropic",
                LlmApiProtocol::ChatCompletions
            ),
            None
        );
        assert_eq!(
            ProviderCapabilities::built_in_for_protocol("openai", LlmApiProtocol::Anthropic),
            None
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

    #[test]
    fn sourced_facts_resolve_in_canonical_precedence_order() {
        let mut provider_capabilities = ProviderCapabilities::openai_compatible();
        provider_capabilities.tokenizer_name = Some("provider-tokenizer-v2");
        let resolver = ModelProfileResolver::new()
            .register_provider_facts(
                "openai",
                ModelFactSet::new(
                    metadata(
                        ModelFactSource::ProviderAdapter,
                        "provider-v1",
                        10,
                        Some(100),
                    ),
                    Some(provider_capabilities),
                    ModelProfileOverride {
                        context_window: Some(64_000),
                        max_output_tokens: Some(8_000),
                        supports_structured_output: Some(true),
                        ..Default::default()
                    },
                ),
            )
            .register_exact_model_facts(
                "openai",
                "gpt-5-test",
                ModelFactSet::new(
                    metadata(ModelFactSource::ExactModel, "model-v2", 20, Some(100)),
                    None,
                    ModelProfileOverride {
                        context_window: Some(32_000),
                        max_output_tokens: Some(4_000),
                        thinking_protocol: Some(ThinkingProtocol::ModelManaged),
                        supports_structured_output: Some(false),
                        ..Default::default()
                    },
                ),
            )
            .register_explicit_override(
                "openai",
                "gpt-5-test",
                ModelFactSet::new(
                    metadata(ModelFactSource::CallerOverride, "config-v3", 30, None),
                    None,
                    ModelProfileOverride {
                        context_window: Some(16_000),
                        max_output_tokens: Some(2_000),
                        thinking_protocol: Some(ThinkingProtocol::None),
                        supports_structured_output: Some(true),
                        ..Default::default()
                    },
                ),
            );

        let resolution = resolver.resolve_at("openai", "gpt-5-test", timestamp(50));
        assert_eq!(resolution.profile.context_window, Some(16_000));
        assert_eq!(resolution.profile.max_output_tokens, Some(2_000));
        assert_eq!(resolution.profile.thinking_protocol, ThinkingProtocol::None);
        assert_eq!(
            resolution.resolved_tokenizer_name.as_deref(),
            Some("provider-tokenizer-v2")
        );
        assert!(resolution.profile.capabilities.structured_output);
        assert_eq!(
            resolution
                .applied_facts
                .iter()
                .map(|metadata| metadata.source)
                .collect::<Vec<_>>(),
            vec![
                ModelFactSource::ConservativeUnknown,
                ModelFactSource::BuiltInCatalog,
                ModelFactSource::ProviderAdapter,
                ModelFactSource::ExactModel,
                ModelFactSource::CallerOverride,
            ]
        );
        assert!(resolution.ignored_stale_facts.is_empty());
    }

    #[test]
    fn stale_or_future_facts_cannot_elevate_unknown_capabilities() {
        let stale_provider = ModelFactSet::new(
            metadata(ModelFactSource::ProviderAdapter, "stale", 10, Some(20)),
            Some(ProviderCapabilities::openai_compatible()),
            ModelProfileOverride {
                supports_structured_output: Some(true),
                context_window: Some(1_000_000),
                max_output_tokens: Some(100_000),
                ..Default::default()
            },
        );
        let future_exact = ModelFactSet::new(
            metadata(ModelFactSource::ExactModel, "future", 80, Some(100)),
            None,
            ModelProfileOverride {
                supports_parallel_tool_calls: Some(true),
                ..Default::default()
            },
        );
        let resolution = ModelProfileResolver::new()
            .register_provider_facts("custom", stale_provider)
            .register_exact_model_facts("custom", "future-model", future_exact)
            .resolve_at("custom", "future-model", timestamp(50));

        assert_eq!(
            resolution.profile.capabilities,
            ProviderCapabilities::conservative_unknown()
        );
        assert!(!resolution.profile.supports_tools);
        assert!(!resolution.profile.supports_parallel_tool_calls);
        assert_eq!(resolution.profile.context_window, None);
        assert_eq!(resolution.profile.max_output_tokens, None);
        assert_eq!(resolution.ignored_stale_facts.len(), 2);
    }

    #[test]
    fn built_in_fallback_is_versioned_and_unknown_provider_is_conservative() -> Result<(), String> {
        let openai = ModelProfileResolver::new().resolve_at("openai", "gpt-5", timestamp(50));
        let built_in = openai
            .applied_facts
            .iter()
            .find(|metadata| metadata.source == ModelFactSource::BuiltInCatalog)
            .ok_or_else(|| "built-in catalog metadata missing".to_string())?;
        assert!(!built_in.version.is_empty());
        assert!(!built_in.provenance.is_empty());

        let unknown = ModelProfileResolver::new().resolve_at(
            "unknown-provider",
            "future-model",
            timestamp(50),
        );
        assert_eq!(
            unknown.profile.capabilities,
            ProviderCapabilities::conservative_unknown()
        );
        Ok(())
    }

    #[test]
    fn confidence_percent_is_bounded_without_panicking() {
        assert_eq!(
            ModelFactConfidence::from_percent_saturating(101).percent(),
            100
        );
        assert_eq!(
            ModelFactConfidence::from_percent_saturating(42).percent(),
            42
        );
    }

    #[test]
    fn registration_scope_normalizes_untrusted_source_labels() {
        let mislabeled = |source| {
            ModelFactSet::new(
                metadata(source, "mislabeled", 10, Some(100)),
                None,
                ModelProfileOverride {
                    context_window: Some(4_096),
                    ..Default::default()
                },
            )
        };
        let resolution = ModelProfileResolver::new()
            .register_provider_facts("custom", mislabeled(ModelFactSource::CallerOverride))
            .register_exact_model_facts(
                "custom",
                "model-a",
                mislabeled(ModelFactSource::ProviderAdapter),
            )
            .register_explicit_override(
                "custom",
                "model-a",
                mislabeled(ModelFactSource::ProviderAdapter),
            )
            .resolve_at("custom", "model-a", timestamp(50));

        assert_eq!(
            resolution
                .applied_facts
                .iter()
                .map(|metadata| metadata.source)
                .collect::<Vec<_>>(),
            vec![
                ModelFactSource::ConservativeUnknown,
                ModelFactSource::ProviderAdapter,
                ModelFactSource::ExactModel,
                ModelFactSource::CallerOverride,
            ]
        );
    }

    #[test]
    fn built_in_observation_time_and_empty_records_are_honored() {
        let before_catalog = SystemTime::UNIX_EPOCH
            + Duration::from_secs(BUILT_IN_MODEL_FACTS_OBSERVED_AT.saturating_sub(1));
        let resolution = ModelProfileResolver::new()
            .register_provider_facts(
                "openai",
                ModelFactSet::new(
                    metadata(ModelFactSource::ProviderAdapter, "empty", 10, None),
                    None,
                    ModelProfileOverride::default(),
                ),
            )
            .resolve_at("openai", "gpt-5", before_catalog);

        assert_eq!(
            resolution
                .applied_facts
                .iter()
                .map(|metadata| metadata.source)
                .collect::<Vec<_>>(),
            vec![ModelFactSource::ConservativeUnknown]
        );
        assert_eq!(
            resolution
                .ignored_stale_facts
                .iter()
                .map(|metadata| metadata.source)
                .collect::<Vec<_>>(),
            vec![ModelFactSource::BuiltInCatalog]
        );
        assert_eq!(
            resolution.profile.capabilities,
            ProviderCapabilities::conservative_unknown()
        );
    }

    #[test]
    fn refresh_at_rejects_facts_that_expired_after_construction() {
        let resolution = ModelProfileResolver::new()
            .register_exact_model_facts(
                "openai",
                "future-model",
                ModelFactSet::new(
                    metadata(ModelFactSource::ExactModel, "expiring", 10, Some(20)),
                    None,
                    ModelProfileOverride {
                        context_window: Some(32_000),
                        max_output_tokens: Some(4_096),
                        ..Default::default()
                    },
                ),
            )
            .resolve_at("openai", "future-model", timestamp(15));
        assert_eq!(resolution.profile.context_window, Some(32_000));

        let refreshed = resolution.refresh_at(timestamp(25));
        assert_eq!(refreshed.profile.context_window, None);
        assert_eq!(refreshed.profile.max_output_tokens, None);
        assert!(
            refreshed
                .ignored_stale_facts
                .iter()
                .any(|metadata| metadata.version == "expiring")
        );
    }

    #[test]
    fn profile_and_capability_views_remain_consistent_after_override() {
        let resolution = ModelProfileResolver::new()
            .register_explicit_override(
                "custom",
                "model-a",
                ModelFactSet::new(
                    metadata(ModelFactSource::CallerOverride, "config", 10, None),
                    None,
                    ModelProfileOverride {
                        supports_reasoning: Some(true),
                        thinking_protocol: Some(ThinkingProtocol::ModelManaged),
                        supports_images: Some(true),
                        supports_tools: Some(true),
                        supports_parallel_tool_calls: Some(true),
                        supports_tool_choice_none: Some(true),
                        supports_structured_output: Some(true),
                        ..Default::default()
                    },
                ),
            )
            .resolve_at("custom", "model-a", timestamp(50));
        let profile = resolution.profile;

        assert_eq!(
            profile.supports_reasoning,
            profile.capabilities.reasoning_content
        );
        assert_eq!(profile.supports_images, profile.capabilities.image_input);
        assert_eq!(profile.supports_tools, profile.capabilities.tool_support);
        assert_eq!(
            profile.supports_parallel_tool_calls,
            profile.capabilities.supports_parallel_tool_calls
        );
        assert_eq!(
            profile.supports_tool_choice_none,
            profile.capabilities.supports_tool_choice_none
        );
        assert!(profile.capabilities.structured_output);
        assert_eq!(profile.thinking_levels, &[]);
    }

    #[test]
    fn explicit_model_flags_survive_family_reconciliation() {
        let resolution = ModelProfileResolver::new()
            .register_explicit_override(
                "openai",
                "o3-mini",
                ModelFactSet::new(
                    metadata(ModelFactSource::CallerOverride, "model-config", 10, None),
                    None,
                    ModelProfileOverride {
                        supports_images: Some(true),
                        supports_reasoning: Some(false),
                        ..Default::default()
                    },
                ),
            )
            .resolve_at("openai", "o3-mini", timestamp(50));

        assert!(resolution.profile.supports_images);
        assert!(!resolution.profile.supports_reasoning);
    }

    #[test]
    fn model_profile_public_borrowed_fields_remain_source_compatible() {
        let profile = ModelProfile::from_provider_name("gpt-5", "openai");
        let _: &'static [ThinkingLevel] = profile.thinking_levels;
        let _: Option<&'static str> = profile.tokenizer_name;
    }

    #[test]
    fn exact_and_caller_facts_cannot_rewrite_adapter_wire_baseline() {
        let protocol = ModelFactSet::new_partial(
            metadata(ModelFactSource::ProviderAdapter, "protocol", 10, None),
            ProviderCapabilityOverride::openai_chat_protocol(),
            ModelProfileOverride {
                supports_streaming: Some(true),
                ..Default::default()
            },
        );
        let caller = ModelFactSet::new(
            metadata(ModelFactSource::CallerOverride, "caller", 20, None),
            Some(ProviderCapabilities::anthropic()),
            ModelProfileOverride {
                supports_streaming: Some(false),
                ..Default::default()
            },
        );
        let resolution = ModelProfileResolver::new()
            .register_protocol_facts("custom", protocol.clone())
            .register_explicit_override("custom", "model-a", caller.clone())
            .resolve_at("custom", "model-a", timestamp(50));

        assert!(resolution.profile.capabilities.streaming_tool_calls);
        assert!(!resolution.profile.capabilities.named_sse_events);
        assert!(!resolution.profile.capabilities.system_as_top_level);
        assert!(!resolution.profile.capabilities.ndjson_streaming);
        assert!(!resolution.profile.capabilities.requires_version_header);
        assert!(resolution.profile.supports_streaming);

        let merged = ModelProfileResolver::new()
            .register_protocol_facts("custom", protocol)
            .resolve_at("custom", "model-a", timestamp(50))
            .with_additional_facts([caller], timestamp(50));
        assert!(merged.profile.capabilities.streaming_tool_calls);
        assert!(!merged.profile.capabilities.named_sse_events);
        assert!(!merged.profile.capabilities.system_as_top_level);
        assert!(merged.profile.supports_streaming);
    }

    #[test]
    fn fact_sets_round_trip_and_deserialized_confidence_stays_bounded() -> Result<(), String> {
        let facts = ModelFactSet::new_partial(
            metadata(ModelFactSource::ExactModel, "serialized-v1", 10, Some(100)),
            ProviderCapabilityOverride {
                tool_support: Some(true),
                tokenizer_name: Some("custom-tokenizer".to_string()),
                ..Default::default()
            },
            ModelProfileOverride {
                context_window: Some(64_000),
                thinking_protocol: Some(ThinkingProtocol::ModelManaged),
                ..Default::default()
            },
        );
        let encoded = serde_json::to_string(&facts).map_err(|error| error.to_string())?;
        let decoded =
            serde_json::from_str::<ModelFactSet>(&encoded).map_err(|error| error.to_string())?;
        assert_eq!(decoded, facts);

        let confidence = serde_json::from_str::<ModelFactConfidence>("255")
            .map_err(|error| error.to_string())?;
        assert_eq!(confidence.percent(), 100);
        Ok(())
    }
}
