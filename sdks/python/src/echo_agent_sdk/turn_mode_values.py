from __future__ import annotations

from enum import Enum


class TurnMode(str, Enum):
    CHAT = "chat"
    EXECUTE = "execute"

    def as_str(self) -> str:
        return self.value
