from __future__ import annotations

from dataclasses import dataclass, replace

from .provider_capabilities_values import ProviderCapabilities
from .thinking import ThinkingLevel
from .thinking_profile_values import resolve_thinking_profile
from .thinking_protocol_values import ThinkingProtocol


def infer_context_window(provider: str, model_name: str) -> int | None:
    provider_lower = provider.strip().lower()
    model = model_name.lower()
    if provider_lower in {"openai", "azure-openai"} and model.startswith("gpt-5.6"):
        return 1_050_000
    if (
        provider_lower == "anthropic"
        and model.startswith(("claude-fable-5", "claude-opus-4-8", "claude-sonnet-5"))
        or provider_lower == "deepseek"
        and model.startswith("deepseek-v4")
        or provider_lower in {"dashscope", "qwen", "aliyun", "alibaba"}
        and model.startswith(("qwen3.7-max", "qwen3.7-plus"))
        or provider_lower == "zhipu"
        and model.startswith("glm-5.2")
    ):
        return 1_000_000
    if provider_lower == "moonshot" and model.startswith(("kimi-k2.7", "kimi-k2.6")):
        return 256_000
    return None


@dataclass(frozen=True)
class ModelProfile:
    provider: str
    model_name: str
    capabilities: ProviderCapabilities
    supports_reasoning: bool
    thinking_protocol: ThinkingProtocol
    thinking_levels: tuple[ThinkingLevel, ...]
    supports_images: bool
    supports_tools: bool
    max_output_tokens: int | None
    supports_streaming: bool
    supports_parallel_tool_calls: bool
    supports_tool_choice_none: bool
    context_window: int | None
    excluded_tools: frozenset[str]
    prompt_suffix: str | None
    tokenizer_name: str | None

    @classmethod
    def new(
        cls,
        model_name: str,
        provider: str,
        capabilities: ProviderCapabilities,
    ) -> ModelProfile:
        lower = model_name.lower()
        thinking = resolve_thinking_profile(provider, model_name)
        max_output_tokens = (
            131_072
            if "qwen3-235b" in lower
            else 16_384
            if lower.startswith(("gpt-5", "o3", "o4"))
            else 8_192
            if lower.startswith("claude-")
            else None
        )
        tokenizer_name = (
            "o200k_base"
            if lower.startswith(("gpt-5", "gpt-4.5"))
            else "cl100k_base"
            if lower.startswith(("gpt-4", "gpt-3"))
            else capabilities.tokenizer_name
        )
        return cls(
            provider=provider,
            model_name=model_name,
            capabilities=capabilities,
            supports_reasoning=(
                thinking.protocol is not ThinkingProtocol.NONE
                and capabilities.reasoning_content
            ),
            thinking_protocol=thinking.protocol,
            thinking_levels=tuple(thinking.levels),
            supports_images=(
                capabilities.image_input
                and not lower.startswith(("o3-mini", "o1-mini", "o1-preview"))
            ),
            supports_tools=capabilities.tool_support,
            max_output_tokens=max_output_tokens,
            supports_streaming=True,
            supports_parallel_tool_calls=capabilities.supports_parallel_tool_calls,
            supports_tool_choice_none=capabilities.supports_tool_choice_none,
            context_window=infer_context_window(provider, model_name),
            excluded_tools=frozenset(),
            prompt_suffix=None,
            tokenizer_name=tokenizer_name,
        )

    @classmethod
    def from_provider_name(cls, model_name: str, provider: str) -> ModelProfile:
        return cls.new(
            model_name, provider, ProviderCapabilities.from_provider_name(provider)
        )


@dataclass(frozen=True)
class ModelProfileOverride:
    supports_parallel_tool_calls: bool | None = None
    supports_tool_choice_none: bool | None = None
    supports_structured_output: bool | None = None
    context_window: int | None = None
    excluded_tools: frozenset[str] = frozenset()
    prompt_suffix: str | None = None

    def __post_init__(self) -> None:
        object.__setattr__(self, "excluded_tools", frozenset(self.excluded_tools))


def _normalize(value: str) -> str:
    return value.strip().lower()


def _selector_key(provider: str, model: str) -> str:
    return f"{_normalize(provider)}:{_normalize(model)}"


def _apply_override(
    profile: ModelProfile, override: ModelProfileOverride
) -> ModelProfile:
    capabilities = profile.capabilities
    if override.supports_structured_output is not None:
        capabilities = replace(
            capabilities, structured_output=override.supports_structured_output
        )
    return replace(
        profile,
        capabilities=capabilities,
        supports_parallel_tool_calls=(
            override.supports_parallel_tool_calls
            if override.supports_parallel_tool_calls is not None
            else profile.supports_parallel_tool_calls
        ),
        supports_tool_choice_none=(
            override.supports_tool_choice_none
            if override.supports_tool_choice_none is not None
            else profile.supports_tool_choice_none
        ),
        context_window=(
            override.context_window
            if override.context_window is not None
            else profile.context_window
        ),
        excluded_tools=profile.excluded_tools | override.excluded_tools,
        prompt_suffix=(
            override.prompt_suffix
            if override.prompt_suffix is not None
            else profile.prompt_suffix
        ),
    )


class ModelProfileResolver:
    def __init__(self) -> None:
        self._provider_defaults: dict[str, ModelProfileOverride] = {}
        self._exact_models: dict[str, ModelProfileOverride] = {}

    @classmethod
    def new(cls) -> ModelProfileResolver:
        return cls()

    def register_provider_default(
        self, provider: str, profile: ModelProfileOverride
    ) -> ModelProfileResolver:
        self._provider_defaults[_normalize(provider)] = profile
        return self

    def register_exact(
        self, provider: str, model: str, profile: ModelProfileOverride
    ) -> ModelProfileResolver:
        self._exact_models[_selector_key(provider, model)] = profile
        return self

    def resolve(
        self, provider: str, model: str, capabilities: ProviderCapabilities
    ) -> ModelProfile:
        profile = ModelProfile.new(model, provider, capabilities)
        provider_default = self._provider_defaults.get(_normalize(provider))
        if provider_default is not None:
            profile = _apply_override(profile, provider_default)
        exact = self._exact_models.get(_selector_key(provider, model))
        if exact is not None:
            profile = _apply_override(profile, exact)
        return profile
