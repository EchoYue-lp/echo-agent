from __future__ import annotations

from collections.abc import Mapping
from typing import NotRequired, TypedDict


class ExecutionUsageValue(TypedDict):
    duration_ms: NotRequired[int | None]
    tokens_used: NotRequired[int | None]
    iterations: NotRequired[int | None]


def execution_usage_duration_millis(usage: Mapping[str, object]) -> int:
    value = usage.get("duration_ms")
    if isinstance(value, str):
        try:
            return int(value)
        except ValueError:
            return 0
    return value if isinstance(value, int) and not isinstance(value, bool) else 0
