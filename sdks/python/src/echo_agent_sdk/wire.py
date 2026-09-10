from __future__ import annotations

import base64
import codecs
import math
from dataclasses import dataclass
from typing import Any

MIN_I64 = -(1 << 63)
MAX_U64 = (1 << 64) - 1
MAX_HANDLE_ID_CHARS = 256
HANDLE_KINDS = frozenset(
    {
        "agent",
        "session",
        "run",
        "stream",
        "task_run",
        "plan_task",
        "subagent",
        "extension",
        "facade_resource",
    }
)
_DEFAULT_STREAM_CHUNK_BYTES = 16 * 1024


def _normalise_max_chunk_bytes(value: int) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        raise TypeError("max_chunk_bytes must be an integer")
    return max(1, value)


def split_utf8_chunks(text: str, max_chunk_bytes: int) -> list[str]:
    """Split text at Unicode scalar boundaries under a UTF-8 byte cap.

    A cap smaller than one scalar is still allowed: that scalar is returned
    as a single chunk because splitting it would make invalid UTF-8.
    """

    if not isinstance(text, str):
        raise TypeError("text must be a string")
    max_chunk_bytes = _normalise_max_chunk_bytes(max_chunk_bytes)
    if not text:
        return []

    chunks: list[str] = []
    current: list[str] = []
    current_bytes = 0
    for character in text:
        character_bytes = len(character.encode("utf-8"))
        if current and current_bytes + character_bytes > max_chunk_bytes:
            chunks.append("".join(current))
            current = []
            current_bytes = 0
        current.append(character)
        current_bytes += character_bytes
    if current:
        chunks.append("".join(current))
    return chunks


class IncrementalUtf8Decoder:
    """Decode split byte reads while preserving incomplete UTF-8 suffixes.

    Invalid byte sequences are replaced with U+FFFD. Complete output is
    returned in UTF-8 byte-capped chunks; ``finish`` flushes one incomplete
    suffix and clears the decoder state.
    """

    __slots__ = ("_decoder", "_max_chunk_bytes")

    def __init__(self, max_chunk_bytes: int = _DEFAULT_STREAM_CHUNK_BYTES) -> None:
        self._max_chunk_bytes = _normalise_max_chunk_bytes(max_chunk_bytes)
        self._decoder = codecs.getincrementaldecoder("utf-8")("replace")

    def push(self, data: bytes | bytearray | memoryview) -> list[str]:
        """Decode one byte read, retaining an incomplete suffix for later."""

        if not isinstance(data, (bytes, bytearray, memoryview)):
            raise TypeError("data must be bytes-like")
        try:
            decoded = self._decoder.decode(bytes(data), final=False)
        except ValueError as error:
            raise TypeError("data must be bytes-like") from error
        return split_utf8_chunks(decoded, self._max_chunk_bytes)

    def finish(self) -> str | None:
        """Flush an incomplete suffix using U+FFFD, if one is pending."""

        decoded = self._decoder.decode(b"", final=True)
        return decoded or None


def extract_json_from_markdown(content: str) -> str:
    """Extract JSON from a fenced Markdown block or return trimmed text."""

    if not isinstance(content, str):
        raise TypeError("content must be a string")
    start = content.find("```json")
    if start >= 0:
        rest = content[start + 7 :]
        end = rest.find("```")
        if end >= 0:
            return rest[:end].strip()
    start = content.find("```")
    if start >= 0:
        rest = content[start + 3 :]
        end = rest.find("```")
        if end >= 0:
            return rest[:end].strip()
    return content.strip()


def clean_json(value: str) -> str:
    """Remove trailing commas before ``]``/``}` outside double-quoted text."""

    if not isinstance(value, str):
        raise TypeError("value must be a string")
    characters = list(value)
    cleaned: list[str] = []
    in_string = False
    escaped = False
    index = 0
    while index < len(characters):
        character = characters[index]
        if in_string:
            cleaned.append(character)
            if escaped:
                escaped = False
            elif character == "\\":
                escaped = True
            elif character == '"':
                in_string = False
            index += 1
            continue
        if character == '"':
            in_string = True
            cleaned.append(character)
            index += 1
            continue
        if character == ",":
            lookahead = index + 1
            while lookahead < len(characters) and characters[lookahead].isspace():
                lookahead += 1
            if lookahead < len(characters) and characters[lookahead] in "}]":
                index += 1
                continue
        cleaned.append(character)
        index += 1
    return "".join(cleaned)


