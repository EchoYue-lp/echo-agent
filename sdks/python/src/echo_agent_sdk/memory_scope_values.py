from __future__ import annotations

from enum import Enum


class MemoryScope(str, Enum):
    """Memory lifetime scopes with Rust-compatible wire names and priority."""

    USER = "user"
    PROJECT = "project"
    REPO = "repo"
    TASK = "task"
    SESSION = "session"
    RUN = "run"


_MEMORY_SCOPE_ALL: tuple[MemoryScope, ...] = (
    MemoryScope.USER,
    MemoryScope.PROJECT,
    MemoryScope.REPO,
    MemoryScope.TASK,
    MemoryScope.SESSION,
    MemoryScope.RUN,
)


def memory_scope_all() -> tuple[MemoryScope, ...]:
    return _MEMORY_SCOPE_ALL


def memory_scope_name(scope: MemoryScope) -> str:
    scope = _validate(scope)
    return scope.value


def memory_scope_priority(scope: MemoryScope) -> int:
    scope = _validate(scope)
    return _MEMORY_SCOPE_ALL.index(scope)


def memory_scope_is_persistent(scope: MemoryScope) -> bool:
    scope = _validate(scope)
    return scope in {MemoryScope.USER, MemoryScope.PROJECT, MemoryScope.REPO}


def parse_memory_scope(value: str) -> MemoryScope | None:
    if not isinstance(value, str):
        return None
    return {
        "user": MemoryScope.USER,
        "project": MemoryScope.PROJECT,
        "proj": MemoryScope.PROJECT,
        "repo": MemoryScope.REPO,
        "task": MemoryScope.TASK,
        "session": MemoryScope.SESSION,
        "sess": MemoryScope.SESSION,
        "run": MemoryScope.RUN,
    }.get(value.strip().lower())


def _validate(scope: MemoryScope) -> MemoryScope:
    if not isinstance(scope, MemoryScope):
        raise TypeError(f"unknown memory scope: {scope!r}")
    return scope
