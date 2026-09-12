from __future__ import annotations

from dataclasses import dataclass


@dataclass(frozen=True, slots=True)
class SegmentRange:
    start: int = 0
    end: int = 0

    def __post_init__(self) -> None:
        if (
            isinstance(self.start, bool)
            or not isinstance(self.start, int)
            or isinstance(self.end, bool)
            or not isinstance(self.end, int)
            or self.start < 0
            or self.end < 0
        ):
            raise ValueError("segment range bounds must be non-negative integers")

    def len(self) -> int:
        return max(0, self.end - self.start)

    def is_empty(self) -> bool:
        return self.len() == 0
