from __future__ import annotations

from dataclasses import dataclass

from .thinking import ThinkingLevel


def _budget_effort(tokens: int, glm: bool = False) -> str:
    if tokens < 4_000:
        return "low"
    if tokens < 12_000:
        return "medium"
    if tokens < 24_000:
        return "high"
    if glm:
        return "max"
    if tokens < 48_000:
        return "xhigh"
    return "max"


@dataclass(frozen=True)
class ThinkingConfig:
    kind: str
    value: ThinkingLevel | int | None = None

    @classmethod
    def disabled(cls) -> ThinkingConfig:
        return cls("disabled")

    @classmethod
    def level(cls, value: ThinkingLevel) -> ThinkingConfig:
        return cls("level", value)

    @classmethod
    def budget_tokens(cls, value: int) -> ThinkingConfig:
        if value < 0:
            raise ValueError("thinking budget must be non-negative")
        return cls("budget_tokens", value)

    @classmethod
    def medium(cls) -> ThinkingConfig:
        return cls.level(ThinkingLevel.MEDIUM)

    @classmethod
    def parse_spec(cls, spec: str) -> ThinkingConfig | None:
        trimmed = spec.strip().lower()
        if trimmed in {"", "auto", "default"}:
            return None
        if trimmed in {"disabled", "off", "false"}:
            return cls.disabled()
        if trimmed.isdecimal():
            return cls.budget_tokens(int(trimmed))
        level = ThinkingLevel.parse(trimmed)
        if level is not None:
            return cls.level(level)
        raise ValueError(f"unrecognized thinking spec: '{spec}'")

    def to_reasoning_effort(self) -> str | None:
        if self.kind == "disabled":
            return "minimal"
        if self.kind == "budget_tokens":
            return _budget_effort(int(self.value))
        return self.value.value if self.value is not ThinkingLevel.NONE else "none"

    def to_anthropic_effort(self) -> str | None:
        if self.kind == "disabled" or self.value in {
            ThinkingLevel.NONE,
            ThinkingLevel.MINIMAL,
        }:
            return None
        if self.kind == "budget_tokens":
            return _budget_effort(int(self.value))
        return self.value.value

    def to_anthropic_budget(self, max_tokens: int) -> int | None:
        if (
            max_tokens <= 1
            or self.kind == "disabled"
            or self.value in {ThinkingLevel.NONE, ThinkingLevel.MINIMAL}
        ):
            return None
        if self.kind == "budget_tokens":
            budget = int(self.value)
        else:
            fractions = {
                ThinkingLevel.LOW: 0.25,
                ThinkingLevel.MEDIUM: 0.5,
                ThinkingLevel.HIGH: 0.8,
                ThinkingLevel.XHIGH: 0.95,
                ThinkingLevel.MAX: 0.98,
            }
            budget = round(max_tokens * fractions[self.value])
        return min(budget, max_tokens - 1)

    def to_enable_thinking(self) -> bool:
        return self.kind == "budget_tokens" or self.value not in {
            ThinkingLevel.NONE,
            ThinkingLevel.MINIMAL,
        }

    def to_glm_thinking_type(self) -> str:
        return "enabled" if self.to_enable_thinking() else "disabled"

    def to_glm_reasoning_effort(self) -> str | None:
        if self.kind == "disabled":
            return "none"
        if self.kind == "budget_tokens":
            return _budget_effort(int(self.value), True)
        return self.value.value
