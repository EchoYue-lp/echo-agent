//! Provider adapter trait — encapsulates per-vendor behaviour.
//!
//! Each LLM provider has different:
//! - Thinking wire protocol (reasoning_effort vs enable_thinking vs thinking.type)
//! - Cache key sensitivity (user_id, system prompt, tools)
//! - Request fields (DeepSeek user_id for KVCache, GLM auth, etc.)
//! - Response parsing (cache tokens in different fields)
//!
//! This trait captures these differences so each provider can have its own
//! implementation, rather than a giant match-on-provider-string in one file.

use echo_core::llm::capabilities::CachePolicy;
use echo_core::llm::types::ChatCompletionRequest;

/// Hooks a provider can use to customise the request/response cycle without
/// reimplementing the HTTP layer.
///
/// Each provider declares its behaviour here — auth, endpoints, thinking
/// protocol, cache policy, and any request-level quirks. The transport
/// (`AdapterClient`) reads these declarations instead of receiving 20+
/// boolean flags. This mirrors Hermes's `ProviderProfile` pattern.
pub trait ProviderAdapter: Send + Sync {
    // ── Identity ─────────────────────────────────────────
    /// Provider identifier string (e.g. "deepseek", "zhipu").
    fn provider_name(&self) -> &str;

    // ── Endpoint ─────────────────────────────────────────
    /// Base URL for the chat completions endpoint.
    fn base_url(&self) -> &str;

    /// Environment variable that overrides the base URL (e.g. "DEEPSEEK_BASE_URL").
    /// Users set this to point at plan-specific endpoints (coding plan, agent plan,
    /// token plan) or self-hosted deployments.
    fn base_url_env_var(&self) -> &str {
        ""
    }

    // ── Auth ─────────────────────────────────────────────
    /// Environment variable names to try for the API key.
    fn api_key_env_vars(&self) -> &[&str] {
        &[]
    }

    // ── Thinking protocol ────────────────────────────────
    /// Whether this provider uses `reasoning_effort` (OpenAI/DeepSeek style),
    /// `enable_thinking` (Qwen style), `thinking.type` (GLM style), or none.
    fn thinking_protocol(&self) -> ThinkingProtocolPreference {
        ThinkingProtocolPreference::OpenAiReasoningEffort
    }

    /// Whether this provider drops temperature when thinking is engaged.
    fn drops_temperature_on_thinking(&self) -> bool {
        matches!(
            self.thinking_protocol(),
            ThinkingProtocolPreference::OpenAiReasoningEffort
        )
    }

    // ── Request customisation ────────────────────────────
    /// Customise the request body before it is sent.
    /// Called after the standard OpenAI-compatible request is built.
    fn prepare_request(&self, _req: &mut ChatCompletionRequest) {}

    // ── Cache ────────────────────────────────────────────
    /// Return the cache policy for this provider.
    fn cache_policy(&self) -> CachePolicy {
        CachePolicy::default()
    }
}

/// Resolve the actual base URL for a provider, checking the overridden
/// environment variable first, then falling back to the adapter's default.
///
/// This is the equivalent of Hermes's `HermesOverlay.base_url_env_var` pattern:
/// users set e.g. `DEEPSEEK_BASE_URL=https://api.deepseek.com/agent` for plan-
/// specific endpoints or self-hosted deployments.
pub fn resolve_base_url<A: ProviderAdapter + ?Sized>(adapter: &A) -> String {
    let env_var = adapter.base_url_env_var();
    if !env_var.is_empty() {
        if let Ok(url) = std::env::var(env_var) {
            let trimmed = url.trim();
            if !trimmed.is_empty() {
                tracing::info!(%env_var, url = %trimmed, provider = %adapter.provider_name(),
                    "using env-var-overridden base URL");
                return trimmed.to_string();
            }
        }
    }
    adapter.base_url().to_string()
}

/// Which thinking wire protocol a provider uses.
///
/// Different providers use incompatible fields for the same "thinking depth"
/// concept. This enum tells the transport which field to populate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingProtocolPreference {
    /// `reasoning_effort` field (OpenAI, Azure)
    OpenAiReasoningEffort,
    /// `enable_thinking` + optional `thinking_budget` (Qwen3, DashScope)
    EnableThinkingFlag,
    /// `reasoning_effort` + `thinking:{type}` combo (GLM-5.x)
    GlmReasoningEffort,
    /// Both `reasoning_effort` AND `thinking:{type}` (DeepSeek requires both)
    DeepSeekDual,
    /// No thinking control — model decides (Kimi)
    None,
}
