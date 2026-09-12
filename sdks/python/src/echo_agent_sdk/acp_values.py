from __future__ import annotations

from dataclasses import dataclass
from enum import Enum


class ConnectionMode(str, Enum):
    STANDARD = "standard"
    EXTENDED = "extended"

    def as_str(self) -> str:
        return self.value


class ExtensionSettlement(str, Enum):
    ANSWERED = "answered"
    TIMED_OUT = "timed_out"
    CANCELLED = "cancelled"
    DISCONNECTED = "disconnected"

    def as_str(self) -> str:
        return self.value

    def is_answered(self) -> bool:
        return self is ExtensionSettlement.ANSWERED


class ExtensionLeaseError(str, Enum):
    ADMISSION_CLOSED = "extension admission is closed"
    CONCURRENCY_LIMIT = "extension concurrency limit reached"
    EXCLUSIVE_CONFLICT = "extension is already executing an exclusive invocation"

    def as_str(self) -> str:
        return self.value


@dataclass(frozen=True, slots=True)
class AcpLedgerLimits:
    max_events: int
    max_bytes: int

    def __post_init__(self) -> None:
        for name, value in (
            ("max_events", self.max_events),
            ("max_bytes", self.max_bytes),
        ):
            if isinstance(value, bool) or not isinstance(value, int) or value < 0:
                raise ValueError(f"{name} must be a non-negative integer")

    @classmethod
    def default(cls) -> AcpLedgerLimits:
        return cls(max_events=10_000, max_bytes=8 * 1024 * 1024)
