from __future__ import annotations

from dataclasses import dataclass


@dataclass(frozen=True, slots=True)
class AcpDuration:
    seconds: int
    nanos: int = 0

    def __post_init__(self) -> None:
        if (
            isinstance(self.seconds, bool)
            or not isinstance(self.seconds, int)
            or self.seconds < 0
        ):
            raise ValueError("duration seconds must be a non-negative integer")
        if (
            isinstance(self.nanos, bool)
            or not isinstance(self.nanos, int)
            or not 0 <= self.nanos < 1_000_000_000
        ):
            raise ValueError("duration nanos must be in [0, 1_000_000_000)")

    def is_zero(self) -> bool:
        return self.seconds == 0 and self.nanos == 0


@dataclass(frozen=True, slots=True)
class AcpAdapterConfig:
    name: str
    title: str
    version: str
    max_sessions: int
    max_prompt_chars: int
    max_update_chars: int
    max_updates_per_turn: int
    max_total_update_chars: int
    max_extension_concurrency: int
    shutdown_timeout: AcpDuration

    @classmethod
    def default(cls, version: str = "0.2.0") -> AcpAdapterConfig:
        return cls(
            name="echo-agent",
            title="echo-agent",
            version=version,
            max_sessions=128,
            max_prompt_chars=1_000_000,
            max_update_chars=1_000_000,
            max_updates_per_turn=10_000,
            max_total_update_chars=8_000_000,
            max_extension_concurrency=8,
            shutdown_timeout=AcpDuration(5),
        )

    def validate(self) -> None:
        if not self.name.strip() or not self.title.strip() or not self.version.strip():
            raise ValueError("ACP adapter name, title, and version must not be empty")
        limits = (
            self.max_sessions,
            self.max_prompt_chars,
            self.max_update_chars,
            self.max_updates_per_turn,
            self.max_total_update_chars,
            self.max_extension_concurrency,
        )
        if any(
            isinstance(value, bool) or not isinstance(value, int) or value <= 0
            for value in limits
        ):
            raise ValueError("ACP adapter resource limits must be positive")
        if self.shutdown_timeout.is_zero():
            raise ValueError("ACP adapter shutdown timeout must be positive")
