from __future__ import annotations

from dataclasses import dataclass
from typing import Any


@dataclass(frozen=True)
class InterventionResult:
    """Immutable intervention decision; callback execution remains Host-owned."""

    block: bool = False
    block_reason: str | None = None
    injected_context: str | None = None
    redirect_to: str | None = None
    cancel: bool = False
    modified_args: Any = None

    @classmethod
    def allow(cls) -> InterventionResult:
        return cls()

    @classmethod
    def block_with_reason(cls, reason: str) -> InterventionResult:
        if not reason.strip():
            raise ValueError("block reason must not be empty")
        return cls(block=True, block_reason=reason)

    @classmethod
    def inject(cls, context: str) -> InterventionResult:
        return cls(injected_context=context)

    @classmethod
    def cancelled(cls) -> InterventionResult:
        return cls(cancel=True)

    @classmethod
    def modify_args(cls, args: Any) -> InterventionResult:
        return cls(modified_args=args)
