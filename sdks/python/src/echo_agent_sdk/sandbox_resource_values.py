from __future__ import annotations

from dataclasses import dataclass


@dataclass(frozen=True)
class ResourceLimits:
    """Sandbox resource policy value; process execution remains Rust-owned."""

    cpu_time_secs: int | None
    memory_bytes: int | None
    max_output_bytes: int | None
    max_processes: int | None
    network: bool
    read_only_paths: tuple[str, ...] = ()
    writable_paths: tuple[str, ...] = ()

    @classmethod
    def default(cls) -> ResourceLimits:
        return cls(30, 256 * 1024 * 1024, 1024 * 1024, 64, False)

    @classmethod
    def strict(cls) -> ResourceLimits:
        return cls(10, 64 * 1024 * 1024, 256 * 1024, 8, False)

    @classmethod
    def unrestricted(cls) -> ResourceLimits:
        return cls(None, None, None, None, True)
