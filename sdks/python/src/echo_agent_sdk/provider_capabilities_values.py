from __future__ import annotations

from dataclasses import dataclass


@dataclass(frozen=True)
class ProviderCapabilities:
    streaming_tool_calls: bool
    named_sse_events: bool
    reasoning_content: bool
    image_input: bool
    system_as_top_level: bool
    ndjson_streaming: bool
    tool_support: bool
    structured_output: bool
    requires_version_header: bool
    supports_parallel_tool_calls: bool
    supports_tool_choice_none: bool
    tokenizer_name: str | None = None

    @classmethod
    def openai_compatible(cls) -> ProviderCapabilities:
        return cls(True, False, True, True, False, False, True, True, False, True, True)

    @classmethod
    def anthropic(cls) -> ProviderCapabilities:
        return cls(
            False,
            True,
            False,
            True,
            True,
            False,
            True,
            False,
            True,
            True,
            False,
            "claude",
        )

    @classmethod
    def ollama(cls) -> ProviderCapabilities:
        return cls(
            False, False, False, False, False, True, True, False, False, False, False
        )

    @classmethod
    def from_provider_name(cls, name: str) -> ProviderCapabilities:
        normalized = name.lower()
        if normalized == "anthropic":
            return cls.anthropic()
        if normalized == "ollama":
            return cls.ollama()
        return cls.openai_compatible()
