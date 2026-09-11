from __future__ import annotations

import sys
from collections.abc import Mapping
from dataclasses import dataclass, field, replace
from datetime import datetime, timezone
from enum import Enum
from types import MappingProxyType
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
class A2AArtifact:
    parts: tuple[Mapping[str, Any], ...]
    name: str | None = None
    index: int | None = None
    append: bool = False

    def __post_init__(self) -> None:
        if self.name is not None and not isinstance(self.name, str):
            raise TypeError("artifact name must be text")
        if self.index is not None and (
            isinstance(self.index, bool)
            or not isinstance(self.index, int)
            or self.index < 0
            or self.index > sys.maxsize
        ):
            raise TypeError("artifact index must fit Rust usize")
        if not isinstance(self.append, bool):
            raise TypeError("artifact append must be boolean")
        object.__setattr__(
            self, "parts", tuple(_freeze_part(part) for part in self.parts)
        )

    @classmethod
    def new(
        cls,
        parts: list[Mapping[str, Any]] | tuple[Mapping[str, Any], ...],
        *,
        name: str | None = None,
        index: int | None = None,
        append: bool = False,
    ) -> A2AArtifact:
        if not isinstance(parts, (list, tuple)):
            raise TypeError("artifact parts must be a sequence")
        return cls(tuple(parts), name=name, index=index, append=append)


@dataclass(frozen=True, slots=True)
class A2AError:
    code: int
    message: str

    def __post_init__(self) -> None:
        if (
            isinstance(self.code, bool)
            or not isinstance(self.code, int)
            or not -(1 << 31) <= self.code <= (1 << 31) - 1
        ):
            raise TypeError("A2A error code must fit i32")
        if not isinstance(self.message, str):
            raise TypeError("A2A error message must be text")

    @classmethod
    def new(cls, code: int, message: str) -> A2AError:
        return cls(code, message)


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


@dataclass(frozen=True, slots=True)
class AuthenticationScheme:
    scheme: str
    config: Mapping[str, Any] = field(default_factory=dict)

    def __post_init__(self) -> None:
        if not isinstance(self.scheme, str):
            raise TypeError("authentication scheme must be text")
        if not isinstance(self.config, Mapping):
            raise TypeError("authentication config must be a mapping")
        object.__setattr__(self, "config", MappingProxyType(dict(self.config)))


@dataclass(frozen=True, slots=True)
class AgentAuthentication:
    schemes: tuple[AuthenticationScheme, ...]

    def __post_init__(self) -> None:
        if any(not isinstance(scheme, AuthenticationScheme) for scheme in self.schemes):
            raise TypeError(
                "authentication schemes must be AuthenticationScheme values"
            )
        object.__setattr__(self, "schemes", tuple(self.schemes))


@dataclass(frozen=True, slots=True)
class AgentCapabilities:
    streaming: bool = False
    push_notifications: bool = False
    state_transition_history: bool = False


@dataclass(frozen=True, slots=True)
class AgentCard:
    name: str
    url: str
    description: str | None = None
    version: str | None = None
    provider: AgentProvider | None = None
    skills: tuple[AgentSkill, ...] = ()
    default_input_modes: tuple[str, ...] = ("text/plain",)
    default_output_modes: tuple[str, ...] = ("text/plain",)
    authentication: AgentAuthentication | None = None
    capabilities: AgentCapabilities = field(default_factory=AgentCapabilities)

    def __post_init__(self) -> None:
        _require_text(self.name, "agent name")
        _require_text(self.url, "agent url")
        if self.description is not None:
            _require_text(self.description, "agent description")
        if self.version is not None:
            _require_text(self.version, "agent version")
        if self.provider is not None and not isinstance(self.provider, AgentProvider):
            raise TypeError("provider must be an AgentProvider")
        if any(not isinstance(skill, AgentSkill) for skill in self.skills):
            raise TypeError("skills must be AgentSkill values")
        object.__setattr__(self, "skills", tuple(self.skills))
        object.__setattr__(
            self,
            "default_input_modes",
            _text_list(self.default_input_modes, "input modes"),
        )
        object.__setattr__(
            self,
            "default_output_modes",
            _text_list(self.default_output_modes, "output modes"),
        )
        if self.authentication is not None and not isinstance(
            self.authentication, AgentAuthentication
        ):
            raise TypeError("authentication must be an AgentAuthentication")
        if not isinstance(self.capabilities, AgentCapabilities):
            raise TypeError("capabilities must be AgentCapabilities")

    @classmethod
    def builder(cls, name: str, url: str) -> AgentCardBuilder:
        return AgentCardBuilder(name, url)


