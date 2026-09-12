from __future__ import annotations

from enum import Enum


class CommandCellTerminalCause(str, Enum):
    EXITED = "exited"
    TIMED_OUT = "timed_out"
    CANCELLED = "cancelled"
    LAUNCH_FAILED = "launch_failed"
    WAIT_FAILED = "wait_failed"
    OUTPUT_DRAIN_FAILED = "output_drain_failed"

    def as_str(self) -> str:
        return self.value


class CommandCellArtifactStatus(str, Enum):
    NOT_REQUESTED = "not_requested"
    WRITING = "writing"
    BELOW_THRESHOLD = "below_threshold"
    AVAILABLE = "available"
    FAILED = "failed"

    def as_str(self) -> str:
        return self.value
