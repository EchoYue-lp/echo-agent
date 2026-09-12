from __future__ import annotations

from dataclasses import dataclass
from enum import Enum


class GuardDecisionKind(str, Enum):
    PASS = "pass"
    BLOCK = "block"
    WARN = "warn"
    TRANSFORM = "transform"


@dataclass(frozen=True, slots=True)
class GuardDecision:
    kind: GuardDecisionKind
    reason: str | None = None
    reasons: tuple[str, ...] = ()
    content: str | None = None

    def __post_init__(self) -> None:
        if self.kind is GuardDecisionKind.PASS:
            if self.reason is not None or self.reasons or self.content is not None:
                raise TypeError("pass decision cannot carry payload fields")
        elif self.kind is GuardDecisionKind.BLOCK:
            if (
                not isinstance(self.reason, str)
                or self.reasons
                or self.content is not None
            ):
                raise TypeError("block decision requires only a reason")
        elif self.kind is GuardDecisionKind.WARN:
            if self.reason is not None or self.content is not None:
                raise TypeError("warn decision cannot carry reason/content fields")
        elif self.kind is GuardDecisionKind.TRANSFORM and (
            not isinstance(self.content, str) or self.reason is not None
        ):
            raise TypeError("transform decision requires content and reasons")
        if any(not isinstance(value, str) for value in self.reasons):
            raise TypeError("reasons must contain text")

    @classmethod
    def pass_decision(cls) -> GuardDecision:
        return cls(GuardDecisionKind.PASS)

    @classmethod
    def block(cls, reason: str) -> GuardDecision:
        return cls(GuardDecisionKind.BLOCK, reason=reason)

    @classmethod
    def warn(cls, reasons: list[str] | tuple[str, ...]) -> GuardDecision:
        return cls(GuardDecisionKind.WARN, reasons=tuple(reasons))

    @classmethod
    def transform(
        cls, content: str, reasons: list[str] | tuple[str, ...]
    ) -> GuardDecision:
        return cls(GuardDecisionKind.TRANSFORM, reasons=tuple(reasons), content=content)

    def is_blocked(self) -> bool:
        return self.kind is GuardDecisionKind.BLOCK