def _integer_text(value: int | str, *, signed: bool) -> str:
    if isinstance(value, bool) or not isinstance(value, (int, str)):
        raise TypeError("wire integer must be a canonical decimal value")
    try:
        parsed = int(value)
    except (TypeError, ValueError) as error:
        raise TypeError("wire integer must be a canonical decimal value") from error
    lower = MIN_I64 if signed else 0
    upper = (1 << 63) - 1 if signed else MAX_U64
    if not lower <= parsed <= upper:
        raise OverflowError("integer is outside the i64/u64 wire range")
    text = str(parsed)
    if isinstance(value, str) and value != text:
        raise ValueError("wire integer string must be canonical decimal text")
    return text


def wire_u64(value: int | str) -> dict[str, Any]:
    return {"kind": "u64", "value": _integer_text(value, signed=False)}


def wire_i64(value: int | str) -> dict[str, Any]:
    return {"kind": "i64", "value": _integer_text(value, signed=True)}


def wire_bytes(value: bytes | bytearray | memoryview) -> dict[str, Any]:
    encoded = base64.b64encode(bytes(value)).decode("ascii").rstrip("=")
    return {"kind": "bytes", "value": {"base64": encoded}}


def wire_utf8_path(path: str) -> dict[str, Any]:
    if not isinstance(path, str):
        raise TypeError("wire UTF-8 path must be text")
    if "\x00" in path or not (
        path.startswith(("/", "\\\\"))
        or (
            len(path) >= 3
            and path[0].isascii()
            and path[0].isalpha()
            and path[1] == ":"
            and path[2] in "/\\"
        )
    ):
        raise ValueError("wire UTF-8 path must be absolute and NUL-free")
    return {"kind": "path", "value": {"encoding": "utf8", "path": path}}


def wire_duration(seconds: int | str, nanos: int) -> dict[str, Any]:
    if isinstance(nanos, bool) or not isinstance(nanos, int):
        raise TypeError("wire duration nanos must be an integer")
    if not 0 <= nanos < 1_000_000_000:
        raise ValueError("wire duration nanos must be in [0, 1_000_000_000)")
    return {
        "kind": "duration",
        "value": {"seconds": _integer_text(seconds, signed=False), "nanos": nanos},
    }


def wire_timestamp(
    unix_seconds: int | str, nanos: int, rfc3339: str | None = None
) -> dict[str, Any]:
    if isinstance(nanos, bool) or not isinstance(nanos, int):
        raise TypeError("wire timestamp nanos must be an integer")
    if not 0 <= nanos < 1_000_000_000:
        raise ValueError("wire timestamp nanos must be in [0, 1_000_000_000)")
    if rfc3339 is not None and not isinstance(rfc3339, str):
        raise TypeError("wire timestamp rfc3339 must be text")
    value: dict[str, Any] = {
        "unix_seconds": _integer_text(unix_seconds, signed=True),
        "nanos": nanos,
    }
    if rfc3339 is not None:
        value["rfc3339"] = rfc3339
    return {"kind": "timestamp", "value": value}


@dataclass(frozen=True, slots=True)
class WireHandle:
    id: str
    generation: str
    kind: str

    def __post_init__(self) -> None:
        if not isinstance(self.id, str):
            raise TypeError("handle id must be text")
        if not self.id.strip():
            raise ValueError("handle id must be non-empty")
        if len(self.id) > MAX_HANDLE_ID_CHARS:
            raise ValueError("handle id exceeds 256 characters")
        if not isinstance(self.generation, str):
            raise TypeError("handle generation must be canonical decimal text")
        _integer_text(self.generation, signed=False)
        if not isinstance(self.kind, str):
            raise TypeError("handle kind must be text")
        if self.kind not in HANDLE_KINDS:
            raise ValueError(f"unknown handle kind: {self.kind}")

    @classmethod
    def from_dict(cls, value: Any) -> WireHandle:
        if not isinstance(value, dict):
            raise TypeError("handle must be an object")
        return cls(value.get("id"), value.get("generation"), value.get("kind"))

    def to_dict(self) -> dict[str, str]:
        return {"id": self.id, "generation": self.generation, "kind": self.kind}


