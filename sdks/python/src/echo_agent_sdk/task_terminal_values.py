from __future__ import annotations

from enum import Enum


class TaskTerminalStatus(str, Enum):
    COMPLETED = "completed"
    FAILED = "failed"
    CANCELLED = "cancelled"
    TIMED_OUT = "timed_out"
    SKIPPED = "skipped"

    def as_str(self) -> str:
        return self.value
