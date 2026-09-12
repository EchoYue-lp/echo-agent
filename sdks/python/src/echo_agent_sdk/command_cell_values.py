from __future__ import annotations

from enum import Enum


class CommandCellPhase(str, Enum):
    PREPARED = "prepared"
    QUEUED = "queued"
    RUNNING = "running"
    SUCCEEDED = "succeeded"
    FAILED = "failed"
    CANCELLED = "cancelled"
    LAUNCH_FAILED = "launch_failed"

    def as_str(self) -> str:
        return self.value

    def is_terminal(self) -> bool:
        return self in {
            self.SUCCEEDED,
            self.FAILED,
            self.CANCELLED,
            self.LAUNCH_FAILED,
        }