class AgentCardBuilder:
    def __init__(self, name: str, url: str) -> None:
        _require_text(name, "agent name")
        _require_text(url, "agent url")
        self._name = name
        self._url = url
        self._description: str | None = None
        self._version: str | None = None
        self._provider: AgentProvider | None = None
        self._skills: list[AgentSkill] = []
        self._input_modes: tuple[str, ...] = ("text/plain",)
        self._output_modes: tuple[str, ...] = ("text/plain",)
        self._authentication: AgentAuthentication | None = None
        self._streaming = False
        self._push_notifications = False

    def description(self, value: str) -> AgentCardBuilder:
        _require_text(value, "agent description")
        self._description = value
        return self

    def version(self, value: str) -> AgentCardBuilder:
        _require_text(value, "agent version")
        self._version = value
        return self

    def provider(self, value: AgentProvider) -> AgentCardBuilder:
        if not isinstance(value, AgentProvider):
            raise TypeError("provider must be an AgentProvider")
        self._provider = value
        return self

    def skill(self, value: AgentSkill) -> AgentCardBuilder:
        if not isinstance(value, AgentSkill):
            raise TypeError("skill must be an AgentSkill")
        self._skills.append(value)
        return self

    def skills(
        self, values: list[AgentSkill] | tuple[AgentSkill, ...]
    ) -> AgentCardBuilder:
        if not isinstance(values, (list, tuple)) or any(
            not isinstance(value, AgentSkill) for value in values
        ):
            raise TypeError("skills must be an AgentSkill sequence")
        self._skills.extend(values)
        return self

    def input_modes(self, values: list[str] | tuple[str, ...]) -> AgentCardBuilder:
        self._input_modes = _text_list(values, "input modes")
        return self

    def output_modes(self, values: list[str] | tuple[str, ...]) -> AgentCardBuilder:
        self._output_modes = _text_list(values, "output modes")
        return self

    def authentication(self, value: AgentAuthentication) -> AgentCardBuilder:
        if not isinstance(value, AgentAuthentication):
            raise TypeError("authentication must be an AgentAuthentication")
        self._authentication = value
        return self

    def streaming(self) -> AgentCardBuilder:
        self._streaming = True
        return self

    def push_notifications(self) -> AgentCardBuilder:
        self._push_notifications = True
        return self

    def build(self) -> AgentCard:
        return AgentCard(
            name=self._name,
            url=self._url,
            description=self._description,
            version=self._version,
            provider=self._provider,
            skills=tuple(self._skills),
            default_input_modes=self._input_modes,
            default_output_modes=self._output_modes,
            authentication=self._authentication,
            capabilities=AgentCapabilities(
                streaming=self._streaming,
                push_notifications=self._push_notifications,
            ),
        )


def _text_list(values: list[str] | tuple[str, ...], field: str) -> tuple[str, ...]:
    if not isinstance(values, (list, tuple)) or any(
        not isinstance(value, str) for value in values
    ):
        raise TypeError(f"{field} must be a string sequence")
    return tuple(values)


def _require_text(value: Any, field: str) -> None:
    if not isinstance(value, str):
        raise TypeError(f"{field} must be text")


def _freeze_part(part: Mapping[str, Any]) -> Mapping[str, Any]:
    if not isinstance(part, Mapping):
        raise TypeError("artifact parts must be mappings")
    part_type = part.get("type")
    if part_type == "text" and not isinstance(part.get("text"), str):
        raise TypeError("artifact text must be text")
    if part_type == "file" and (
        not isinstance(part.get("mimeType"), str)
        or not isinstance(part.get("data"), str)
    ):
        raise TypeError("artifact file part must contain text mimeType and data")
    if part_type not in {"text", "file"}:
        raise TypeError("artifact part type must be text or file")
    return MappingProxyType(dict(part))


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
