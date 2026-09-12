from __future__ import annotations

from dataclasses import dataclass


@dataclass(frozen=True, slots=True)
class SubagentContext:
    tool_definitions: tuple[object, ...] = ()
    messages: tuple[object, ...] = ()
    store_present: bool = False
    parent_goal: str | None = None
    allowed_tools: tuple[str, ...] | None = None

    @classmethod
    def empty(cls) -> SubagentContext:
        return cls()

    def has_content(self) -> bool:
        return bool(
            self.tool_definitions
            or self.messages
            or self.store_present
            or self.parent_goal is not None
            or self.allowed_tools is not None
        )
