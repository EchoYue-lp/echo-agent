from __future__ import annotations

from dataclasses import dataclass

from .thinking import ThinkingLevel
from .thinking_protocol_values import ThinkingProtocol


@dataclass(frozen=True)
class ThinkingProfile:
    protocol: ThinkingProtocol
    levels: tuple[ThinkingLevel, ...]

    @classmethod
    def new(
        cls, protocol: ThinkingProtocol, levels: tuple[ThinkingLevel, ...]
    ) -> ThinkingProfile:
        return cls(protocol, tuple(levels))

    @classmethod
    def unknown(cls) -> ThinkingProfile:
        return cls(ThinkingProtocol.NONE, ())

    def supports_manual_control(self) -> bool:
        return self.protocol.emits_field() and bool(self.levels)


def _version(model: str, prefix: str) -> tuple[int, int] | None:
    if not model.startswith(prefix):
        return None
    segments = model[len(prefix) :].split("-")
    for index, segment in enumerate(segments):
        if "." in segment:
            major_text, minor_text = segment.split(".", 1)
            if "." in minor_text:
                continue
            try:
                major = int(major_text)
                minor = int(minor_text)
            except ValueError:
                continue
        else:
            try:
                major = int(segment)
            except ValueError:
                continue
            minor = 0
            if index + 1 < len(segments):
                try:
                    candidate = int(segments[index + 1])
                except ValueError:
                    candidate = 0
                if 0 <= candidate <= 9:
                    minor = candidate
        if 3 <= major <= 9:
            return major, minor
    return None


def resolve_thinking_profile(
    provider: str,
    model_name: str,
    api_protocol: str = "chat_completions",
    endpoint: str | None = None,
) -> ThinkingProfile:
    provider_lower = provider.strip().lower()
    model = model_name.strip().lower()
    endpoint_lower = (endpoint or "").lower()
    dashscope = (
        provider_lower
        in {"dashscope", "qwen", "aliyun", "alibaba", "modelstudio", "bailian"}
        or "dashscope.aliyuncs.com" in endpoint_lower
    )
    ollama = (
        provider_lower == "ollama"
        or "localhost:11434" in endpoint_lower
        or "127.0.0.1:11434" in endpoint_lower
    )
    if model.startswith("claude-"):
        parsed = _version(model, "claude-")
        if parsed is None or parsed < (4, 6):
            return ThinkingProfile.unknown()
        if parsed == (4, 6):
            return ThinkingProfile.new(
                ThinkingProtocol.ANTHROPIC_EFFORT
                if api_protocol == "anthropic"
                else ThinkingProtocol.OPENAI_REASONING_EFFORT,
                (
                    ThinkingLevel.LOW,
                    ThinkingLevel.MEDIUM,
                    ThinkingLevel.HIGH,
                    ThinkingLevel.XHIGH,
                    ThinkingLevel.MAX,
                ),
            )
        return ThinkingProfile.new(ThinkingProtocol.ANTHROPIC_ADAPTIVE, ())
    if api_protocol == "anthropic":
        return ThinkingProfile.unknown()
    if ollama and api_protocol == "chat_completions":
        if model.startswith("gpt-oss"):
            return ThinkingProfile.new(
                ThinkingProtocol.OLLAMA_THINK,
                (ThinkingLevel.LOW, ThinkingLevel.MEDIUM, ThinkingLevel.HIGH),
            )
        if any(
            model.startswith(prefix)
            for prefix in (
                "qwen3",
                "deepseek-r1",
                "deepseek-v3",
                "deepseek-v4",
                "magistral",
            )
        ):
            return ThinkingProfile.new(
                ThinkingProtocol.OLLAMA_THINK, (ThinkingLevel.NONE, ThinkingLevel.HIGH)
            )
        return ThinkingProfile.unknown()
    if model.startswith(("gpt-5.6", "gpt-5-6")):
        return ThinkingProfile.new(
            ThinkingProtocol.OPENAI_REASONING_EFFORT,
            (
                ThinkingLevel.NONE,
                ThinkingLevel.LOW,
                ThinkingLevel.MEDIUM,
                ThinkingLevel.HIGH,
                ThinkingLevel.XHIGH,
                ThinkingLevel.MAX,
            ),
        )
    if model.startswith("deepseek-v4"):
        return (
            ThinkingProfile.new(
                ThinkingProtocol.ENABLE_THINKING_FLAG,
                (ThinkingLevel.NONE, ThinkingLevel.HIGH),
            )
            if dashscope and api_protocol == "chat_completions"
            else ThinkingProfile.new(
                ThinkingProtocol.DEEPSEEK_REASONING_EFFORT,
                (
                    ThinkingLevel.NONE,
                    ThinkingLevel.LOW,
                    ThinkingLevel.HIGH,
                    ThinkingLevel.MAX,
                ),
            )
        )
    glm = _version(model, "glm-")
    if glm and (glm[0] > 5 or glm >= (5, 2)) and api_protocol == "chat_completions":
        return ThinkingProfile.new(
            ThinkingProtocol.GLM_REASONING_EFFORT,
            (ThinkingLevel.NONE, ThinkingLevel.HIGH, ThinkingLevel.MAX),
        )
    if model.startswith("kimi-k3") and api_protocol == "chat_completions":
        return ThinkingProfile.new(
            ThinkingProtocol.OPENAI_REASONING_EFFORT,
            (ThinkingLevel.LOW, ThinkingLevel.HIGH, ThinkingLevel.MAX),
        )
    if model.startswith("kimi-k2.7"):
        return ThinkingProfile.new(ThinkingProtocol.MODEL_MANAGED, ())
    if model.startswith("kimi-k2.6") and api_protocol == "chat_completions":
        return ThinkingProfile.new(
            ThinkingProtocol.THINKING_TYPE, (ThinkingLevel.NONE, ThinkingLevel.HIGH)
        )
    if model.startswith("qwen3") and api_protocol == "chat_completions":
        return ThinkingProfile.new(
            ThinkingProtocol.ENABLE_THINKING_FLAG,
            (ThinkingLevel.NONE, ThinkingLevel.HIGH),
        )
    if model.startswith("gemini-3") and api_protocol == "chat_completions":
        return ThinkingProfile.new(
            ThinkingProtocol.OPENAI_REASONING_EFFORT,
            (
                ThinkingLevel.MINIMAL,
                ThinkingLevel.LOW,
                ThinkingLevel.MEDIUM,
                ThinkingLevel.HIGH,
            ),
        )
    if model.startswith("gemini-2.5") and api_protocol == "chat_completions":
        return ThinkingProfile.new(
            ThinkingProtocol.OPENAI_REASONING_EFFORT,
            (
                ThinkingLevel.NONE,
                ThinkingLevel.LOW,
                ThinkingLevel.MEDIUM,
                ThinkingLevel.HIGH,
            ),
        )
    return ThinkingProfile.unknown()
