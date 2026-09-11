from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass, replace
from datetime import datetime, timezone
from enum import Enum
from typing import Any


@dataclass(frozen=True, slots=True)
class A2AMessage:
    role: str
    parts: tuple[Mapping[str, Any], ...]

    @classmethod
    def user_text(cls, text: str) -> A2AMessage:
        if not isinstance(text, str):
            raise TypeError("message text must be text")
        return cls("user", ({"type": "text", "text": text},))

    @classmethod
    def agent_text(cls, text: str) -> A2AMessage:
        if not isinstance(text, str):
            raise TypeError("message text must be text")
        return cls("agent", ({"type": "text", "text": text},))

    def text_content(self) -> str:
        return "\n".join(
            str(part["text"])
            for part in self.parts
            if part.get("type") == "text" and isinstance(part.get("text"), str)
        )


@dataclass(frozen=True, slots=True)
class A2ATaskStatus:
    state: TaskState
    message: A2AMessage | None
    timestamp: str

    @classmethod
    def new(cls, state: TaskState) -> A2ATaskStatus:
        if not isinstance(state, TaskState):
            raise TypeError(f"unknown A2A task state: {state}")
        return cls(state, None, datetime.now(timezone.utc).isoformat())

    @classmethod
    def with_message(cls, state: TaskState, message: A2AMessage) -> A2ATaskStatus:
        if not isinstance(state, TaskState):
            raise TypeError(f"unknown A2A task state: {state}")
        if not isinstance(message, A2AMessage):
            raise TypeError("A2A task status message must be an A2AMessage")
        return cls(state, message, datetime.now(timezone.utc).isoformat())


@dataclass(frozen=True, slots=True)
class AgentProvider:
    organization: str
    url: str | None = None

    @classmethod
    def new(cls, organization: str) -> AgentProvider:
        if not isinstance(organization, str):
            raise TypeError("organization must be text")
        return cls(organization)

    def with_url(self, url: str) -> AgentProvider:
        if not isinstance(url, str):
            raise TypeError("provider url must be text")
        return replace(self, url=url)


@dataclass(frozen=True, slots=True)
class AgentSkill:
    id: str
    name: str
    description: str | None
    examples: tuple[str, ...]
    input_modes: tuple[str, ...]
    output_modes: tuple[str, ...]
    tags: tuple[str, ...]

    @classmethod
    def new(cls, name: str, description: str) -> AgentSkill:
        if not isinstance(name, str) or not isinstance(description, str):
            raise TypeError("skill name and description must be text")
        return cls(name, name, description, (), (), (), ())

    def with_examples(self, examples: list[str] | tuple[str, ...]) -> AgentSkill:
        return replace(self, examples=_text_list(examples, "examples"))

    def with_tags(self, tags: list[str] | tuple[str, ...]) -> AgentSkill:
        return replace(self, tags=_text_list(tags, "tags"))


def _text_list(values: list[str] | tuple[str, ...], field: str) -> tuple[str, ...]:
    if not isinstance(values, (list, tuple)) or any(
        not isinstance(value, str) for value in values
    ):
        raise TypeError(f"{field} must be a string sequence")
    return tuple(values)


class TaskState(str, Enum):
    """Closed A2A task state with the Rust transition rules."""

    SUBMITTED = "submitted"
    WORKING = "working"
    INPUT_REQUIRED = "input-required"
    COMPLETED = "completed"
    FAILED = "failed"
    CANCELED = "canceled"

    def is_terminal(self) -> bool:
        return self in {self.COMPLETED, self.FAILED, self.CANCELED}

    def can_transition_to(self, next_state: TaskState) -> bool:
        if not isinstance(next_state, TaskState):
            raise TypeError(f"unknown A2A task state: {next_state}")
        if self.is_terminal():
            return False
        return (self, next_state) in {
            (self.SUBMITTED, self.WORKING),
            (self.SUBMITTED, self.CANCELED),
            (self.WORKING, self.COMPLETED),
            (self.WORKING, self.FAILED),
            (self.WORKING, self.INPUT_REQUIRED),
            (self.WORKING, self.CANCELED),
            (self.INPUT_REQUIRED, self.WORKING),
            (self.INPUT_REQUIRED, self.CANCELED),
        }

    def __str__(self) -> str:
        return self.value
