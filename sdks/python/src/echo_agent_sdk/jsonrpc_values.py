from __future__ import annotations

from dataclasses import dataclass
from typing import Any


@dataclass(frozen=True, slots=True)
class JsonRpcRequest:
    jsonrpc: str
    id: Any
    method: str
    params: Any

    @classmethod
    def new(cls, method: str, params: Any = None) -> JsonRpcRequest:
        return cls("2.0", None, method, params)


@dataclass(frozen=True, slots=True)
class JsonRpcNotification:
    jsonrpc: str
    method: str
    params: Any

    @classmethod
    def new(cls, method: str, params: Any = None) -> JsonRpcNotification:
        return cls("2.0", method, params)
