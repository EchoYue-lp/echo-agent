from __future__ import annotations

from enum import Enum


class SubagentCommandPhase(str, Enum):
    PERSISTED = "persisted"
    MAILBOX_ACCEPTED = "mailbox_accepted"
    DRAINED = "drained"
    TURN_SETTLED = "turn_settled"

    def as_str(self) -> str:
        return self.value

    @classmethod
    def parse(cls, value: str) -> SubagentCommandPhase | None:
        if not isinstance(value, str):
            return None
        try:
            return cls(value)
        except ValueError:
            return None


class SubagentStatus(str, Enum):
    RUNNING = "running"
    COMPLETED = "completed"
    FAILED = "failed"
    CANCELLED = "cancelled"
    TIMED_OUT = "timed_out"

    def as_str(self) -> str:
        return self.value

    @classmethod
    def parse(cls, value: str) -> SubagentStatus:
        if not isinstance(value, str):
            raise TypeError("subagent status must be text")
        try:
            return cls(value)
        except ValueError as error:
            raise ValueError(f"unknown Subagent status: {value}") from error
