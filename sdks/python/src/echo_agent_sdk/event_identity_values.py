from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass, replace
from typing import Any
from uuid import uuid4


def _non_empty(value: str, label: str) -> str:
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f"{label} must not be empty")
    return value


@dataclass(frozen=True)
class StreamId:
    value: str

    def __post_init__(self) -> None:
        object.__setattr__(self, "value", _non_empty(self.value, "event stream_id"))

    @classmethod
    def new(cls, value: str) -> StreamId:
        return cls(value)

    def as_str(self) -> str:
        return self.value

    def __str__(self) -> str:
        return self.value


@dataclass(frozen=True)
class EventId:
    value: str

    def __post_init__(self) -> None:
        object.__setattr__(self, "value", _non_empty(self.value, "event_id"))

    @classmethod
    def new(cls, value: str) -> EventId:
        return cls(value)

    def as_str(self) -> str:
        return self.value

    def __str__(self) -> str:
        return self.value


def _field(value: Any, name: str) -> str | None:
    if isinstance(value, Mapping):
        result = value.get(name)
    else:
        result = getattr(value, name, None)
    return result if result is None or isinstance(result, str) else str(result)


@dataclass(frozen=True)
class EventIdentity:
    stream_id: StreamId
    turn_id: str
    conversation_id: str | None = None
    run_id: str | None = None
    message_id: str | None = None
    execution_id: str | None = None
    parent_event_id: str | None = None

    def __post_init__(self) -> None:
        if not isinstance(self.stream_id, StreamId):
            object.__setattr__(self, "stream_id", StreamId.new(str(self.stream_id)))
        object.__setattr__(self, "turn_id", _non_empty(self.turn_id, "turn_id"))
        for value, label in (
            (self.conversation_id, "conversation_id"),
            (self.run_id, "run_id"),
            (self.message_id, "message_id"),
            (self.execution_id, "execution_id"),
            (self.parent_event_id, "event_id"),
        ):
            if value is not None:
                _non_empty(value, label)

    @classmethod
    def new(cls, stream_id: str, turn_id: str) -> EventIdentity:
        return cls(StreamId.new(stream_id), turn_id)

    @classmethod
    def for_run(cls, run_id: str) -> EventIdentity:
        run_id = _non_empty(run_id, "run_id")
        return cls(
            StreamId.new(str(uuid4())), run_id, run_id=run_id, execution_id=run_id
        )

    @classmethod
    def for_chat(
        cls,
        conversation_id: str | None,
        turn_id: str,
        message_id: str,
        run_id: str | None = None,
    ) -> EventIdentity:
        return cls(
            StreamId.new(str(uuid4())),
            turn_id,
            conversation_id=conversation_id,
            run_id=run_id,
            message_id=_non_empty(message_id, "message_id"),
        )

    @classmethod
    def from_invocation(cls, invocation: Any | None) -> EventIdentity:
        if invocation is None:
            return cls.from_runtime_context(None)
        if isinstance(invocation, Mapping):
            runtime = invocation.get("runtime")
        else:
            runtime = getattr(invocation, "runtime", None)
        return cls.from_runtime_context(runtime)

    @classmethod
    def from_runtime_context(cls, runtime: Any | None) -> EventIdentity:
        run_id = _field(runtime, "run_id")
        execution_id = _field(runtime, "execution_id")
        turn_id = _field(runtime, "turn_id") or execution_id or run_id or str(uuid4())
        lineage = _field(runtime, "subagent_lineage")
        parent_event_id = _field(runtime, "parent_event_id") or _field(
            lineage, "parent_event_id"
        )
        return cls(
            StreamId.new(str(uuid4())),
            turn_id,
            conversation_id=_field(runtime, "conversation_id"),
            run_id=run_id,
            message_id=_field(runtime, "message_id"),
            execution_id=execution_id,
            parent_event_id=parent_event_id,
        )

    def validate(self) -> None:
        StreamId.new(self.stream_id.as_str())
        _non_empty(self.turn_id, "turn_id")

    def with_conversation_id(self, value: str) -> EventIdentity:
        return replace(self, conversation_id=_non_empty(value, "conversation_id"))

    def with_run_id(self, value: str) -> EventIdentity:
        return replace(self, run_id=_non_empty(value, "run_id"))

    def with_message_id(self, value: str) -> EventIdentity:
        return replace(self, message_id=_non_empty(value, "message_id"))

    def with_execution_id(self, value: str) -> EventIdentity:
        return replace(self, execution_id=_non_empty(value, "execution_id"))

    def with_parent_event_id(self, value: str) -> EventIdentity:
        return replace(self, parent_event_id=_non_empty(value, "event_id"))
