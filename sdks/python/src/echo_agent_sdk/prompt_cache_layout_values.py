from __future__ import annotations

from collections.abc import Mapping, Sequence
from dataclasses import dataclass, field
from typing import Any

from .segment_range_values import SegmentRange


@dataclass(frozen=True, slots=True)
class SegmentRanges:
    """Message-cache segment indexes without owning cache state."""

    system: SegmentRange = field(default_factory=SegmentRange)
    canonical: SegmentRange = field(default_factory=SegmentRange)
    history: SegmentRange = field(default_factory=SegmentRange)
    runtime_context: SegmentRange = field(default_factory=SegmentRange)


@dataclass(frozen=True, slots=True)
class PromptCacheLayout:
    """Read-only prompt layout projection; cache placement stays in Rust."""

    system: tuple[Mapping[str, Any], ...]
    canonical: tuple[Mapping[str, Any], ...]
    history: tuple[Mapping[str, Any], ...]
    runtime_context: tuple[Mapping[str, Any], ...]
    tools: tuple[Mapping[str, Any], ...]

    @classmethod
    def from_messages(
        cls,
        messages: Sequence[Mapping[str, Any]],
        tools: Sequence[Mapping[str, Any]],
    ) -> PromptCacheLayout:
        if not isinstance(messages, Sequence) or isinstance(messages, (str, bytes)):
            raise TypeError("messages must be a sequence")
        if not isinstance(tools, Sequence) or isinstance(tools, (str, bytes)):
            raise TypeError("tools must be a sequence")
        message_values = _snapshot_values(messages, "messages")
        tool_values = _snapshot_values(tools, "tools")

        system_end = next(
            (
                index
                for index, message in enumerate(message_values)
                if message.get("role") != "system"
            ),
            len(message_values),
        )
        canonical_start = next(
            (
                index
                for index, message in enumerate(message_values[:system_end])
                if (_message_text(message) or "").find("Canonical context") >= 0
            ),
            system_end,
        )

        runtime_start = len(message_values)
        for index in range(len(message_values) - 1, -1, -1):
            text = _message_text(message_values[index])
            if text is not None and text.lstrip().startswith("[runtime_context:"):
                runtime_start = index
                while runtime_start > system_end:
                    previous = _message_text(message_values[runtime_start - 1])
                    if previous is None or not previous.lstrip().startswith(
                        "[runtime_context:"
                    ):
                        break
                    runtime_start -= 1
                break

        return cls(
            tuple(message_values[: min(canonical_start, system_end)]),
            tuple(message_values[canonical_start:system_end])
            if canonical_start < system_end
            else (),
            tuple(message_values[system_end:runtime_start]),
            tuple(message_values[runtime_start:]),
            tuple(tool_values),
        )

    def segment_ranges(self) -> SegmentRanges:
        system = len(self.system)
        canonical = len(self.canonical)
        history = len(self.history)
        runtime_context = len(self.runtime_context)
        canonical_end = system + canonical
        history_end = canonical_end + history
        return SegmentRanges(
            system=SegmentRange(0, system),
            canonical=SegmentRange(system, canonical_end),
            history=SegmentRange(canonical_end, history_end),
            runtime_context=SegmentRange(history_end, history_end + runtime_context),
        )


def _snapshot_values(
    values: Sequence[Mapping[str, Any]], name: str
) -> list[dict[str, Any]]:
    result: list[dict[str, Any]] = []
    for value in values:
        if not isinstance(value, Mapping):
            raise TypeError(f"{name} entries must be objects")
        result.append(dict(value))
    return result


def _message_text(message: Mapping[str, Any]) -> str | None:
    content = message.get("content")
    if isinstance(content, str):
        return content
    if isinstance(content, Mapping) and content.get("kind") == "string":
        value = content.get("value")
        return value if isinstance(value, str) else None
    return None
