from __future__ import annotations

import json
from collections.abc import Mapping
from dataclasses import dataclass, replace
from typing import Any, Literal, TypeAlias

from .wire import to_wire, wire_u64

JsonValue: TypeAlias = None | bool | int | float | str | list[Any] | dict[str, Any]
ParamValue: TypeAlias = JsonValue
ToolFailureCategory: TypeAlias = Literal[
    "invalid_arguments",
    "unavailable",
    "timeout",
    "cancelled",
    "transient",
    "permanent",
    "partial_side_effect",
]
ToolRecoveryAction: TypeAlias = Literal[
    "correct_arguments", "retry", "restore_then_retry", "verify_then_retry", "stop"
]
ToolSideEffect: TypeAlias = Literal["none", "possible", "confirmed"]

_FAILURE_CATEGORIES = frozenset(
    {
        "invalid_arguments",
        "unavailable",
        "timeout",
        "cancelled",
        "transient",
        "permanent",
        "partial_side_effect",
    }
)
_RECOVERY_ACTIONS = frozenset(
    {"correct_arguments", "retry", "restore_then_retry", "verify_then_retry", "stop"}
)
_SIDE_EFFECTS = frozenset({"none", "possible", "confirmed"})


def _param_type_name(value: ParamValue) -> str:
    if value is None:
        return "null"
    if isinstance(value, bool):
        return "bool"
    if isinstance(value, (int, float)):
        return "number"
    if isinstance(value, str):
        return "string"
    if isinstance(value, list):
        return "array"
    return "object"


@dataclass(frozen=True, slots=True)
class ToolCallParams:
    """Type-safe view over a tool call's JSON parameter object."""

    raw: ParamValue
    _parsed: Mapping[str, ParamValue]

    def __init__(
        self, raw: ParamValue, parsed: Mapping[str, ParamValue] | None = None
    ) -> None:
        object.__setattr__(self, "raw", raw)
        if isinstance(raw, dict):
            values = dict(raw)
        else:
            values = {}
        object.__setattr__(self, "_parsed", dict(values if parsed is None else parsed))

    @classmethod
    def from_value(cls, value: ParamValue) -> ToolCallParams:
        return cls(value)

    @classmethod
    def from_params(cls, params: Mapping[str, ParamValue]) -> ToolCallParams:
        if not isinstance(params, Mapping):
            raise TypeError("params must be a JSON object")
        if any(not isinstance(key, str) for key in params):
            raise TypeError("params keys must be text")
        return cls(dict(params))

    def get_str(self, key: str) -> str | None:
        value = self._parsed.get(key)
        return value if isinstance(value, str) else None

    def get_number(self, key: str) -> float | None:
        value = self._parsed.get(key)
        if isinstance(value, bool) or not isinstance(value, (int, float)):
            return None
        return float(value)

    def get_bool(self, key: str) -> bool | None:
        value = self._parsed.get(key)
        return value if isinstance(value, bool) else None

    def get(self, key: str) -> ParamValue | None:
        return self._parsed.get(key)

    def validate_required(self, key: str, expected_type: str) -> None:
        if key not in self._parsed:
            raise TypeError(f"Missing required parameter: {key}")
        actual = _param_type_name(self._parsed[key])
        if actual != expected_type:
            raise TypeError(
                f"Parameter '{key}': expected {expected_type}, got {actual}"
            )

    def has(self, key: str) -> bool:
        return key in self._parsed

    def len(self) -> int:
        return len(self._parsed)

    def is_empty(self) -> bool:
        return not self._parsed


