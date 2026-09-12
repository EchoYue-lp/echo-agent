from __future__ import annotations

from typing import Any


class EchoAgentError(RuntimeError):
    """Typed SDK/ACP failure without parsing human-readable ACP messages."""

    def __init__(
        self,
        code: str,
        message: str,
        retryable: str = "never",
        operation: str | None = None,
        details: Any = None,
    ) -> None:
        super().__init__(message)
        self.code = code
        self.message = message
        self.retryable = retryable
        self.operation = operation
        self.details = details

    @classmethod
    def from_exception(
        cls, error: BaseException, operation: str | None = None
    ) -> EchoAgentError:
        data = getattr(error, "data", None)
        if not isinstance(data, dict):
            data = {}
        return cls(
            str(data.get("code", "transport_error")),
            str(data.get("message", str(error))),
            str(data.get("retryable", "never")),
            str(data["operation"])
            if isinstance(data.get("operation"), str)
            else operation,
            data.get("details"),
        )
