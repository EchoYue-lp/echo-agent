from __future__ import annotations

import time
from datetime import datetime, timezone


def now_secs() -> int:
    return time.time_ns() // 1_000_000_000


def now_millis() -> int:
    return time.time_ns() // 1_000_000


def now_local() -> datetime:
    return datetime.now().astimezone()


def to_local(value: datetime) -> datetime:
    if value.tzinfo is None:
        raise ValueError("timestamp must be timezone-aware")
    return value.astimezone()


def local_rfc3339_serialize(value: datetime) -> str:
    return to_local(value).isoformat()


def local_rfc3339_deserialize(value: str) -> datetime:
    try:
        parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError as error:
        raise ValueError("invalid RFC3339 timestamp") from error
    if parsed.tzinfo is None:
        raise ValueError("timestamp must include an offset")
    return parsed.astimezone(timezone.utc)


def option_local_rfc3339_serialize(value: datetime | None) -> str | None:
    return None if value is None else local_rfc3339_serialize(value)


def option_local_rfc3339_deserialize(value: str | None) -> datetime | None:
    return None if value is None else local_rfc3339_deserialize(value)