@dataclass(frozen=True, slots=True)
class ToolResult:
    """Immutable language-native equivalent of Rust's ``ToolResult``."""

    kind: dict[str, Any]
    output: str
    success: bool
    truncated: bool = False
    artifact: Any | None = None
    data: dict[str, Any] | None = None
    error: str | None = None
    failure: dict[str, Any] | None = None
    metadata: Mapping[str, str] | None = None
    mime_type: str | None = None
    model_content: tuple[Mapping[str, Any], ...] = ()

    @classmethod
    def success_result(cls, output: str) -> ToolResult:
        return cls({"kind": "text"}, output, True)

    @classmethod
    def success_json(cls, data: JsonValue) -> ToolResult:
        return cls(
            {"kind": "json"},
            json.dumps(data, separators=(",", ":"), ensure_ascii=False),
            True,
            data=_json_to_wire(data),
        )

    @classmethod
    def success_with_kind(cls, kind: Mapping[str, Any], output: str) -> ToolResult:
        _validate_kind(kind)
        return cls(dict(kind), output, True)

    @classmethod
    def error_result(cls, error: str) -> ToolResult:
        return cls(
            {"kind": "structured_error", "error_code": "tool_error"},
            "",
            False,
            error=error,
            failure={
                "category": "permanent",
                "recovery": "stop",
                "side_effect": "none",
            },
        )

    @classmethod
    def invalid_arguments(cls, error: str) -> ToolResult:
        return cls.failure_result("invalid_arguments", error)

    @classmethod
    def failure_result(cls, category: ToolFailureCategory, error: str) -> ToolResult:
        if category not in _FAILURE_CATEGORIES:
            raise ValueError(f"unknown tool failure category: {category}")
        recovery = (
            "correct_arguments"
            if category == "invalid_arguments"
            else "restore_then_retry"
            if category == "unavailable"
            else "verify_then_retry"
            if category in {"timeout", "partial_side_effect"}
            else "retry"
            if category == "transient"
            else "stop"
        )
        side_effect = "possible" if category == "partial_side_effect" else "none"
        return cls.error_result(error).with_failure(
            {"category": category, "recovery": recovery, "side_effect": side_effect}
        )

    def with_output(self, output: str) -> ToolResult:
        return replace(self, output=output)

    def with_error(self, error: str) -> ToolResult:
        return replace(
            self,
            success=False,
            error=error,
            failure=self.failure
            or {"category": "permanent", "recovery": "stop", "side_effect": "none"},
        )

    def with_failure(self, failure: Mapping[str, Any]) -> ToolResult:
        _validate_failure(failure)
        return replace(self, success=False, failure=dict(failure))

    def with_data(self, data: JsonValue) -> ToolResult:
        return replace(self, data=_json_to_wire(data))

    def with_truncated(self, truncated: bool) -> ToolResult:
        return replace(self, truncated=truncated)

    def with_mime_type(self, mime_type: str) -> ToolResult:
        return replace(self, mime_type=mime_type)

    def with_artifact(self, artifact: Any) -> ToolResult:
        return replace(self, artifact=artifact)

    def with_meta(self, key: str, value: str) -> ToolResult:
        metadata = dict(self.metadata or {})
        metadata[key] = value
        return replace(self, metadata=metadata)

    def with_metadata(self, metadata: Mapping[str, str]) -> ToolResult:
        return replace(self, metadata=dict(metadata))

    def with_model_content(self, content: Mapping[str, Any]) -> ToolResult:
        return replace(self, model_content=(*self.model_content, dict(content)))

    def to_wire(self) -> dict[str, Any]:
        return {
            "kind": self.kind,
            "output": self.output,
            "success": self.success,
            "truncated": self.truncated,
            "artifact": self.artifact,
            "data": self.data,
            "error": self.error,
            "failure": self.failure,
            "metadata": dict(self.metadata or {}),
            "mime_type": self.mime_type,
            "model_content": list(self.model_content),
        }


def _validate_failure(failure: Mapping[str, Any]) -> None:
    if not isinstance(failure, Mapping):
        raise TypeError("tool failure must be an object")
    category = failure.get("category")
    recovery = failure.get("recovery")
    side_effect = failure.get("side_effect")
    if category not in _FAILURE_CATEGORIES:
        raise ValueError(f"unknown tool failure category: {category}")
    if recovery not in _RECOVERY_ACTIONS:
        raise ValueError(f"unknown tool recovery action: {recovery}")
    if side_effect not in _SIDE_EFFECTS:
        raise ValueError(f"unknown tool side effect: {side_effect}")
    retry_after_ms = failure.get("retry_after_ms")
    if retry_after_ms is not None:
        wire_u64(retry_after_ms)
    for field in ("idempotency_key", "postcondition"):
        value = failure.get(field)
        if value is not None and not isinstance(value, str):
            raise TypeError(f"tool failure {field} must be text")


def _validate_kind(kind: Mapping[str, Any]) -> None:
    if not isinstance(kind, Mapping) or not isinstance(kind.get("kind"), str):
        raise TypeError("tool result kind must contain a textual discriminator")
    discriminator = kind["kind"]
    if discriminator in {"text", "json"}:
        return
    required = {
        "image": "mime_type",
        "diff": "unified_diff",
        "file_reference": "path",
        "skill_activation": "name",
        "structured_error": "error_code",
    }
    if discriminator == "table":
        if not isinstance(kind.get("columns"), list) or not isinstance(
            kind.get("rows"), list
        ):
            raise TypeError("table result kind requires columns and rows")
        return
    if discriminator == "command_output":
        exit_code = kind.get("exit_code")
        if exit_code is not None and (
            isinstance(exit_code, bool) or not isinstance(exit_code, int)
        ):
            raise TypeError("command_output result kind requires an integer exit_code")
        return
    field = required.get(discriminator)
    if field is None:
        raise TypeError(f"unknown tool result kind: {discriminator}")
    if not isinstance(kind.get(field), str):
        raise TypeError(f"{discriminator} result kind requires {field}")


def _json_to_wire(value: JsonValue) -> dict[str, Any]:
    """Encode ordinary JSON objects as maps, including kind/value collisions."""

    if isinstance(value, list):
        return {"kind": "list", "value": [_json_to_wire(item) for item in value]}
    if isinstance(value, dict):
        return {
            "kind": "map",
            "value": [
                {
                    "key": {"kind": "string", "value": key},
                    "value": _json_to_wire(item),
                }
                for key, item in value.items()
            ],
        }
    return to_wire(value)
