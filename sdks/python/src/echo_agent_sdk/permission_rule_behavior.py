from __future__ import annotations

from dataclasses import dataclass
from enum import Enum


class RuleBehaviorKind(str, Enum):
    ALLOW = "allow"
    DENY = "deny"
    ASK = "ask"


@dataclass(frozen=True, slots=True)
class RuleBehavior:
    kind: RuleBehaviorKind
    reason: str | None = None
    suggestions: tuple[str, ...] = ()

    @classmethod
    def allow(cls) -> RuleBehavior:
        return cls(RuleBehaviorKind.ALLOW)

    @classmethod
    def deny(cls, reason: str) -> RuleBehavior:
        return cls(RuleBehaviorKind.DENY, reason=reason)

    @classmethod
    def ask(cls, suggestions: list[str] | tuple[str, ...]) -> RuleBehavior:
        return cls(RuleBehaviorKind.ASK, suggestions=tuple(suggestions))

    @classmethod
    def parse(cls, value: str) -> RuleBehavior:
        if not isinstance(value, str):
            raise TypeError("rule behavior must be text")
        if value == "allow":
            return cls.allow()
        if value == "deny":
            return cls.deny("denied by rule")
        if value == "ask":
            return cls.ask(("allow", "deny"))
        raise ValueError(f"unknown permission rule behavior: {value}")

    def to_decision(self) -> PermissionDecision:
        if self.kind is RuleBehaviorKind.ALLOW:
            return PermissionDecision.allow()
        if self.kind is RuleBehaviorKind.DENY:
            return PermissionDecision.deny(self.reason or "")
        return PermissionDecision.ask(self.suggestions)


@dataclass(frozen=True, slots=True)
class PermissionDecision:
    kind: RuleBehaviorKind
    reason: str | None = None
    suggestions: tuple[str, ...] = ()

    @classmethod
    def allow(cls) -> PermissionDecision:
        return cls(RuleBehaviorKind.ALLOW)

    @classmethod
    def deny(cls, reason: str) -> PermissionDecision:
        return cls(RuleBehaviorKind.DENY, reason=reason)

    @classmethod
    def ask(cls, suggestions: tuple[str, ...]) -> PermissionDecision:
        return cls(RuleBehaviorKind.ASK, suggestions=tuple(suggestions))
