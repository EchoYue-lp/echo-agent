from __future__ import annotations

from dataclasses import dataclass
from enum import Enum


class ContentGuardKind(str, Enum):
    PASS = "pass"
    DETECTED = "detected"
    REJECTED = "rejected"
    REDACTED = "redacted"


@dataclass(frozen=True, slots=True)
class ContentGuardResult:
    kind: ContentGuardKind
    pii_types: tuple[str, ...] = ()
    content: str | None = None

    def __post_init__(self) -> None:
        if self.kind in {ContentGuardKind.DETECTED, ContentGuardKind.REJECTED}:
            if any(not isinstance(value, str) for value in self.pii_types):
                raise TypeError("pii_types must contain text")
            if self.content is not None:
                raise TypeError("pii result cannot carry redacted content")
        elif self.kind is ContentGuardKind.REDACTED:
            if not isinstance(self.content, str):
                raise TypeError("redacted result requires text content")
            if self.pii_types:
                raise TypeError("redacted result cannot carry pii types")
        elif self.pii_types or self.content is not None:
            raise TypeError("pass result cannot carry payload fields")

    @classmethod
    def pass_result(cls) -> ContentGuardResult:
        return cls(ContentGuardKind.PASS)

    @classmethod
    def detected(cls, pii_types: list[str] | tuple[str, ...]) -> ContentGuardResult:
        return cls(ContentGuardKind.DETECTED, tuple(pii_types))

    @classmethod
    def rejected(cls, pii_types: list[str] | tuple[str, ...]) -> ContentGuardResult:
        return cls(ContentGuardKind.REJECTED, tuple(pii_types))

    @classmethod
    def redacted(cls, content: str) -> ContentGuardResult:
        return cls(ContentGuardKind.REDACTED, content=content)

    def is_rejected(self) -> bool:
        return self.kind is ContentGuardKind.REJECTED
