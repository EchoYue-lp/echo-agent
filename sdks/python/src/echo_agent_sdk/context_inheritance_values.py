from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass
from types import MappingProxyType


@dataclass(frozen=True, slots=True)
class ContextInheritance:
    inherit_tools: tuple[str, ...] | None
    inherit_history: int | None
    inherit_memory: bool
    inject_metadata: Mapping[str, str]

    def __post_init__(self) -> None:
        if self.inherit_history is not None and (
            isinstance(self.inherit_history, bool)
            or not isinstance(self.inherit_history, int)
            or self.inherit_history < 0
        ):
            raise ValueError("inherit history must be a non-negative integer")
        object.__setattr__(
            self,
            "inherit_tools",
            None if self.inherit_tools is None else tuple(self.inherit_tools),
        )
        object.__setattr__(
            self, "inject_metadata", MappingProxyType(dict(self.inject_metadata))
        )

    @classmethod
    def sync_default(cls) -> ContextInheritance:
        return cls(None, None, False, {})

    @classmethod
    def fresh_default(cls) -> ContextInheritance:
        return cls.sync_default()

    @classmethod
    def fork_default(cls) -> ContextInheritance:
        return cls(None, 2, True, {})

    @classmethod
    def teammate_default(cls) -> ContextInheritance:
        return cls((), 2, False, {})

    @classmethod
    def for_mode(cls, mode: str) -> ContextInheritance:
        if mode == "sync":
            return cls.sync_default()
        if mode == "fork":
            return cls.fork_default()
        if mode in {"teammate", "team"}:
            return cls.teammate_default()
        raise ValueError(f"unknown execution mode: {mode}")
