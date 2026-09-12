from __future__ import annotations

from enum import Enum


class LlmApiProtocol(str, Enum):
    """HTTP API protocol spoken by a complete provider endpoint."""

    CHAT_COMPLETIONS = "chat_completions"
    RESPONSES = "responses"
    ANTHROPIC = "anthropic"

    def endpoint_path(self) -> str:
        if self is LlmApiProtocol.RESPONSES:
            return "responses"
        if self is LlmApiProtocol.ANTHROPIC:
            return "messages"
        return "chat/completions"

    @classmethod
    def try_from_endpoint(cls, endpoint: str) -> LlmApiProtocol | None:
        base = endpoint.split("?", 1)[0].split("#", 1)[0].rstrip("/")
        if base.endswith("/responses"):
            return cls.RESPONSES
        if base.endswith("/messages"):
            return cls.ANTHROPIC
        if base.endswith("/chat/completions"):
            return cls.CHAT_COMPLETIONS
        return None

    @classmethod
    def from_endpoint(cls, endpoint: str) -> LlmApiProtocol:
        detected = cls.try_from_endpoint(endpoint)
        if detected is not None:
            return detected
        base = endpoint.split("?", 1)[0].split("#", 1)[0]
        return cls.ANTHROPIC if "anthropic.com/" in base else cls.CHAT_COMPLETIONS