def parse_wire_handle(value: Any) -> WireHandle:
    """Decode and validate a Host-issued generation-fenced handle."""

    return WireHandle.from_dict(value)


def _wire_integer_text(value: Any, *, signed: bool) -> str:
    """Validate the JSON representation, which must be a decimal string."""

    if not isinstance(value, str):
        raise TypeError("wire integer value must be canonical decimal text")
    return _integer_text(value, signed=signed)


def to_wire(value: Any) -> dict[str, Any]:
    if value is None:
        return {"kind": "null"}
    if isinstance(value, WireHandle):
        return {"kind": "handle", "value": value.to_dict()}
    if isinstance(value, dict) and isinstance(value.get("kind"), str):
        kind = value["kind"]
        if kind == "handle":
            WireHandle.from_dict(value.get("value"))
        elif kind in {"i64", "u64"}:
            _wire_integer_text(value.get("value"), signed=kind == "i64")
        return value
    if isinstance(value, bool):
        return {"kind": "bool", "value": value}
    if isinstance(value, str):
        return {"kind": "string", "value": value}
    if isinstance(value, int):
        if value < MIN_I64 or value > MAX_U64:
            raise OverflowError("integer is outside the i64/u64 wire range")
        return {"kind": "i64" if value < 0 else "u64", "value": str(value)}
    if isinstance(value, float):
        if not math.isfinite(value):
            raise ValueError("wire floats must be finite")
        return {"kind": "f64", "value": value}
    if isinstance(value, (list, tuple)):
        return {"kind": "list", "value": [to_wire(item) for item in value]}
    if isinstance(value, dict):
        if any(not isinstance(key, str) for key in value):
            raise TypeError("wire map keys must be strings")
        return {
            "kind": "map",
            "value": [
                {"key": {"kind": "string", "value": key}, "value": to_wire(item)}
                for key, item in value.items()
            ],
        }
    raise TypeError(f"unsupported wire value type: {type(value).__name__}")


def from_wire(value: Any) -> Any:
    if not isinstance(value, dict) or not isinstance(value.get("kind"), str):
        return value
    kind = value["kind"]
    if kind == "null":
        return None
    if kind == "bool":
        raw = value.get("value")
        if not isinstance(raw, bool):
            raise TypeError("wire bool value must be boolean")
        return raw
    if kind == "string":
        raw = value.get("value")
        if not isinstance(raw, str):
            raise TypeError("wire string value must be text")
        return raw
    if kind == "f64":
        raw = value.get("value")
        if isinstance(raw, bool) or not isinstance(raw, (int, float)):
            raise TypeError("wire f64 value must be numeric")
        if not math.isfinite(raw):
            raise ValueError("wire floats must be finite")
        return raw
    if kind == "i64":
        return _wire_integer_text(value.get("value"), signed=True)
    if kind == "u64":
        return _wire_integer_text(value.get("value"), signed=False)
    if kind == "handle":
        return WireHandle.from_dict(value.get("value"))
    if kind == "list":
        return [from_wire(item) for item in value.get("value", [])]
    if kind == "map":
        result: dict[str, Any] = {}
        for entry in value.get("value", []):
            if not isinstance(entry, dict):
                continue
            key = entry.get("key")
            if isinstance(key, dict) and key.get("kind") == "string":
                result[str(key.get("value", ""))] = from_wire(entry.get("value"))
        return result
    # Keep the discriminator for typed and future additive variants so values
    # remain observable and round-trip without a language-specific schema.
    return value
