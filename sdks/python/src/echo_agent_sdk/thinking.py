from __future__ import annotations

from enum import Enum


class ThinkingLevel(str, Enum):
    NONE = "none"
    MINIMAL = "minimal"
    LOW = "low"
    MEDIUM = "medium"
    HIGH = "high"
    XHIGH = "xhigh"
    MAX = "max"

    @classmethod
    def parse(cls, value: str) -> ThinkingLevel | None:
        if not isinstance(value, str):
            return None
        return {
            "none": cls.NONE,
            "off": cls.NONE,
            "minimal": cls.MINIMAL,
            "min": cls.MINIMAL,
            "low": cls.LOW,
            "medium": cls.MEDIUM,
            "med": cls.MEDIUM,
            "normal": cls.MEDIUM,
            "high": cls.HIGH,
            "xhigh": cls.XHIGH,
            "max": cls.MAX,
        }.get(value.strip().lower())
