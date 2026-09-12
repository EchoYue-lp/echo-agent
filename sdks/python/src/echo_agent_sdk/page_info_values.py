from __future__ import annotations

import json
from dataclasses import dataclass

from .intrinsics import ToolResult


@dataclass(frozen=True, slots=True)
class PageInfo:
    next_cursor: str | None = None
    truncated: bool = False
    total_known: bool = False
    total: int | None = None
    returned: int = 0

    def apply_to(self, result: ToolResult) -> ToolResult:
        updated = result.with_truncated(result.truncated or self.truncated)
        updated = updated.with_meta("page.truncated", str(self.truncated).lower())
        updated = updated.with_meta("page.total_known", str(self.total_known).lower())
        updated = updated.with_meta("page.returned", str(self.returned))
        if self.total is not None:
            updated = updated.with_meta("page.total", str(self.total))
        if self.next_cursor is not None:
            updated = updated.with_meta("page.next_cursor", self.next_cursor)
            continuation = json.dumps(
                {
                    "next_cursor": self.next_cursor,
                    "returned": self.returned,
                    "total": self.total,
                    "total_known": self.total_known,
                    "truncated": True,
                },
                separators=(",", ":"),
            )
            updated = updated.with_output(f"{updated.output}\n[page]{continuation}")
        return updated
