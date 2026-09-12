from __future__ import annotations

import random
from dataclasses import dataclass, replace


@dataclass(frozen=True)
class RetryPolicy:
    max_retries: int
    base_delay_ms: float
    max_delay_ms: float = 60_000
    jitter_enabled: bool = False

    @classmethod
    def new(cls, max_retries: int, base_delay_ms: float) -> RetryPolicy:
        if max_retries < 0 or base_delay_ms < 0:
            raise ValueError("retry policy values must be non-negative")
        return cls(max_retries, base_delay_ms)

    @classmethod
    def default(cls) -> RetryPolicy:
        return cls(3, 500, 30_000, True)

    @classmethod
    def no_retry(cls) -> RetryPolicy:
        return cls(0, 0, 0, False)

    def max_delay(self, delay_ms: float) -> RetryPolicy:
        if delay_ms < 0:
            raise ValueError("retry max delay must be non-negative")
        return replace(self, max_delay_ms=delay_ms)

    def jitter(self, enabled: bool) -> RetryPolicy:
        return replace(self, jitter_enabled=enabled)

    def delay_for(self, attempt: int) -> float:
        if attempt < 0:
            raise ValueError("retry attempt must be non-negative")
        if attempt == 0:
            return 0
        capped = min(
            self.base_delay_ms * (2 ** min(attempt - 1, 10)), self.max_delay_ms
        )
        return (
            random.uniform(0, capped) if self.jitter_enabled and capped > 0 else capped
        )
