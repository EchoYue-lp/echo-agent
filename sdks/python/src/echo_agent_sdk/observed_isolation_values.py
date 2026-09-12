from __future__ import annotations

from dataclasses import dataclass


@dataclass(frozen=True, slots=True)
class ObservedIsolation:
    value: str

    @classmethod
    def new(cls, value: str) -> ObservedIsolation:
        if not isinstance(value, str):
            raise TypeError("observed isolation must be text")
        trimmed = value.strip()
        return cls(trimmed[:512] if trimmed else "unknown")

    @classmethod
    def default(cls) -> ObservedIsolation:
        return cls("unknown")

    def as_str(self) -> str:
        return self.value
