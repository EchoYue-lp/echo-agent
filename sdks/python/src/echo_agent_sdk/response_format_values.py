from __future__ import annotations

from dataclasses import dataclass
from typing import Any


@dataclass(frozen=True, slots=True)
class JsonSchemaSpec:
    name: str
    schema: Any
    strict: bool = True


@dataclass(frozen=True, slots=True)
class ResponseFormat:
    """Response format values; schema validation remains provider-owned."""

    format_type: str
    json_schema_spec: JsonSchemaSpec | None = None

    @property
    def type(self) -> str:
        return self.format_type

    @classmethod
    def text(cls) -> ResponseFormat:
        return cls("text")

    @classmethod
    def json_object(cls) -> ResponseFormat:
        return cls("json_object")

    @classmethod
    def json_schema(cls, name: str, schema: Any) -> ResponseFormat:
        return cls("json_schema", JsonSchemaSpec(name, schema, True))

    def is_json(self) -> bool:
        return self.format_type in {"json_object", "json_schema"}
