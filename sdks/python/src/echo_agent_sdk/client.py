from __future__ import annotations

import asyncio
import contextlib
import inspect
import math
import os
from collections import deque
from collections.abc import AsyncIterator, Awaitable, Callable, Iterable, Mapping
from dataclasses import dataclass, field
from datetime import timedelta
from pathlib import Path
from types import TracebackType
from typing import Any, Literal, Protocol, TypedDict, cast

from acp.connection import StreamDirection, StreamEvent
from acp.schema import ClientCapabilities, Implementation
from acp.stdio import spawn_agent_process
from typing_extensions import Self

from .catalog import FacadeCatalog
from .errors import EchoAgentError
from .wire import MAX_U64, WireHandle, from_wire, to_wire

ExtensionKind = Literal[
    "tool",
    "llm_client",
    "store",
    "critic",
    "human_loop_provider",
    "hook",
    "agent_callback",
    "intervention_callback",
    "agent_factory",
    "custom_agent",
    "channel_plugin",
    "channel_message_handler",
    "context_compressor",
    "agent_component",
]

AgentComponentKind = Literal[
    "conversation_store",
    "run_store",
    "runtime_state_store",
    "audit_logger",
    "context_projector",
    "memory_trigger_sink",
    "guard",
    "search_provider",
    "workflow_checkpoint_store",
    "revisioned_task_store",
    "sandbox_executor",
    "mcp_transport",
    "embedder",
    "memory_promoter",
    "workflow",
    "intent_classifier",
    "skill_load_policy",
]

AGENT_COMPONENT_OPERATIONS = frozenset(
    {
        "conversation_create",
        "conversation_get",
        "conversation_list",
        "conversation_update",
        "conversation_delete",
        "conversation_save_messages",
        "conversation_get_messages",
        "conversation_count_messages",
        "conversation_ensure",
        "conversation_search",
        "run_save",
        "run_load",
        "run_list_by_session",
        "run_list_all",
        "run_append_event",
        "run_list_by_parent",
        "runtime_get_checkpoint",
        "runtime_save_checkpoint",
        "runtime_save_checkpoint_for_scope",
        "runtime_state_ids",
        "runtime_clear_state",
        "runtime_clear_scope",
        "runtime_clear_conversation",
        "audit_log",
        "audit_query",
        "context_project",
        "memory_trigger",
        "guard_check",
        "search_provider_search",
        "workflow_checkpoint_save",
        "workflow_checkpoint_save_if_generation",
        "workflow_checkpoint_load",
        "workflow_checkpoint_claim",
        "workflow_checkpoint_ack_claim",
        "workflow_checkpoint_requeue_claim",
        "workflow_checkpoint_renew_claim",
        "workflow_checkpoint_list",
        "workflow_checkpoint_list_by_graph",
        "workflow_checkpoint_list_filtered",
        "workflow_checkpoint_delete",
        "workflow_checkpoint_clear",
        "revisioned_task_load",
        "revisioned_task_compare_and_commit",
        "sandbox_is_available",
        "sandbox_execute",
        "sandbox_execute_stream",
        "sandbox_execute_with_limits",
        "sandbox_execute_with_limits_and_cancel",
        "sandbox_cleanup",
        "mcp_transport_send",
        "mcp_transport_notify",
        "mcp_transport_close",
        "mcp_transport_try_notification",
        "embedder_embed",
        "memory_promoter_promote",
        "workflow_run",
        "workflow_run_stream",
        "intent_classify",
        "skill_load_allows",
    }
)

_AGENT_COMPONENTS = frozenset(
    {
        "conversation_store",
        "run_store",
        "runtime_state_store",
        "audit_logger",
        "context_projector",
        "memory_trigger_sink",
        "guard",
        "search_provider",
        "workflow_checkpoint_store",
        "revisioned_task_store",
        "sandbox_executor",
        "mcp_transport",
        "embedder",
        "memory_promoter",
        "workflow",
        "intent_classifier",
        "skill_load_policy",
    }
)

_AGENT_COMPONENT_INPUT_FIELDS: dict[str, frozenset[str]] = {
    "conversation_create": frozenset({"conversation"}),
    "conversation_get": frozenset({"conversation_id"}),
    "conversation_list": frozenset({"user_id", "agent_type", "limit", "offset"}),
    "conversation_update": frozenset(
        {"conversation_id", "title", "summary", "compressed_before_id"}
    ),
    "conversation_delete": frozenset({"conversation_id"}),
    "conversation_save_messages": frozenset({"conversation_id", "messages"}),
    "conversation_get_messages": frozenset({"conversation_id"}),
    "conversation_count_messages": frozenset({"conversation_id"}),
    "conversation_ensure": frozenset({"conversation"}),
    "conversation_search": frozenset({"query", "limit"}),
    "run_save": frozenset({"run"}),
    "run_load": frozenset({"run_id"}),
    "run_list_by_session": frozenset({"session_id"}),
    "run_list_all": frozenset({"limit"}),
    "run_append_event": frozenset({"run_id", "event"}),
    "run_list_by_parent": frozenset({"parent_run_id"}),
    "runtime_get_checkpoint": frozenset({"conversation_id"}),
    "runtime_save_checkpoint": frozenset({"checkpoint"}),
    "runtime_save_checkpoint_for_scope": frozenset({"scope_id", "checkpoint"}),
    "runtime_state_ids": frozenset({"scope_id"}),
    "runtime_clear_state": frozenset({"scope_id", "runtime_state_id"}),
    "runtime_clear_scope": frozenset({"scope_id"}),
    "runtime_clear_conversation": frozenset({"conversation_id"}),
    "audit_log": frozenset({"event"}),
    "audit_query": frozenset({"session_id", "agent_name", "from", "to", "limit"}),
    "context_project": frozenset(
        {
            "iteration",
            "agent_name",
            "session_id",
            "conversation_id",
            "run_id",
            "turn_id",
        }
    ),
    "memory_trigger": frozenset({"trigger"}),
    "guard_check": frozenset({"content", "direction"}),
    "search_provider_search": frozenset({"query", "max_results"}),
    "workflow_checkpoint_save": frozenset({"checkpoint"}),
    "workflow_checkpoint_save_if_generation": frozenset(
        {"checkpoint", "expected_generation"}
    ),
    "workflow_checkpoint_load": frozenset({"checkpoint_id"}),
    "workflow_checkpoint_claim": frozenset({"checkpoint_id"}),
    "workflow_checkpoint_ack_claim": frozenset({"checkpoint_id", "attempt_id"}),
    "workflow_checkpoint_requeue_claim": frozenset({"checkpoint_id", "attempt_id"}),
    "workflow_checkpoint_renew_claim": frozenset({"checkpoint_id", "attempt_id"}),
    "workflow_checkpoint_list": frozenset(),
    "workflow_checkpoint_list_by_graph": frozenset({"graph_name"}),
    "workflow_checkpoint_list_filtered": frozenset({"filter"}),
    "workflow_checkpoint_delete": frozenset({"checkpoint_id"}),
    "workflow_checkpoint_clear": frozenset(),
    "revisioned_task_load": frozenset({"scope_id"}),
    "revisioned_task_compare_and_commit": frozenset({"scope_id", "commit"}),
    "sandbox_is_available": frozenset(),
    "sandbox_execute": frozenset({"command"}),
    "sandbox_execute_stream": frozenset({"command"}),
    "sandbox_execute_with_limits": frozenset({"command", "limits"}),
    "sandbox_execute_with_limits_and_cancel": frozenset({"command", "limits"}),
    "sandbox_cleanup": frozenset(),
    "mcp_transport_send": frozenset({"request"}),
    "mcp_transport_notify": frozenset({"notification"}),
    "mcp_transport_close": frozenset(),
    "mcp_transport_try_notification": frozenset(),
    "embedder_embed": frozenset({"text"}),
    "memory_promoter_promote": frozenset({"evicted"}),
    "workflow_run": frozenset({"input"}),
    "workflow_run_stream": frozenset({"input"}),
    "intent_classify": frozenset({"user_input", "context"}),
    "skill_load_allows": frozenset({"descriptor"}),
}


def _component_for_operation(operation: str) -> str | None:
    if operation.startswith("conversation_"):
        return "conversation_store"
    if operation.startswith("run_"):
        return "run_store"
    if operation.startswith("runtime_"):
        return "runtime_state_store"
    if operation.startswith("audit_"):
        return "audit_logger"
    exact = {
        "context_project": "context_projector",
        "memory_trigger": "memory_trigger_sink",
        "guard_check": "guard",
        "search_provider_search": "search_provider",
        "embedder_embed": "embedder",
        "memory_promoter_promote": "memory_promoter",
        "workflow_run": "workflow",
        "workflow_run_stream": "workflow",
        "intent_classify": "intent_classifier",
        "skill_load_allows": "skill_load_policy",
    }
    if operation in exact:
        return exact[operation]
    if operation.startswith("workflow_checkpoint_"):
        return "workflow_checkpoint_store"
    if operation.startswith("revisioned_task_"):
        return "revisioned_task_store"
    if operation.startswith("sandbox_"):
        return "sandbox_executor"
    if operation.startswith("mcp_transport_"):
        return "mcp_transport"
    return None


_EXTENSION_ERROR_CODES = frozenset(
    {
        "acp_protocol_mismatch",
        "extension_version_mismatch",
        "extension_digest_mismatch",
        "extension_capability_mismatch",
        "invalid_request",
        "invalid_config",
        "invalid_value",
        "feature_unavailable",
        "stale_handle",
        "closed_handle",
        "framework_error",
        "extension_rejected",
        "extension_failed",
        "extension_timeout",
        "extension_disconnected",
        "extension_conflict",
        "cancelled",
        "host_shutting_down",
        "host_exited",
        "event_gap",
        "replay_unavailable",
        "payload_too_large",
        "serialization_violation",
    }
)


class ExtensionDescriptor(TypedDict):
    kind: ExtensionKind
    descriptor_version: int


class ExecutionUsage(TypedDict):
    """Lossless usage snapshot returned by a settled Run receipt."""

    duration_ms: str
    tokens_used: str | None
    iterations: str | None


RunUsage = ExecutionUsage


_WIRE_VALUE_KINDS = frozenset(
    {
        "null",
        "bool",
        "string",
        "i64",
        "u64",
        "f64",
        "bytes",
        "duration",
        "timestamp",
        "path",
        "handle",
        "list",
        "map",
        "record",
        "variant",
        "unknown",
    }
)


def _descriptor_value(value: Any) -> Any:
    """Keep wire-shaped values intact while accepting plain JSON helpers."""

    if (
        isinstance(value, Mapping)
        and isinstance(value.get("kind"), str)
        and value["kind"] in _WIRE_VALUE_KINDS
    ):
        return dict(value)
    return to_wire(value)


def _canonical_u64_text(value: Any, name: str) -> str:
    if isinstance(value, bool):
        raise TypeError(f"{name} must be canonical u64 text")
    text = str(value)
    if (
        not text.isascii()
        or not text.isdigit()
        or (len(text) > 1 and text.startswith("0"))
    ):
        raise ValueError(f"{name} must be canonical u64 text")
    try:
        parsed = int(text)
    except ValueError as error:
        raise ValueError(f"{name} must be canonical u64 text") from error
    if parsed > MAX_U64:
        raise ValueError(f"{name} exceeds u64 range")
    return text


@dataclass(frozen=True, slots=True)
class ToolDescriptor:
    """Typed registration descriptor for a host-language Tool."""

    name: str
    description: str = ""
    parameters: Any = field(
        default_factory=lambda: {
            "kind": "record",
            "value": {"type_id": "json_schema", "fields": []},
        }
    )
    schema_revision: str = "1"
    required_input_modalities: tuple[str, ...] = ()
    required_permissions: tuple[str, ...] = ()
    risk_level: str = "standard"
    supports_streaming: bool = False
    exempt_from_batch_timeout: bool = False
    allows_parallel_batch_execution: bool = True
    manages_own_timeout: bool = False

    def to_wire(self) -> ExtensionDescriptor | dict[str, Any]:
        return {
            "kind": "tool",
            "descriptor_version": 1,
            "name": self.name,
            "description": self.description,
            "parameters": _descriptor_value(self.parameters),
            "schema_revision": _canonical_u64_text(
                self.schema_revision, "schema_revision"
            ),
            "required_input_modalities": list(self.required_input_modalities),
            "required_permissions": list(self.required_permissions),
            "risk_level": self.risk_level,
            "supports_streaming": self.supports_streaming,
            "exempt_from_batch_timeout": self.exempt_from_batch_timeout,
            "allows_parallel_batch_execution": self.allows_parallel_batch_execution,
            "manages_own_timeout": self.manages_own_timeout,
        }


@dataclass(frozen=True, slots=True)
class LlmClientDescriptor:
    """Typed registration descriptor for a host-language LlmClient."""

    model_name: str
    supports_streaming: bool
    capabilities: Mapping[str, Any] | None = None

    def to_wire(self) -> dict[str, Any]:
        descriptor: dict[str, Any] = {
            "kind": "llm_client",
            "descriptor_version": 1,
            "model_name": self.model_name,
            "supports_streaming": self.supports_streaming,
        }
        # Rust's LlmCapabilitiesWire has serde defaults; omitting the object
        # preserves those defaults. An explicitly supplied mapping is sent as
        # provided so additive fields remain visible to the Host validator.
        if self.capabilities is not None:
            descriptor["capabilities"] = dict(self.capabilities)
        return descriptor


@dataclass(frozen=True, slots=True)
class StoreDescriptor:
    """Typed registration descriptor for a host-language Store."""

    search_modes: tuple[str, ...] = ()

    def to_wire(self) -> dict[str, Any]:
        return {
            "kind": "store",
            "descriptor_version": 1,
            "search_modes": list(self.search_modes),
        }


@dataclass(frozen=True, slots=True)
class CriticDescriptor:
    """Typed registration descriptor for a host-language Critic."""

    name: str

    def to_wire(self) -> dict[str, Any]:
        return {
            "kind": "critic",
            "descriptor_version": 1,
            "name": self.name,
        }


@dataclass(frozen=True, slots=True)
class ContextCompressorDescriptor:
    """Typed registration descriptor for a host-language ContextCompressor."""

    name: str

    def to_wire(self) -> dict[str, Any]:
        return {
            "kind": "context_compressor",
            "descriptor_version": 1,
            "name": self.name,
        }


@dataclass(frozen=True, slots=True)
class AgentComponentDescriptor:
    """Typed registration descriptor for a Host-consumed Agent component."""

    component: AgentComponentKind
    name: str
    isolation_level: str | None = None
    supports_streaming: bool = False
    supports_notifications: bool = False
    claim_heartbeat_interval_ms: int | None = None

    def __post_init__(self) -> None:
        if self.component not in _AGENT_COMPONENTS:
            raise ValueError("unknown Agent component kind")
        if not self.name or len(self.name) > 256:
            raise ValueError("Agent component name must contain 1-256 characters")
        if self.supports_streaming and self.component not in {
            "sandbox_executor",
            "workflow",
        }:
            raise ValueError("streaming is only valid for sandbox and workflow")
        if self.isolation_level is not None and self.component != "sandbox_executor":
            raise ValueError("isolation_level is only valid for sandbox_executor")
        if self.isolation_level not in {
            None,
            "none",
            "process",
            "os-sandbox",
            "container",
            "orchestrated",
        }:
            raise ValueError("unknown sandbox isolation level")
        if self.supports_notifications and self.component != "mcp_transport":
            raise ValueError("notifications are only valid for mcp_transport")
        if self.component == "workflow_checkpoint_store":
            if (
                isinstance(self.claim_heartbeat_interval_ms, bool)
                or not isinstance(self.claim_heartbeat_interval_ms, int)
                or not 1 <= self.claim_heartbeat_interval_ms <= 300_000
            ):
                raise ValueError(
                    "workflow checkpoint stores require a claim heartbeat from 1 to 300000 ms"
                )
        elif self.claim_heartbeat_interval_ms is not None:
            raise ValueError(
                "claim_heartbeat_interval_ms is only valid for workflow_checkpoint_store"
            )

    def to_wire(self) -> dict[str, Any]:
        capabilities: dict[str, Any] = {
            "isolation_level": self.isolation_level,
            "supports_streaming": self.supports_streaming,
            "supports_notifications": self.supports_notifications,
        }
        if self.claim_heartbeat_interval_ms is not None:
            capabilities["claim_heartbeat_interval_ms"] = _canonical_u64_text(
                self.claim_heartbeat_interval_ms, "claim_heartbeat_interval_ms"
            )
        return {
            "kind": "agent_component",
            "descriptor_version": 1,
            "component": self.component,
            "name": self.name,
            "capabilities": capabilities,
        }


class ExtensionCall(Protocol):
    """Common metadata shared by typed reverse-invocation dataclasses."""

    operation: str
    extension: WireHandle | None
    invocation_id: str | None
    deadline: Mapping[str, Any] | None


@dataclass(frozen=True, slots=True)
class ToolCall:
    """Decoded Tool invocation; metadata remains Host-issued and opaque."""

    parameters: Any
    context: Mapping[str, Any] | None = None
    operation: Literal[
        "tool_execute", "tool_execute_stream", "tool_validate_parameters"
    ] = "tool_execute"
    extension: WireHandle | None = None
    invocation_id: str | None = None
    deadline: Mapping[str, Any] | None = None
    stream: WireHandle | None = None

    @classmethod
    def from_payload(cls, payload: Mapping[str, Any]) -> ToolCall:
        invocation = payload.get("invocation")
        if not isinstance(invocation, Mapping):
            raise TypeError("extension invocation is missing invocation")
        operation = str(invocation.get("operation", ""))
        if operation not in {
            "tool_execute",
            "tool_execute_stream",
            "tool_validate_parameters",
        }:
            raise ValueError(f"unsupported Tool operation: {operation}")
        raw_input = invocation.get("input")
        if not isinstance(raw_input, Mapping) or "parameters" not in raw_input:
            raise TypeError("Tool invocation input must contain parameters")
        input_value = raw_input
        raw_parameters = input_value.get("parameters")
        extension = _payload_handle(payload, "extension")
        stream = _payload_handle(payload, "stream")
        return cls(
            parameters=from_wire(raw_parameters),
            context=cast(Mapping[str, Any] | None, input_value.get("context")),
            operation=cast(
                Literal[
                    "tool_execute", "tool_execute_stream", "tool_validate_parameters"
                ],
                operation,
            ),
            extension=extension,
            invocation_id=_optional_text(payload.get("invocation_id")),
            deadline=cast(Mapping[str, Any] | None, payload.get("deadline")),
            stream=stream,
        )


@dataclass(frozen=True, slots=True)
class LlmChatCall:
    """Decoded LlmClient invocation with an additive request mapping."""

    request: Mapping[str, Any]
    operation: Literal["llm_chat", "llm_chat_stream"] = "llm_chat"
    extension: WireHandle | None = None
    invocation_id: str | None = None
    deadline: Mapping[str, Any] | None = None
    stream: WireHandle | None = None

    @property
    def messages(self) -> tuple[Any, ...]:
        messages = self.request.get("messages", ())
        return tuple(messages) if isinstance(messages, (list, tuple)) else ()

    @classmethod
    def from_payload(cls, payload: Mapping[str, Any]) -> LlmChatCall:
        invocation = payload.get("invocation")
        if not isinstance(invocation, Mapping):
            raise TypeError("extension invocation is missing invocation")
        operation = str(invocation.get("operation", ""))
        if operation not in {"llm_chat", "llm_chat_stream"}:
            raise ValueError(f"unsupported LlmClient operation: {operation}")
        raw_input = invocation.get("input")
        if not isinstance(raw_input, Mapping) or not isinstance(
            raw_input.get("messages"), list
        ):
            raise TypeError("LlmClient invocation input must contain messages")
        request = dict(raw_input)
        return cls(
            request=request,
            operation=cast(Literal["llm_chat", "llm_chat_stream"], operation),
            extension=_payload_handle(payload, "extension"),
            invocation_id=_optional_text(payload.get("invocation_id")),
            deadline=cast(Mapping[str, Any] | None, payload.get("deadline")),
            stream=_payload_handle(payload, "stream"),
        )


@dataclass(frozen=True, slots=True)
class StoreCall:
    """Decoded Store invocation preserving its operation-specific input."""

    operation: str
    request: Mapping[str, Any]
    extension: WireHandle | None = None
    invocation_id: str | None = None
    deadline: Mapping[str, Any] | None = None

    @property
    def namespace(self) -> tuple[str, ...]:
        value = self.request.get("namespace", ())
        return (
            tuple(str(item) for item in value)
            if isinstance(value, (list, tuple))
            else ()
        )

    @property
    def key(self) -> str | None:
        value = self.request.get("key")
        return value if isinstance(value, str) else None

    @property
    def value(self) -> Any:
        return from_wire(self.request.get("value"))

    @classmethod
    def from_payload(cls, payload: Mapping[str, Any]) -> StoreCall:
        invocation = payload.get("invocation")
        if not isinstance(invocation, Mapping):
            raise TypeError("extension invocation is missing invocation")
        operation = str(invocation.get("operation", ""))
        if operation not in {
            "store_put",
            "store_get",
            "store_search",
            "store_search_with",
            "store_delete",
            "store_list_namespaces",
            "store_list",
            "store_prune_expired",
            "store_dedup_by_content",
        }:
            raise ValueError(f"unsupported Store operation: {operation}")
        raw_input = invocation.get("input")
        if not isinstance(raw_input, Mapping):
            raise TypeError("Store invocation input must be an object")
        request = dict(raw_input)
        return cls(
            operation=operation,
            request=request,
            extension=_payload_handle(payload, "extension"),
            invocation_id=_optional_text(payload.get("invocation_id")),
            deadline=cast(Mapping[str, Any] | None, payload.get("deadline")),
        )


@dataclass(frozen=True, slots=True)
class CriticCall:
    """Decoded Host-issued Critic invocation."""

    task: str
    answer: str
    context: str
    operation: Literal["critic_critique"] = "critic_critique"
    extension: WireHandle | None = None
    invocation_id: str | None = None
    deadline: Mapping[str, Any] | None = None
    stream: WireHandle | None = None

    @classmethod
    def from_payload(cls, payload: Mapping[str, Any]) -> CriticCall:
        invocation = payload.get("invocation")
        if not isinstance(invocation, Mapping):
            raise TypeError("extension invocation is missing invocation")
        operation = str(invocation.get("operation", ""))
        if operation != "critic_critique":
            raise ValueError(f"unsupported Critic operation: {operation}")
        raw_input = invocation.get("input")
        if not isinstance(raw_input, Mapping):
            raise TypeError("Critic invocation input must be an object")
        values = {
            "task": raw_input.get("task"),
            "answer": raw_input.get("answer"),
            "context": raw_input.get("context"),
        }
        if not all(isinstance(value, str) for value in values.values()):
            raise TypeError(
                "Critique invocation input must contain task, answer, and context strings"
            )
        return cls(
            task=cast(str, values["task"]),
            answer=cast(str, values["answer"]),
            context=cast(str, values["context"]),
            extension=_payload_handle(payload, "extension"),
            invocation_id=_optional_text(payload.get("invocation_id")),
            deadline=cast(Mapping[str, Any] | None, payload.get("deadline")),
            stream=_payload_handle(payload, "stream"),
        )


@dataclass(frozen=True, slots=True)
class TokenizerReference:
    """Temporary owner-checked Host tokenizer supplied to one callback."""

    resource: WireHandle
    owner_session_id: str


class AgentComponentRequest(Protocol):
    operation: str


@dataclass(frozen=True, slots=True)
class ConversationCreateRequest:
    conversation: Mapping[str, Any]
    operation: Literal["conversation_create"] = "conversation_create"


@dataclass(frozen=True, slots=True)
class ConversationGetRequest:
    conversation_id: str
    operation: Literal["conversation_get"] = "conversation_get"


@dataclass(frozen=True, slots=True)
class ConversationListRequest:
    user_id: str | None
    agent_type: str | None
    limit: int | None
    offset: int | None
    operation: Literal["conversation_list"] = "conversation_list"


@dataclass(frozen=True, slots=True)
class ConversationUpdateRequest:
    conversation_id: str
    title: str | None
    summary: str | None
    compressed_before_id: int | None
    operation: Literal["conversation_update"] = "conversation_update"


@dataclass(frozen=True, slots=True)
class ConversationDeleteRequest:
    conversation_id: str
    operation: Literal["conversation_delete"] = "conversation_delete"


@dataclass(frozen=True, slots=True)
class ConversationSaveMessagesRequest:
    conversation_id: str
    messages: tuple[Mapping[str, Any], ...]
    operation: Literal["conversation_save_messages"] = "conversation_save_messages"


@dataclass(frozen=True, slots=True)
class ConversationGetMessagesRequest:
    conversation_id: str
    operation: Literal["conversation_get_messages"] = "conversation_get_messages"


@dataclass(frozen=True, slots=True)
class ConversationCountMessagesRequest:
    conversation_id: str
    operation: Literal["conversation_count_messages"] = "conversation_count_messages"


@dataclass(frozen=True, slots=True)
class ConversationEnsureRequest:
    conversation: Mapping[str, Any]
    operation: Literal["conversation_ensure"] = "conversation_ensure"


@dataclass(frozen=True, slots=True)
class ConversationSearchRequest:
    query: str
    limit: int
    operation: Literal["conversation_search"] = "conversation_search"


@dataclass(frozen=True, slots=True)
class RunSaveRequest:
    run: Mapping[str, Any]
    operation: Literal["run_save"] = "run_save"


@dataclass(frozen=True, slots=True)
class RunLoadRequest:
    run_id: str
    operation: Literal["run_load"] = "run_load"


@dataclass(frozen=True, slots=True)
class RunListBySessionRequest:
    session_id: str
    operation: Literal["run_list_by_session"] = "run_list_by_session"


@dataclass(frozen=True, slots=True)
class RunListAllRequest:
    limit: int
    operation: Literal["run_list_all"] = "run_list_all"


@dataclass(frozen=True, slots=True)
class RunAppendEventRequest:
    run_id: str
    event: Mapping[str, Any]
    operation: Literal["run_append_event"] = "run_append_event"


@dataclass(frozen=True, slots=True)
class RunListByParentRequest:
    parent_run_id: str
    operation: Literal["run_list_by_parent"] = "run_list_by_parent"


@dataclass(frozen=True, slots=True)
class RuntimeGetCheckpointRequest:
    conversation_id: str
    operation: Literal["runtime_get_checkpoint"] = "runtime_get_checkpoint"


@dataclass(frozen=True, slots=True)
class RuntimeSaveCheckpointRequest:
    checkpoint: Mapping[str, Any]
    operation: Literal["runtime_save_checkpoint"] = "runtime_save_checkpoint"


@dataclass(frozen=True, slots=True)
class RuntimeSaveCheckpointForScopeRequest:
    scope_id: str
    checkpoint: Mapping[str, Any]
    operation: Literal["runtime_save_checkpoint_for_scope"] = (
        "runtime_save_checkpoint_for_scope"
    )


@dataclass(frozen=True, slots=True)
class RuntimeStateIdsRequest:
    scope_id: str
    operation: Literal["runtime_state_ids"] = "runtime_state_ids"


@dataclass(frozen=True, slots=True)
class RuntimeClearStateRequest:
    scope_id: str
    runtime_state_id: str
    operation: Literal["runtime_clear_state"] = "runtime_clear_state"


@dataclass(frozen=True, slots=True)
class RuntimeClearScopeRequest:
    scope_id: str
    operation: Literal["runtime_clear_scope"] = "runtime_clear_scope"


@dataclass(frozen=True, slots=True)
class RuntimeClearConversationRequest:
    conversation_id: str
    operation: Literal["runtime_clear_conversation"] = "runtime_clear_conversation"


@dataclass(frozen=True, slots=True)
class AuditLogRequest:
    event: Mapping[str, Any]
    operation: Literal["audit_log"] = "audit_log"


@dataclass(frozen=True, slots=True)
class AuditQueryRequest:
    session_id: str | None
    agent_name: str | None
    from_timestamp: str | None
    to_timestamp: str | None
    limit: int | None
    operation: Literal["audit_query"] = "audit_query"


@dataclass(frozen=True, slots=True)
class ContextProjectRequest:
    iteration: int
    agent_name: str
    session_id: str | None
    conversation_id: str | None
    run_id: str | None
    turn_id: str | None
    operation: Literal["context_project"] = "context_project"


@dataclass(frozen=True, slots=True)
class MemoryTriggerRequest:
    trigger: Mapping[str, Any]
    operation: Literal["memory_trigger"] = "memory_trigger"


@dataclass(frozen=True, slots=True)
class GuardCheckRequest:
    content: str
    direction: Literal["input", "output", "tool_input", "tool_output"]
    operation: Literal["guard_check"] = "guard_check"


@dataclass(frozen=True, slots=True)
class SearchProviderSearchRequest:
    query: str
    max_results: int
    operation: Literal["search_provider_search"] = "search_provider_search"


@dataclass(frozen=True, slots=True)
class WorkflowCheckpointSaveRequest:
    checkpoint: Mapping[str, Any]
    operation: Literal["workflow_checkpoint_save"] = "workflow_checkpoint_save"


@dataclass(frozen=True, slots=True)
class WorkflowCheckpointSaveIfGenerationRequest:
    checkpoint: Mapping[str, Any]
    expected_generation: int
    operation: Literal["workflow_checkpoint_save_if_generation"] = (
        "workflow_checkpoint_save_if_generation"
    )


@dataclass(frozen=True, slots=True)
class WorkflowCheckpointClaimAttemptRequest:
    checkpoint_id: str
    attempt_id: str
    operation: Literal[
        "workflow_checkpoint_ack_claim",
        "workflow_checkpoint_requeue_claim",
        "workflow_checkpoint_renew_claim",
    ] = "workflow_checkpoint_ack_claim"

    def __post_init__(self) -> None:
        if self.operation not in {
            "workflow_checkpoint_ack_claim",
            "workflow_checkpoint_requeue_claim",
            "workflow_checkpoint_renew_claim",
        }:
            raise ValueError("invalid workflow checkpoint claim-attempt operation")


@dataclass(frozen=True, slots=True)
class WorkflowCheckpointIdRequest:
    checkpoint_id: str
    operation: Literal[
        "workflow_checkpoint_load",
        "workflow_checkpoint_claim",
        "workflow_checkpoint_delete",
    ] = "workflow_checkpoint_load"

    def __post_init__(self) -> None:
        if self.operation not in {
            "workflow_checkpoint_load",
            "workflow_checkpoint_claim",
            "workflow_checkpoint_delete",
        }:
            raise ValueError("invalid workflow checkpoint id operation")


@dataclass(frozen=True, slots=True)
class WorkflowCheckpointListRequest:
    operation: Literal["workflow_checkpoint_list"] = "workflow_checkpoint_list"


@dataclass(frozen=True, slots=True)
class WorkflowCheckpointListByGraphRequest:
    graph_name: str
    operation: Literal["workflow_checkpoint_list_by_graph"] = (
        "workflow_checkpoint_list_by_graph"
    )


@dataclass(frozen=True, slots=True)
class WorkflowCheckpointListFilteredRequest:
    filter: Mapping[str, Any]
    operation: Literal["workflow_checkpoint_list_filtered"] = (
        "workflow_checkpoint_list_filtered"
    )


@dataclass(frozen=True, slots=True)
class WorkflowCheckpointClearRequest:
    operation: Literal["workflow_checkpoint_clear"] = "workflow_checkpoint_clear"


@dataclass(frozen=True, slots=True)
class RevisionedTaskLoadRequest:
    scope_id: str
    operation: Literal["revisioned_task_load"] = "revisioned_task_load"


@dataclass(frozen=True, slots=True)
class RevisionedTaskCompareAndCommitRequest:
    scope_id: str
    commit: Mapping[str, Any]
    operation: Literal["revisioned_task_compare_and_commit"] = (
        "revisioned_task_compare_and_commit"
    )


@dataclass(frozen=True, slots=True)
class SandboxEmptyRequest:
    operation: Literal["sandbox_is_available", "sandbox_cleanup"] = (
        "sandbox_is_available"
    )

    def __post_init__(self) -> None:
        if self.operation not in {"sandbox_is_available", "sandbox_cleanup"}:
            raise ValueError("invalid empty sandbox operation")


@dataclass(frozen=True, slots=True)
class SandboxExecuteRequest:
    command: Mapping[str, Any]
    limits: Mapping[str, Any] | None = None
    operation: Literal[
        "sandbox_execute",
        "sandbox_execute_stream",
        "sandbox_execute_with_limits",
        "sandbox_execute_with_limits_and_cancel",
    ] = "sandbox_execute"

    def __post_init__(self) -> None:
        if self.operation in {"sandbox_execute", "sandbox_execute_stream"}:
            if self.limits is not None:
                raise ValueError(f"{self.operation} does not accept limits")
        elif self.operation in {
            "sandbox_execute_with_limits",
            "sandbox_execute_with_limits_and_cancel",
        }:
            if self.limits is None:
                raise ValueError(f"{self.operation} requires limits")
        else:
            raise ValueError("invalid sandbox execute operation")


@dataclass(frozen=True, slots=True)
class McpTransportEmptyRequest:
    operation: Literal["mcp_transport_close", "mcp_transport_try_notification"] = (
        "mcp_transport_close"
    )

    def __post_init__(self) -> None:
        if self.operation not in {
            "mcp_transport_close",
            "mcp_transport_try_notification",
        }:
            raise ValueError("invalid empty MCP transport operation")


@dataclass(frozen=True, slots=True)
class McpTransportPayloadRequest:
    payload: Mapping[str, Any]
    operation: Literal["mcp_transport_send", "mcp_transport_notify"] = (
        "mcp_transport_send"
    )

    def __post_init__(self) -> None:
        if self.operation not in {"mcp_transport_send", "mcp_transport_notify"}:
            raise ValueError("invalid MCP transport payload operation")


@dataclass(frozen=True, slots=True)
class EmbedderEmbedRequest:
    text: str
    operation: Literal["embedder_embed"] = "embedder_embed"


@dataclass(frozen=True, slots=True)
class MemoryPromoterPromoteRequest:
    evicted: tuple[Mapping[str, Any], ...]
    operation: Literal["memory_promoter_promote"] = "memory_promoter_promote"


@dataclass(frozen=True, slots=True)
class WorkflowRunRequest:
    input: str
    operation: Literal["workflow_run", "workflow_run_stream"] = "workflow_run"

    def __post_init__(self) -> None:
        if self.operation not in {"workflow_run", "workflow_run_stream"}:
            raise ValueError("invalid workflow run operation")


@dataclass(frozen=True, slots=True)
class IntentClassifyRequest:
    user_input: str
    context: tuple[Mapping[str, Any], ...]
    operation: Literal["intent_classify"] = "intent_classify"


@dataclass(frozen=True, slots=True)
class SkillLoadAllowsRequest:
    descriptor: Mapping[str, Any]
    operation: Literal["skill_load_allows"] = "skill_load_allows"


def _component_text(arguments: Mapping[str, Any], field: str) -> str:
    value = arguments.get(field)
    if not isinstance(value, str):
        raise TypeError(f"{field} must be text")
    return value


def _component_optional_text(arguments: Mapping[str, Any], field: str) -> str | None:
    value = arguments.get(field)
    if value is not None and not isinstance(value, str):
        raise TypeError(f"{field} must be text or None")
    return cast(str | None, value)


def _component_wire(arguments: Mapping[str, Any], field: str) -> Mapping[str, Any]:
    value = arguments.get(field)
    if not isinstance(value, Mapping) or not isinstance(value.get("kind"), str):
        raise TypeError(f"{field} must be a WireValue")
    return cast(Mapping[str, Any], value)


def _component_request(
    operation: str, arguments: Mapping[str, Any]
) -> AgentComponentRequest:
    allowed = _AGENT_COMPONENT_INPUT_FIELDS.get(operation)
    if allowed is None:
        raise ValueError(f"unsupported Agent component operation: {operation}")
    unexpected = set(arguments) - allowed
    if unexpected:
        raise ValueError(
            f"Agent component {operation} has unknown input fields: {sorted(unexpected)}"
        )
    text = lambda field: _component_text(arguments, field)
    optional_text = lambda field: _component_optional_text(arguments, field)
    wire = lambda field: _component_wire(arguments, field)
    if operation == "conversation_create":
        return ConversationCreateRequest(wire("conversation"))
    if operation == "conversation_get":
        return ConversationGetRequest(text("conversation_id"))
    if operation == "conversation_list":
        limit = arguments.get("limit")
        offset = arguments.get("offset")
        return ConversationListRequest(
            optional_text("user_id"),
            optional_text("agent_type"),
            int(_canonical_u64_text(limit, "limit")) if limit is not None else None,
            int(_canonical_u64_text(offset, "offset")) if offset is not None else None,
        )
    if operation == "conversation_update":
        compressed = arguments.get("compressed_before_id")
        if compressed is not None and (
            not isinstance(compressed, str) or str(int(compressed)) != compressed
        ):
            raise ValueError("compressed_before_id must be canonical integer text")
        return ConversationUpdateRequest(
            text("conversation_id"),
            optional_text("title"),
            optional_text("summary"),
            int(compressed) if compressed is not None else None,
        )
    if operation == "conversation_delete":
        return ConversationDeleteRequest(text("conversation_id"))
    if operation == "conversation_save_messages":
        messages = arguments.get("messages")
        if not isinstance(messages, list):
            raise TypeError("messages must be a list of WireValue objects")
        return ConversationSaveMessagesRequest(
            text("conversation_id"),
            tuple(_component_wire({"item": item}, "item") for item in messages),
        )
    if operation == "conversation_get_messages":
        return ConversationGetMessagesRequest(text("conversation_id"))
    if operation == "conversation_count_messages":
        return ConversationCountMessagesRequest(text("conversation_id"))
    if operation == "conversation_ensure":
        return ConversationEnsureRequest(wire("conversation"))
    if operation == "conversation_search":
        return ConversationSearchRequest(
            text("query"), int(_canonical_u64_text(arguments.get("limit"), "limit"))
        )
    if operation == "run_save":
        return RunSaveRequest(wire("run"))
    if operation == "run_load":
        return RunLoadRequest(text("run_id"))
    if operation == "run_list_by_session":
        return RunListBySessionRequest(text("session_id"))
    if operation == "run_list_all":
        return RunListAllRequest(
            int(_canonical_u64_text(arguments.get("limit"), "limit"))
        )
    if operation == "run_append_event":
        return RunAppendEventRequest(text("run_id"), wire("event"))
    if operation == "run_list_by_parent":
        return RunListByParentRequest(text("parent_run_id"))
    if operation == "runtime_get_checkpoint":
        return RuntimeGetCheckpointRequest(text("conversation_id"))
    if operation == "runtime_save_checkpoint":
        return RuntimeSaveCheckpointRequest(wire("checkpoint"))
    if operation == "runtime_save_checkpoint_for_scope":
        return RuntimeSaveCheckpointForScopeRequest(
            text("scope_id"), wire("checkpoint")
        )
    if operation == "runtime_state_ids":
        return RuntimeStateIdsRequest(text("scope_id"))
    if operation == "runtime_clear_state":
        return RuntimeClearStateRequest(text("scope_id"), text("runtime_state_id"))
    if operation == "runtime_clear_scope":
        return RuntimeClearScopeRequest(text("scope_id"))
    if operation == "runtime_clear_conversation":
        return RuntimeClearConversationRequest(text("conversation_id"))
    if operation == "audit_log":
        return AuditLogRequest(wire("event"))
    if operation == "audit_query":
        limit = arguments.get("limit")
        return AuditQueryRequest(
            optional_text("session_id"),
            optional_text("agent_name"),
            optional_text("from"),
            optional_text("to"),
            int(_canonical_u64_text(limit, "limit")) if limit is not None else None,
        )
    if operation == "context_project":
        return ContextProjectRequest(
            int(_canonical_u64_text(arguments.get("iteration"), "iteration")),
            text("agent_name"),
            optional_text("session_id"),
            optional_text("conversation_id"),
            optional_text("run_id"),
            optional_text("turn_id"),
        )
    if operation == "memory_trigger":
        return MemoryTriggerRequest(wire("trigger"))
    if operation == "guard_check":
        direction = text("direction")
        if direction not in {"input", "output", "tool_input", "tool_output"}:
            raise ValueError("guard direction is invalid")
        return GuardCheckRequest(
            text("content"),
            cast(Literal["input", "output", "tool_input", "tool_output"], direction),
        )
    if operation == "search_provider_search":
        return SearchProviderSearchRequest(
            text("query"),
            int(_canonical_u64_text(arguments.get("max_results"), "max_results")),
        )
    if operation == "workflow_checkpoint_save":
        return WorkflowCheckpointSaveRequest(wire("checkpoint"))
    if operation == "workflow_checkpoint_save_if_generation":
        return WorkflowCheckpointSaveIfGenerationRequest(
            wire("checkpoint"),
            int(
                _canonical_u64_text(
                    arguments.get("expected_generation"), "expected_generation"
                )
            ),
        )
    if operation in {
        "workflow_checkpoint_load",
        "workflow_checkpoint_claim",
        "workflow_checkpoint_delete",
    }:
        return WorkflowCheckpointIdRequest(
            text("checkpoint_id"),
            cast(
                Literal[
                    "workflow_checkpoint_load",
                    "workflow_checkpoint_claim",
                    "workflow_checkpoint_delete",
                ],
                operation,
            ),
        )
    if operation in {
        "workflow_checkpoint_ack_claim",
        "workflow_checkpoint_requeue_claim",
        "workflow_checkpoint_renew_claim",
    }:
        return WorkflowCheckpointClaimAttemptRequest(
            text("checkpoint_id"),
            text("attempt_id"),
            cast(
                Literal[
                    "workflow_checkpoint_ack_claim",
                    "workflow_checkpoint_requeue_claim",
                    "workflow_checkpoint_renew_claim",
                ],
                operation,
            ),
        )
    if operation == "workflow_checkpoint_list":
        if arguments:
            raise ValueError("workflow_checkpoint_list input must be empty")
        return WorkflowCheckpointListRequest()
    if operation == "workflow_checkpoint_list_by_graph":
        return WorkflowCheckpointListByGraphRequest(text("graph_name"))
    if operation == "workflow_checkpoint_list_filtered":
        return WorkflowCheckpointListFilteredRequest(wire("filter"))
    if operation == "workflow_checkpoint_clear":
        if arguments:
            raise ValueError("workflow_checkpoint_clear input must be empty")
        return WorkflowCheckpointClearRequest()
    if operation == "revisioned_task_load":
        return RevisionedTaskLoadRequest(text("scope_id"))
    if operation == "revisioned_task_compare_and_commit":
        return RevisionedTaskCompareAndCommitRequest(text("scope_id"), wire("commit"))
    if operation in {"sandbox_is_available", "sandbox_cleanup"}:
        if arguments:
            raise ValueError(f"{operation} input must be empty")
        return SandboxEmptyRequest(
            cast(Literal["sandbox_is_available", "sandbox_cleanup"], operation)
        )
    if operation in {"sandbox_execute", "sandbox_execute_stream"}:
        return SandboxExecuteRequest(
            wire("command"),
            operation=cast(
                Literal["sandbox_execute", "sandbox_execute_stream"], operation
            ),
        )
    if operation in {
        "sandbox_execute_with_limits",
        "sandbox_execute_with_limits_and_cancel",
    }:
        return SandboxExecuteRequest(
            wire("command"),
            wire("limits"),
            cast(
                Literal[
                    "sandbox_execute_with_limits",
                    "sandbox_execute_with_limits_and_cancel",
                ],
                operation,
            ),
        )
    if operation in {"mcp_transport_close", "mcp_transport_try_notification"}:
        if arguments:
            raise ValueError(f"{operation} input must be empty")
        return McpTransportEmptyRequest(
            cast(
                Literal["mcp_transport_close", "mcp_transport_try_notification"],
                operation,
            )
        )
    if operation in {"mcp_transport_send", "mcp_transport_notify"}:
        field = "request" if operation == "mcp_transport_send" else "notification"
        return McpTransportPayloadRequest(
            wire(field),
            cast(Literal["mcp_transport_send", "mcp_transport_notify"], operation),
        )
    if operation == "embedder_embed":
        return EmbedderEmbedRequest(text("text"))
    if operation == "memory_promoter_promote":
        evicted = arguments.get("evicted")
        if not isinstance(evicted, list) or not all(
            isinstance(message, Mapping) for message in evicted
        ):
            raise TypeError("evicted must be a list of message objects")
        return MemoryPromoterPromoteRequest(
            tuple(cast(Mapping[str, Any], message) for message in evicted)
        )
    if operation in {"workflow_run", "workflow_run_stream"}:
        return WorkflowRunRequest(
            text("input"),
            cast(Literal["workflow_run", "workflow_run_stream"], operation),
        )
    if operation == "intent_classify":
        context = arguments.get("context")
        if not isinstance(context, list) or not all(
            isinstance(message, Mapping) for message in context
        ):
            raise TypeError("intent context must be a list of message objects")
        return IntentClassifyRequest(
            text("user_input"),
            tuple(cast(Mapping[str, Any], message) for message in context),
        )
    if operation == "skill_load_allows":
        descriptor = arguments.get("descriptor")
        if not isinstance(descriptor, Mapping):
            raise TypeError("skill descriptor must be an object")
        required = {
            "name",
            "description",
            "location",
            "license",
            "compatibility",
            "metadata",
            "source",
            "allowed_tools",
            "shell",
            "paths",
            "triggers",
            "hooks",
            "sandbox",
            "depends_on",
        }
        if set(descriptor) != required:
            raise ValueError("skill descriptor fields do not match the typed contract")
        return SkillLoadAllowsRequest(dict(descriptor))
    raise ValueError(f"unsupported Agent component operation: {operation}")


@dataclass(frozen=True, slots=True)
class CompressionCall:
    """Decoded Host-issued ContextCompressor invocation."""

    messages: tuple[Any, ...]
    token_limit: int
    current_query: str | None
    focus_instructions: str | None
    tokenizer: TokenizerReference
    operation: Literal["compressor_compress"] = "compressor_compress"
    extension: WireHandle | None = None
    invocation_id: str | None = None
    deadline: Mapping[str, Any] | None = None

    @classmethod
    def from_payload(cls, payload: Mapping[str, Any]) -> CompressionCall:
        invocation = payload.get("invocation")
        if not isinstance(invocation, Mapping):
            raise TypeError("extension invocation is missing invocation")
        operation = str(invocation.get("operation", ""))
        if operation != "compressor_compress":
            raise ValueError(f"unsupported ContextCompressor operation: {operation}")
        raw_input = invocation.get("input")
        if not isinstance(raw_input, Mapping):
            raise TypeError("ContextCompressor invocation input must be an object")
        messages = raw_input.get("messages")
        if not isinstance(messages, list) or not all(
            isinstance(message, Mapping) for message in messages
        ):
            raise TypeError("ContextCompressor messages must be objects")
        token_limit = int(
            _canonical_u64_text(raw_input.get("token_limit"), "token_limit")
        )
        current_query = raw_input.get("current_query")
        focus_instructions = raw_input.get("focus_instructions")
        if current_query is not None and not isinstance(current_query, str):
            raise TypeError("current_query must be text or None")
        if focus_instructions is not None and not isinstance(focus_instructions, str):
            raise TypeError("focus_instructions must be text or None")
        tokenizer = raw_input.get("tokenizer")
        if not isinstance(tokenizer, Mapping) or not isinstance(
            tokenizer.get("resource"), Mapping
        ):
            raise TypeError("ContextCompressor tokenizer resource is malformed")
        tokenizer_resource = WireHandle.from_dict(
            cast(Mapping[str, Any], tokenizer["resource"])
        )
        if tokenizer_resource.kind != "facade_resource":
            raise ValueError("ContextCompressor tokenizer must be a facade resource")
        owner_session_id = tokenizer.get("owner_session_id")
        if not isinstance(owner_session_id, str) or not owner_session_id:
            raise ValueError("ContextCompressor tokenizer owner_session_id is required")
        return cls(
            messages=tuple(dict(message) for message in messages),
            token_limit=token_limit,
            current_query=current_query,
            focus_instructions=focus_instructions,
            tokenizer=TokenizerReference(tokenizer_resource, owner_session_id),
            extension=_payload_handle(payload, "extension"),
            invocation_id=_optional_text(payload.get("invocation_id")),
            deadline=cast(Mapping[str, Any] | None, payload.get("deadline")),
        )


@dataclass(frozen=True, slots=True)
class AgentComponentCall:
    """Decoded operation-discriminated Agent component invocation."""

    component: str
    request: AgentComponentRequest
    operation: Literal["agent_component_call", "agent_component_call_stream"] = (
        "agent_component_call"
    )
    extension: WireHandle | None = None
    invocation_id: str | None = None
    deadline: Mapping[str, Any] | None = None

    @classmethod
    def from_payload(cls, payload: Mapping[str, Any]) -> AgentComponentCall:
        invocation = payload.get("invocation")
        if not isinstance(invocation, Mapping) or invocation.get("operation") not in {
            "agent_component_call",
            "agent_component_call_stream",
        }:
            raise ValueError("unsupported Agent component operation")
        raw_input = invocation.get("input")
        call = raw_input.get("call") if isinstance(raw_input, Mapping) else None
        if (
            not isinstance(raw_input, Mapping)
            or not isinstance(raw_input.get("component"), str)
            or not isinstance(call, Mapping)
            or not isinstance(call.get("operation"), str)
        ):
            raise TypeError("Agent component invocation input is malformed")
        component_operation = cast(str, call["operation"])
        component_input = call.get("input")
        if component_input is None and not _AGENT_COMPONENT_INPUT_FIELDS.get(
            component_operation
        ):
            component_input = {}
        if not isinstance(component_input, Mapping):
            raise TypeError("Agent component invocation input is malformed")
        component = cast(str, raw_input["component"])
        expected_component = _component_for_operation(component_operation)
        if component != expected_component:
            raise ValueError("Agent component kind does not match its operation")
        request = _component_request(
            component_operation, cast(Mapping[str, Any], component_input)
        )
        return cls(
            component=component,
            request=request,
            operation=cast(
                Literal["agent_component_call", "agent_component_call_stream"],
                invocation["operation"],
            ),
            extension=_payload_handle(payload, "extension"),
            invocation_id=_optional_text(payload.get("invocation_id")),
            deadline=cast(Mapping[str, Any] | None, payload.get("deadline")),
        )

    @property
    def component_operation(self) -> str:
        return self.request.operation


class ExtensionOutcome(Protocol):
    """Typed callback result converted to the Host's outcome union."""

    def to_wire(self) -> dict[str, Any]: ...


@dataclass(frozen=True, slots=True)
class ExtensionResultOutcome:
    operation: str
    value: Any

    def to_wire(self) -> dict[str, Any]:
        return {
            "outcome": "result",
            "result": {"operation": self.operation, "value": self.value},
        }


@dataclass(frozen=True, slots=True)
class CritiqueOutcome:
    """Typed result for a Host-issued Critic invocation."""

    score: float
    passed: bool
    feedback: str
    suggestions: tuple[str, ...] = ()

    def to_wire(self) -> dict[str, Any]:
        return ExtensionResultOutcome(
            "critic_critique",
            {
                "score": self.score,
                "passed": self.passed,
                "feedback": self.feedback,
                "suggestions": list(self.suggestions),
            },
        ).to_wire()


@dataclass(frozen=True, slots=True)
class CompressionOutcome:
    """Typed result for a Host-issued ContextCompressor invocation."""

    messages: tuple[Mapping[str, Any], ...]
    evicted: tuple[Mapping[str, Any], ...] = ()
    checkpoint: Any = None

    def to_wire(self) -> dict[str, Any]:
        return ExtensionResultOutcome(
            "compressor_compress",
            {
                "messages": [dict(message) for message in self.messages],
                "evicted": [dict(message) for message in self.evicted],
                "checkpoint": self.checkpoint,
            },
        ).to_wire()


class AgentComponentResult(Protocol):
    component: str
    operation: str
    value: Mapping[str, Any] | None


@dataclass(frozen=True, slots=True)
class _TypedAgentComponentResult:
    component: str
    operation: str
    value: Mapping[str, Any] | None = None


class ConversationStoreResult:
    @staticmethod
    def created(conversation: Mapping[str, Any]) -> AgentComponentResult:
        return _TypedAgentComponentResult(
            "conversation_store", "conversation_create", {"conversation": conversation}
        )

    @staticmethod
    def found(conversation: Mapping[str, Any] | None) -> AgentComponentResult:
        return _TypedAgentComponentResult(
            "conversation_store", "conversation_get", {"conversation": conversation}
        )

    @staticmethod
    def listed(conversations: Iterable[Mapping[str, Any]]) -> AgentComponentResult:
        return _TypedAgentComponentResult(
            "conversation_store",
            "conversation_list",
            {"conversations": list(conversations)},
        )

    @staticmethod
    def updated() -> AgentComponentResult:
        return _TypedAgentComponentResult("conversation_store", "conversation_update")

    @staticmethod
    def deleted() -> AgentComponentResult:
        return _TypedAgentComponentResult("conversation_store", "conversation_delete")

    @staticmethod
    def messages_saved() -> AgentComponentResult:
        return _TypedAgentComponentResult(
            "conversation_store", "conversation_save_messages"
        )

    @staticmethod
    def messages_loaded(messages: Iterable[Mapping[str, Any]]) -> AgentComponentResult:
        return _TypedAgentComponentResult(
            "conversation_store",
            "conversation_get_messages",
            {"messages": list(messages)},
        )

    @staticmethod
    def messages_counted(count: int) -> AgentComponentResult:
        return _TypedAgentComponentResult(
            "conversation_store",
            "conversation_count_messages",
            {"count": _canonical_u64_text(count, "count")},
        )

    @staticmethod
    def ensured(conversation: Mapping[str, Any]) -> AgentComponentResult:
        return _TypedAgentComponentResult(
            "conversation_store",
            "conversation_ensure",
            {"conversation": conversation},
        )

    @staticmethod
    def searched(
        conversations: Iterable[Mapping[str, Any]],
    ) -> AgentComponentResult:
        return _TypedAgentComponentResult(
            "conversation_store",
            "conversation_search",
            {"conversations": list(conversations)},
        )


class RunStoreResult:
    @staticmethod
    def saved() -> AgentComponentResult:
        return _TypedAgentComponentResult("run_store", "run_save")

    @staticmethod
    def loaded(run: Mapping[str, Any] | None) -> AgentComponentResult:
        return _TypedAgentComponentResult("run_store", "run_load", {"run": run})

    @staticmethod
    def session_runs(runs: Iterable[Mapping[str, Any]]) -> AgentComponentResult:
        return _TypedAgentComponentResult(
            "run_store", "run_list_by_session", {"runs": list(runs)}
        )

    @staticmethod
    def all_runs(runs: Iterable[Mapping[str, Any]]) -> AgentComponentResult:
        return _TypedAgentComponentResult(
            "run_store", "run_list_all", {"runs": list(runs)}
        )

    @staticmethod
    def event_appended() -> AgentComponentResult:
        return _TypedAgentComponentResult("run_store", "run_append_event")

    @staticmethod
    def parent_runs(runs: Iterable[Mapping[str, Any]]) -> AgentComponentResult:
        return _TypedAgentComponentResult(
            "run_store", "run_list_by_parent", {"runs": list(runs)}
        )


class RuntimeStateStoreResult:
    @staticmethod
    def checkpoint_loaded(checkpoint: Mapping[str, Any] | None) -> AgentComponentResult:
        return _TypedAgentComponentResult(
            "runtime_state_store", "runtime_get_checkpoint", {"checkpoint": checkpoint}
        )

    @staticmethod
    def checkpoint_saved() -> AgentComponentResult:
        return _TypedAgentComponentResult(
            "runtime_state_store", "runtime_save_checkpoint"
        )

    @staticmethod
    def scope_checkpoint_saved() -> AgentComponentResult:
        return _TypedAgentComponentResult(
            "runtime_state_store", "runtime_save_checkpoint_for_scope"
        )

    @staticmethod
    def state_ids(state_ids: Iterable[str]) -> AgentComponentResult:
        return _TypedAgentComponentResult(
            "runtime_state_store", "runtime_state_ids", {"state_ids": list(state_ids)}
        )

    @staticmethod
    def state_cleared(receipt: Mapping[str, Any]) -> AgentComponentResult:
        return _TypedAgentComponentResult(
            "runtime_state_store", "runtime_clear_state", {"receipt": receipt}
        )

    @staticmethod
    def scope_cleared(receipt: Mapping[str, Any]) -> AgentComponentResult:
        return _TypedAgentComponentResult(
            "runtime_state_store", "runtime_clear_scope", {"receipt": receipt}
        )

    @staticmethod
    def conversation_cleared() -> AgentComponentResult:
        return _TypedAgentComponentResult(
            "runtime_state_store", "runtime_clear_conversation"
        )


class AuditLoggerResult:
    @staticmethod
    def logged() -> AgentComponentResult:
        return _TypedAgentComponentResult("audit_logger", "audit_log")

    @staticmethod
    def queried(events: Iterable[Mapping[str, Any]]) -> AgentComponentResult:
        return _TypedAgentComponentResult(
            "audit_logger", "audit_query", {"events": list(events)}
        )


class ContextProjectorResult:
    @staticmethod
    def projected(projections: Iterable[Mapping[str, Any]]) -> AgentComponentResult:
        return _TypedAgentComponentResult(
            "context_projector", "context_project", {"projections": list(projections)}
        )


class MemoryTriggerResult:
    @staticmethod
    def persisted() -> AgentComponentResult:
        return _TypedAgentComponentResult(
            "memory_trigger_sink", "memory_trigger", {"disposition": "persist"}
        )

    @staticmethod
    def captured() -> AgentComponentResult:
        return _TypedAgentComponentResult(
            "memory_trigger_sink", "memory_trigger", {"disposition": "captured"}
        )


class GuardResult:
    @staticmethod
    def checked(result: Mapping[str, Any]) -> AgentComponentResult:
        return _TypedAgentComponentResult("guard", "guard_check", {"result": result})


class SearchProviderResult:
    @staticmethod
    def searched(results: Iterable[Mapping[str, Any]]) -> AgentComponentResult:
        return _TypedAgentComponentResult(
            "search_provider", "search_provider_search", {"results": list(results)}
        )


class WorkflowCheckpointResult:
    @staticmethod
    def saved() -> AgentComponentResult:
        return _TypedAgentComponentResult(
            "workflow_checkpoint_store", "workflow_checkpoint_save"
        )

    @staticmethod
    def saved_if_generation(committed: bool) -> AgentComponentResult:
        if not isinstance(committed, bool):
            raise TypeError("committed must be a bool")
        return _TypedAgentComponentResult(
            "workflow_checkpoint_store",
            "workflow_checkpoint_save_if_generation",
            {"committed": committed},
        )

    @staticmethod
    def claim_acked() -> AgentComponentResult:
        return _TypedAgentComponentResult(
            "workflow_checkpoint_store", "workflow_checkpoint_ack_claim"
        )

    @staticmethod
    def claim_requeued() -> AgentComponentResult:
        return _TypedAgentComponentResult(
            "workflow_checkpoint_store", "workflow_checkpoint_requeue_claim"
        )

    @staticmethod
    def claim_renewed() -> AgentComponentResult:
        return _TypedAgentComponentResult(
            "workflow_checkpoint_store", "workflow_checkpoint_renew_claim"
        )

    @staticmethod
    def loaded(checkpoint: Mapping[str, Any] | None) -> AgentComponentResult:
        return _TypedAgentComponentResult(
            "workflow_checkpoint_store",
            "workflow_checkpoint_load",
            {"checkpoint": checkpoint},
        )

    @staticmethod
    def claimed(checkpoint: Mapping[str, Any] | None) -> AgentComponentResult:
        return _TypedAgentComponentResult(
            "workflow_checkpoint_store",
            "workflow_checkpoint_claim",
            {"checkpoint": checkpoint},
        )

    @staticmethod
    def listed(
        operation: Literal[
            "workflow_checkpoint_list",
            "workflow_checkpoint_list_by_graph",
            "workflow_checkpoint_list_filtered",
        ],
        checkpoints: Iterable[Mapping[str, Any]],
    ) -> AgentComponentResult:
        if operation not in {
            "workflow_checkpoint_list",
            "workflow_checkpoint_list_by_graph",
            "workflow_checkpoint_list_filtered",
        }:
            raise ValueError("invalid workflow checkpoint list operation")
        return _TypedAgentComponentResult(
            "workflow_checkpoint_store",
            operation,
            {"checkpoints": list(checkpoints)},
        )

    @staticmethod
    def deleted() -> AgentComponentResult:
        return _TypedAgentComponentResult(
            "workflow_checkpoint_store", "workflow_checkpoint_delete"
        )

    @staticmethod
    def cleared() -> AgentComponentResult:
        return _TypedAgentComponentResult(
            "workflow_checkpoint_store", "workflow_checkpoint_clear"
        )


class RevisionedTaskStoreResult:
    @staticmethod
    def loaded(graph: Mapping[str, Any] | None) -> AgentComponentResult:
        return _TypedAgentComponentResult(
            "revisioned_task_store", "revisioned_task_load", {"graph": graph}
        )

    @staticmethod
    def committed(graph: Mapping[str, Any]) -> AgentComponentResult:
        return _TypedAgentComponentResult(
            "revisioned_task_store",
            "revisioned_task_compare_and_commit",
            {"graph": graph},
        )


class SandboxResult:
    @staticmethod
    def availability(available: bool) -> AgentComponentResult:
        return _TypedAgentComponentResult(
            "sandbox_executor", "sandbox_is_available", {"available": available}
        )

    @staticmethod
    def executed(
        operation: Literal[
            "sandbox_execute",
            "sandbox_execute_with_limits",
            "sandbox_execute_with_limits_and_cancel",
        ],
        result: Mapping[str, Any],
    ) -> AgentComponentResult:
        if operation not in {
            "sandbox_execute",
            "sandbox_execute_with_limits",
            "sandbox_execute_with_limits_and_cancel",
        }:
            raise ValueError("invalid sandbox result operation")
        return _TypedAgentComponentResult(
            "sandbox_executor", operation, {"result": result}
        )

    @staticmethod
    def cleaned() -> AgentComponentResult:
        return _TypedAgentComponentResult("sandbox_executor", "sandbox_cleanup")


class McpTransportResult:
    @staticmethod
    def sent(response: Mapping[str, Any]) -> AgentComponentResult:
        return _TypedAgentComponentResult(
            "mcp_transport", "mcp_transport_send", {"response": response}
        )

    @staticmethod
    def notified() -> AgentComponentResult:
        return _TypedAgentComponentResult("mcp_transport", "mcp_transport_notify")

    @staticmethod
    def closed() -> AgentComponentResult:
        return _TypedAgentComponentResult("mcp_transport", "mcp_transport_close")

    @staticmethod
    def notification(value: Mapping[str, Any] | None) -> AgentComponentResult:
        return _TypedAgentComponentResult(
            "mcp_transport",
            "mcp_transport_try_notification",
            {"notification": value},
        )


class EmbedderResult:
    @staticmethod
    def embedded(vector: Iterable[float]) -> AgentComponentResult:
        values = [float(value) for value in vector]
        if not all(math.isfinite(value) for value in values):
            raise ValueError("embedding vector values must be finite")
        return _TypedAgentComponentResult(
            "embedder", "embedder_embed", {"vector": values}
        )


class MemoryPromoterResult:
    @staticmethod
    def promoted(
        submitted: int, promoted: int, deduplicated: int
    ) -> AgentComponentResult:
        return _TypedAgentComponentResult(
            "memory_promoter",
            "memory_promoter_promote",
            {
                "submitted": _canonical_u64_text(submitted, "submitted"),
                "promoted": _canonical_u64_text(promoted, "promoted"),
                "deduplicated": _canonical_u64_text(deduplicated, "deduplicated"),
            },
        )


class WorkflowResult:
    @staticmethod
    def completed(output: Mapping[str, Any]) -> AgentComponentResult:
        return _TypedAgentComponentResult(
            "workflow", "workflow_run", {"output": output}
        )


class IntentClassifierResult:
    @staticmethod
    def classified(intent: Mapping[str, Any]) -> AgentComponentResult:
        return _TypedAgentComponentResult(
            "intent_classifier", "intent_classify", {"intent": intent}
        )


class SkillLoadPolicyResult:
    @staticmethod
    def allowed(allowed: bool) -> AgentComponentResult:
        if not isinstance(allowed, bool):
            raise TypeError("allowed must be a bool")
        return _TypedAgentComponentResult(
            "skill_load_policy", "skill_load_allows", {"allowed": allowed}
        )


def _validate_component_result(operation: str, value: Mapping[str, Any] | None) -> None:
    unit = {
        "conversation_update",
        "conversation_delete",
        "conversation_save_messages",
        "run_save",
        "run_append_event",
        "runtime_save_checkpoint",
        "runtime_save_checkpoint_for_scope",
        "runtime_clear_conversation",
        "audit_log",
        "workflow_checkpoint_save",
        "workflow_checkpoint_ack_claim",
        "workflow_checkpoint_requeue_claim",
        "workflow_checkpoint_renew_claim",
        "workflow_checkpoint_delete",
        "workflow_checkpoint_clear",
        "sandbox_cleanup",
        "mcp_transport_notify",
        "mcp_transport_close",
    }
    if operation in unit:
        if value is not None:
            raise ValueError(f"{operation} must return a unit result")
        return
    if value is None:
        raise ValueError(f"{operation} must return a typed value")

    fields_by_operation = {
        "conversation_create": {"conversation"},
        "conversation_get": {"conversation"},
        "conversation_list": {"conversations"},
        "conversation_get_messages": {"messages"},
        "conversation_count_messages": {"count"},
        "conversation_ensure": {"conversation"},
        "conversation_search": {"conversations"},
        "run_load": {"run"},
        "run_list_by_session": {"runs"},
        "run_list_all": {"runs"},
        "run_list_by_parent": {"runs"},
        "runtime_get_checkpoint": {"checkpoint"},
        "runtime_state_ids": {"state_ids"},
        "runtime_clear_state": {"receipt"},
        "runtime_clear_scope": {"receipt"},
        "audit_query": {"events"},
        "context_project": {"projections"},
        "memory_trigger": {"disposition"},
        "guard_check": {"result"},
        "search_provider_search": {"results"},
        "workflow_checkpoint_save_if_generation": {"committed"},
        "workflow_checkpoint_load": {"checkpoint"},
        "workflow_checkpoint_claim": {"checkpoint"},
        "workflow_checkpoint_list": {"checkpoints"},
        "workflow_checkpoint_list_by_graph": {"checkpoints"},
        "workflow_checkpoint_list_filtered": {"checkpoints"},
        "revisioned_task_load": {"graph"},
        "revisioned_task_compare_and_commit": {"graph"},
        "sandbox_is_available": {"available"},
        "sandbox_execute": {"result"},
        "sandbox_execute_with_limits": {"result"},
        "sandbox_execute_with_limits_and_cancel": {"result"},
        "mcp_transport_send": {"response"},
        "mcp_transport_try_notification": {"notification"},
        "embedder_embed": {"vector"},
        "memory_promoter_promote": {"submitted", "promoted", "deduplicated"},
        "workflow_run": {"output"},
        "intent_classify": {"intent"},
        "skill_load_allows": {"allowed"},
    }
    expected_fields = fields_by_operation.get(operation)
    if expected_fields is None or set(value) != expected_fields:
        raise ValueError(f"{operation} result fields do not match its typed contract")

    if operation == "workflow_checkpoint_save_if_generation":
        if not isinstance(value.get("committed"), bool):
            raise TypeError("committed must be a bool")
        return

    def wire(field: str, nullable: bool = False) -> None:
        candidate = value.get(field)
        if nullable and candidate is None:
            return
        _component_wire(value, field)

    if operation in {
        "conversation_create",
        "conversation_ensure",
        "runtime_clear_state",
        "runtime_clear_scope",
        "guard_check",
        "revisioned_task_compare_and_commit",
        "sandbox_execute",
        "sandbox_execute_with_limits",
        "sandbox_execute_with_limits_and_cancel",
        "mcp_transport_send",
        "workflow_run",
        "intent_classify",
    }:
        field = next(iter(expected_fields))
        wire(field)
    elif operation in {
        "conversation_get",
        "run_load",
        "runtime_get_checkpoint",
        "workflow_checkpoint_load",
        "workflow_checkpoint_claim",
        "revisioned_task_load",
        "mcp_transport_try_notification",
    }:
        field = next(iter(expected_fields))
        wire(field, nullable=True)
    elif operation in {
        "conversation_list",
        "conversation_get_messages",
        "run_list_by_session",
        "run_list_all",
        "run_list_by_parent",
        "conversation_search",
        "audit_query",
        "context_project",
        "search_provider_search",
        "workflow_checkpoint_list",
        "workflow_checkpoint_list_by_graph",
        "workflow_checkpoint_list_filtered",
    }:
        field = next(iter(expected_fields))
        items = value.get(field)
        if not isinstance(items, list):
            raise TypeError(f"{field} must be a list")
        for item in items:
            _component_wire({"item": item}, "item")
    elif operation in {"conversation_count_messages"}:
        _canonical_u64_text(value.get("count"), "count")
    elif operation == "runtime_state_ids":
        state_ids = value.get("state_ids")
        if not isinstance(state_ids, list) or not all(
            isinstance(state_id, str) for state_id in state_ids
        ):
            raise TypeError("state_ids must be a list of strings")
    elif operation == "memory_trigger":
        if value.get("disposition") not in {"persist", "captured"}:
            raise ValueError("memory_trigger disposition is invalid")
    elif operation == "sandbox_is_available":
        if not isinstance(value.get("available"), bool):
            raise TypeError("available must be a bool")
    elif operation == "skill_load_allows":
        if not isinstance(value.get("allowed"), bool):
            raise TypeError("allowed must be a bool")
    elif operation == "embedder_embed":
        vector = value.get("vector")
        if not isinstance(vector, list) or not all(
            isinstance(item, (int, float))
            and not isinstance(item, bool)
            and math.isfinite(item)
            for item in vector
        ):
            raise TypeError("vector must contain finite numbers")
    elif operation == "memory_promoter_promote":
        for field in ("submitted", "promoted", "deduplicated"):
            _canonical_u64_text(value.get(field), field)


@dataclass(frozen=True, slots=True)
class AgentComponentOutcome:
    result: AgentComponentResult

    def to_wire(self) -> dict[str, Any]:
        expected_component = _component_for_operation(self.result.operation)
        if expected_component is None or self.result.component != expected_component:
            raise ValueError("Agent component result kind does not match its operation")
        _validate_component_result(self.result.operation, self.result.value)
        return ExtensionResultOutcome(
            "agent_component_call",
            {
                "component": self.result.component,
                "result": {
                    "operation": self.result.operation,
                    **(
                        {"value": dict(self.result.value)}
                        if self.result.value is not None
                        else {}
                    ),
                },
            },
        ).to_wire()


@dataclass(frozen=True, slots=True)
class ExtensionStreamOutcome:
    stream: WireHandle

    def to_wire(self) -> dict[str, Any]:
        return {"outcome": "stream", "stream": self.stream.to_dict()}


@dataclass(frozen=True, slots=True)
class ExtensionErrorOutcome:
    code: str
    message: str
    retryable: str = "never"
    operation: str | None = None
    details: Any = None

    def to_wire(self) -> dict[str, Any]:
        error: dict[str, Any] = {
            "code": self.code,
            "message": self.message,
            "retryable": self.retryable,
        }
        if self.operation is not None:
            error["operation"] = self.operation
        if self.details is not None:
            error["details"] = self.details
        return {"outcome": "error", "error": error}


class Tool(Protocol):
    async def execute(
        self, call: ToolCall, cancellation: asyncio.Event
    ) -> ExtensionOutcome | Mapping[str, Any]: ...


class LlmClient(Protocol):
    async def chat(
        self, call: LlmChatCall, cancellation: asyncio.Event
    ) -> ExtensionOutcome | Mapping[str, Any]: ...


class Store(Protocol):
    async def call(
        self, call: StoreCall, cancellation: asyncio.Event
    ) -> ExtensionOutcome | Mapping[str, Any]: ...


class Critic(Protocol):
    async def critique(
        self, call: CriticCall, cancellation: asyncio.Event
    ) -> ExtensionOutcome | Mapping[str, Any]: ...


class ContextCompressor(Protocol):
    async def compress(
        self, call: CompressionCall, cancellation: asyncio.Event
    ) -> ExtensionOutcome | Mapping[str, Any]: ...


class AgentComponent(Protocol):
    async def call(
        self, call: AgentComponentCall, cancellation: asyncio.Event
    ) -> ExtensionOutcome | Mapping[str, Any]: ...


def _optional_text(value: Any) -> str | None:
    return value if isinstance(value, str) else None


def _payload_handle(payload: Mapping[str, Any], key: str) -> WireHandle | None:
    value = payload.get(key)
    return WireHandle.from_dict(value) if isinstance(value, Mapping) else None


def _typed_call(kind: ExtensionKind, payload: Mapping[str, Any]) -> Any:
    extension = payload.get("extension")
    if not isinstance(extension, Mapping):
        raise TypeError("extension invocation is missing an extension handle")
    extension_handle = WireHandle.from_dict(extension)
    if extension_handle.kind != "extension":
        raise ValueError("extension invocation has an invalid extension handle")
    invocation_id = payload.get("invocation_id")
    if not isinstance(invocation_id, str) or not invocation_id.strip():
        raise ValueError("extension invocation_id must be non-empty text")
    deadline = payload.get("deadline")
    if not isinstance(deadline, Mapping):
        raise TypeError("extension invocation deadline must be an object")
    try:
        _canonical_u64_text(deadline.get("seconds"), "deadline.seconds")
    except (TypeError, ValueError):
        raise TypeError("extension invocation deadline seconds are malformed") from None
    if (
        not isinstance(deadline.get("nanos"), int)
        or isinstance(deadline.get("nanos"), bool)
        or not 0 <= deadline["nanos"] < 1_000_000_000
    ):
        raise TypeError("extension invocation deadline is malformed")
    stream = payload.get("stream")
    if stream is not None and (
        not isinstance(stream, Mapping) or WireHandle.from_dict(stream).kind != "stream"
    ):
        raise ValueError("extension invocation stream handle is invalid")
    if kind == "tool":
        return ToolCall.from_payload(payload)
    if kind == "llm_client":
        return LlmChatCall.from_payload(payload)
    if kind == "store":
        return StoreCall.from_payload(payload)
    if kind == "critic":
        return CriticCall.from_payload(payload)
    if kind == "context_compressor":
        return CompressionCall.from_payload(payload)
    if kind == "agent_component":
        return AgentComponentCall.from_payload(payload)
    return payload


def _typed_method(
    kind: ExtensionKind, operation: str, handler: Any
) -> Callable[..., Any]:
    if kind == "tool":
        method_name = (
            "validate_parameters"
            if operation == "tool_validate_parameters"
            else "execute"
        )
    elif kind == "llm_client":
        method_name = "chat"
    elif kind == "critic":
        method_name = "critique"
    elif kind == "context_compressor":
        method_name = "compress"
    else:
        method_name = "call"
    method = getattr(handler, method_name, None)
    if method is not None and callable(method):
        return method
    if kind == "tool" and method_name == "validate_parameters":
        method = getattr(handler, "execute", None)
        if method is not None and callable(method):
            return method
    if callable(handler):
        return handler
    raise TypeError(
        f"typed {kind} extension must be callable or implement {method_name}()"
    )


def _typed_handler(kind: ExtensionKind, handler: Any) -> Callable[..., Awaitable[Any]]:
    async def invoke(payload: Mapping[str, Any], cancellation: asyncio.Event) -> Any:
        call = _typed_call(kind, payload)
        operation = getattr(call, "operation", "")
        method = _typed_method(kind, operation, handler)
        result = method(call, cancellation)
        if inspect.isawaitable(result):
            result = await result
        if hasattr(result, "to_wire") and callable(result.to_wire):
            wire = result.to_wire()
        elif isinstance(result, Mapping) and "outcome" in result:
            wire = dict(result)
        else:
            if operation.endswith("_stream"):
                return ExtensionErrorOutcome(
                    "invalid_value",
                    f"{operation} requires a stream outcome",
                ).to_wire()
            wire = ExtensionResultOutcome(operation, result).to_wire()
        return _validate_typed_outcome(operation, wire)

    return invoke


def _validate_typed_outcome(operation: str, value: Any) -> dict[str, Any]:
    """Keep typed callbacks inside the Rust ExtensionInvokeOutcome union."""

    if not isinstance(value, Mapping):
        return ExtensionErrorOutcome(
            "invalid_value", "typed extension handler returned a non-object outcome"
        ).to_wire()
    outcome = value.get("outcome")
    if outcome == "result":
        result = value.get("result")
        if not isinstance(result, Mapping) or result.get("operation") != operation:
            return ExtensionErrorOutcome(
                "invalid_value",
                "typed extension result operation does not match invocation",
            ).to_wire()
    elif outcome == "stream":
        stream = value.get("stream")
        if not operation.endswith("_stream") or not isinstance(stream, Mapping):
            return ExtensionErrorOutcome(
                "invalid_value",
                "typed extension stream outcome does not match invocation",
            ).to_wire()
        try:
            handle = WireHandle.from_dict(stream)
        except (TypeError, ValueError, OverflowError):
            return ExtensionErrorOutcome(
                "invalid_value", "typed extension stream outcome has an invalid handle"
            ).to_wire()
        if handle.kind != "stream":
            return ExtensionErrorOutcome(
                "invalid_value", "typed extension stream outcome has an invalid handle"
            ).to_wire()
    elif outcome != "error" or not isinstance(value.get("error"), Mapping):
        return ExtensionErrorOutcome(
            "invalid_value", "typed extension handler returned a malformed outcome"
        ).to_wire()
    return dict(value)


def _cancelled_outcome() -> dict[str, Any]:
    return {
        "outcome": "error",
        "error": {
            "code": "cancelled",
            "message": "extension invocation was cancelled",
            "retryable": "never",
        },
    }


def _normalize_outcome(value: Any) -> dict[str, Any]:
    if not isinstance(value, dict) or value.get("outcome") not in {
        "result",
        "stream",
        "error",
    }:
        return {
            "outcome": "error",
            "error": {
                "code": "invalid_value",
                "message": "extension handler must return an outcome",
                "retryable": "never",
            },
        }
    outcome = value["outcome"]
    if outcome == "result" and "result" in value:
        return value
    if outcome == "stream" and isinstance(value.get("stream"), dict):
        return value
    if outcome == "error" and isinstance(value.get("error"), dict):
        return value
    return {
        "outcome": "error",
        "error": {
            "code": "invalid_value",
            "message": "extension handler returned a malformed outcome",
            "retryable": "never",
        },
    }


def _validate_queue_size(value: int, name: str) -> None:
    if isinstance(value, bool) or not isinstance(value, int) or value < 1:
        raise ValueError(f"{name} must be a positive integer")
    # Prevent an accidental multi-gigabyte local mailbox while still allowing
    # callers to choose a larger bound than the Host default when needed.
    if value > 65_536:
        raise ValueError(f"{name} exceeds the SDK safety bound")


class _AsyncQueue:
    """A bounded async mailbox used by one SDK subscription.

    The Host already bounds live event delivery with ACK windows. The SDK
    needs its own bound as well because a consumer can stop iterating after
    the ACP reader has accepted more notifications. Overflow is surfaced as
    the typed ``event_gap`` error instead of silently dropping facts.
    """

    def __init__(
        self,
        maxsize: int = 128,
        on_consume: Callable[[Any], Awaitable[None]] | None = None,
    ) -> None:
        if isinstance(maxsize, bool) or not isinstance(maxsize, int) or maxsize < 1:
            raise ValueError("async queue maxsize must be a positive integer")
        self._items: deque[Any] = deque()
        self._waiters: deque[asyncio.Future[Any]] = deque()
        self._maxsize = maxsize
        self._on_consume = on_consume
        self._closed = False
        self._failure: BaseException | None = None

    def set_on_consume(self, callback: Callable[[Any], Awaitable[None]] | None) -> None:
        self._on_consume = callback

    def push(self, value: Any) -> None:
        if self._closed:
            return
        while self._waiters:
            waiter = self._waiters.popleft()
            if waiter.done():
                continue
            waiter.set_result(value)
            return
        if len(self._items) >= self._maxsize:
            self.fail(
                EchoAgentError(
                    "event_gap",
                    "SDK subscription queue exceeded its bounded capacity",
                    details={"reason": "client_queue_full", "capacity": self._maxsize},
                )
            )
            return
        self._items.append(value)

    def fail(self, error: BaseException, *, discard_pending: bool = False) -> None:
        if discard_pending:
            self._items.clear()
        if self._closed:
            return
        self._closed = True
        self._failure = error
        while self._waiters:
            waiter = self._waiters.popleft()
            if not waiter.done():
                waiter.set_exception(error)

    def close(self) -> None:
        if self._closed:
            return
        self._closed = True
        while self._waiters:
            waiter = self._waiters.popleft()
            if not waiter.done():
                waiter.set_exception(StopAsyncIteration)

    async def get(self) -> Any:
        if self._items:
            value = self._items.popleft()
            if self._on_consume is not None:
                await self._on_consume(value)
            return value
        if self._failure is not None:
            raise self._failure
        if self._closed:
            raise StopAsyncIteration
        waiter = asyncio.get_running_loop().create_future()
        self._waiters.append(waiter)
        try:
            value = await waiter
            if self._on_consume is not None:
                await self._on_consume(value)
            return value
        except asyncio.CancelledError:
            try:
                self._waiters.remove(waiter)
            except ValueError:
                pass
            raise

    async def __aiter__(self) -> AsyncIterator[Any]:
        while True:
            try:
                yield await self.get()
            except StopAsyncIteration:
                return


@dataclass(slots=True)
class _EventFeed:
    """One generation-fenced event feed owned by a stream id."""

    queue: _AsyncQueue
    handle: WireHandle | None = None
    last_sequence: int = 0

    def bind(self, handle: WireHandle) -> None:
        if handle.kind != "stream":
            raise EchoAgentError(
                "serialization_violation", "event stream handle kind is invalid"
            )
        if self.handle is not None and self.handle != handle:
            raise EchoAgentError(
                "handle_mismatch", "event stream handle changed generation"
            )
        self.handle = handle

    def accept_event(self, value: Mapping[str, Any]) -> bool:
        stream = _parse_event_stream(value)
        self.bind(stream)
        envelope = value.get("envelope")
        if not isinstance(envelope, Mapping):
            raise EchoAgentError(
                "serialization_violation", "event notification is missing its envelope"
            )
        if envelope.get("stream_id") != stream.id:
            raise EchoAgentError(
                "serialization_violation",
                "event envelope stream_id does not match its handle",
            )
        sequence = _parse_positive_sequence(envelope.get("sequence"), "event sequence")
        if sequence == self.last_sequence:
            return False
        if self.last_sequence > 0 and sequence != self.last_sequence + 1:
            raise EchoAgentError(
                "event_gap", "event sequence is not contiguous; replay is required"
            )
        self.last_sequence = sequence
        return True

    def accept_gap(self, value: Mapping[str, Any]) -> bool:
        stream = _parse_event_stream(value)
        self.bind(stream)
        gap = value.get("gap")
        if not isinstance(gap, Mapping):
            raise EchoAgentError(
                "serialization_violation", "gap notification is missing its gap"
            )
        from_sequence = _parse_positive_sequence(
            gap.get("from_sequence"), "gap from_sequence"
        )
        to_sequence = _parse_positive_sequence(
            gap.get("to_sequence"), "gap to_sequence"
        )
        watermark = _parse_positive_sequence(
            gap.get("snapshot_watermark"), "gap snapshot_watermark"
        )
        reason = gap.get("reason")
        if (
            not isinstance(reason, str)
            or not reason.strip()
            or to_sequence < from_sequence
            or watermark < to_sequence
            or watermark < self.last_sequence
            or (
                watermark > self.last_sequence
                and self.last_sequence > 0
                and from_sequence != self.last_sequence + 1
            )
        ):
            raise EchoAgentError(
                "serialization_violation", "gap sequence range is malformed"
            )
        if watermark == self.last_sequence:
            return False
        self.last_sequence = watermark
        return True


def _parse_positive_sequence(value: Any, name: str) -> int:
    if not isinstance(value, str):
        raise EchoAgentError("serialization_violation", f"{name} must be decimal text")
    try:
        text = _canonical_u64_text(value, name)
    except (TypeError, ValueError, OverflowError) as error:
        raise EchoAgentError(
            "serialization_violation", f"{name} must be canonical u64 text"
        ) from error
    parsed = int(text)
    if parsed < 1:
        raise EchoAgentError("serialization_violation", f"{name} must be positive")
    return parsed


def _parse_event_stream(value: Mapping[str, Any]) -> WireHandle:
    try:
        stream = WireHandle.from_dict(value.get("stream"))
    except (TypeError, ValueError, OverflowError) as error:
        raise EchoAgentError(
            "serialization_violation", "event notification has an invalid stream handle"
        ) from error
    if stream.kind != "stream":
        raise EchoAgentError(
            "serialization_violation", "event notification has an invalid stream handle"
        )
    return stream


class _Callbacks:
    def __init__(self, updates: dict[str, _AsyncQueue], queue_size: int) -> None:
        self._updates = updates
        self._queue_size = queue_size
        self.extensions: dict[str, Any] = {}
        self.invocation_cancel: dict[str, asyncio.Event] = {}

    async def session_update(self, session_id: str, update: Any, **kwargs: Any) -> None:
        value = (
            update.model_dump(by_alias=True)
            if hasattr(update, "model_dump")
            else update
        )
        self._updates.setdefault(session_id, _AsyncQueue(self._queue_size)).push(
            {"sessionId": session_id, "update": value}
        )

    async def request_permission(self, *args: Any, **kwargs: Any) -> Any:
        # The SDK advertises no permission capability by default. If a Host
        # still requests one, preserve the typed protocol failure.
        from acp.exceptions import RequestError

        raise RequestError.method_not_found("session/request_permission")

    def on_connect(self, _connection: Any) -> None:
        return None

    async def ext_method(self, name: str, payload: dict[str, Any]) -> Any:
        if name != "echo_agent/extension/invoke":
            from acp.exceptions import RequestError

            raise RequestError.method_not_found(f"_{name}")
        extension = payload.get("extension")
        extension_id = extension.get("id") if isinstance(extension, dict) else None
        handler = self.extensions.get(extension_id)
        if handler is None:
            return {
                "outcome": "error",
                "error": {
                    "code": "invalid_value",
                    "message": "extension registration is not available",
                    "retryable": "never",
                },
            }
        invocation_id = str(payload.get("invocation_id", ""))
        cancellation = asyncio.Event()
        self.invocation_cancel[invocation_id] = cancellation
        try:
            result = await handler(payload, cancellation)
            return (
                _cancelled_outcome()
                if cancellation.is_set()
                else _normalize_outcome(result)
            )
        except EchoAgentError as error:
            code = (
                error.code
                if error.code in _EXTENSION_ERROR_CODES
                else "extension_failed"
            )
            return {
                "outcome": "error",
                "error": {
                    "code": code,
                    "message": error.message,
                    "retryable": error.retryable,
                    "operation": error.operation,
                    "details": error.details,
                },
            }
        # Every user callback failure must settle the reverse request with the
        # closed extension error union; otherwise ACP serializes an invalid
        # transport error and the Host cannot finish the invocation.
        except Exception as error:  # noqa: BLE001
            return {
                "outcome": "error",
                "error": {
                    "code": "extension_failed",
                    "message": str(error) or "typed extension handler failed",
                    "retryable": "never",
                },
            }
        finally:
            self.invocation_cancel.pop(invocation_id, None)

    async def ext_notification(self, name: str, payload: dict[str, Any]) -> None:
        if name != "echo_agent/extension/cancel":
            return
        invocation_id = payload.get("invocation_id")
        event = self.invocation_cancel.get(invocation_id)
        if event is not None:
            event.set()

    def cancel_all(self) -> None:
        for event in self.invocation_cancel.values():
            event.set()


class EchoAgentClient:
    def __init__(
        self,
        connection: Any,
        process: asyncio.subprocess.Process,
        context_manager: Any,
        catalog: FacadeCatalog,
        capability: Mapping[str, Any],
        updates: dict[str, _AsyncQueue],
        events: dict[str, _EventFeed],
        callbacks: _Callbacks,
        event_queue_size: int,
        update_queue_size: int,
        process_watch: asyncio.Task[None] | None,
    ) -> None:
        self.connection = connection
        self.process = process
        self._context_manager = context_manager
        self.catalog = catalog
        self.capability = dict(capability)
        self._updates = updates
        self._events: dict[str, _EventFeed] = events
        self._callbacks = callbacks
        self._event_queue_size = event_queue_size
        self._update_queue_size = update_queue_size
        self._process_watch = process_watch
        self._close_lock = asyncio.Lock()
        self._event_cursors: dict[WireHandle, int] = {}
        self._closed = False

    @classmethod
    async def spawn(
        cls,
        host_command: str,
        *args: str,
        cwd: str | Path | None = None,
        env: Mapping[str, str] | None = None,
        catalog_path: str | Path | None = None,
        required_features: Iterable[str] = (),
        required_capabilities: Iterable[str] = (),
        max_buffered_events: int = 128,
        max_buffered_updates: int = 128,
    ) -> EchoAgentClient:
        _validate_queue_size(max_buffered_events, "max_buffered_events")
        _validate_queue_size(max_buffered_updates, "max_buffered_updates")
        catalog = FacadeCatalog(catalog_path)
        updates: dict[str, _AsyncQueue] = {}
        events: dict[str, _EventFeed] = {}

        async def observe(event: StreamEvent) -> None:
            if event.direction is not StreamDirection.INCOMING:
                return
            message = event.message
            if message.get("method") != "_echo_agent/event":
                return
            params = message.get("params")
            if not isinstance(params, dict):
                error = EchoAgentError(
                    "serialization_violation", "event notification must be an object"
                )
                for feed in events.values():
                    feed.queue.fail(error)
                return
            try:
                stream = _parse_event_stream(params)
            except EchoAgentError as error:
                for feed in events.values():
                    feed.queue.fail(error)
                return
            feed = events.setdefault(
                stream.id, _EventFeed(_AsyncQueue(max_buffered_events))
            )
            try:
                if feed.accept_event(params):
                    feed.queue.push(params)
            except EchoAgentError as error:
                feed.queue.fail(error)

        async def observe_gap(event: StreamEvent) -> None:
            if event.direction is not StreamDirection.INCOMING:
                return
            message = event.message
            if message.get("method") != "_echo_agent/gap":
                return
            params = message.get("params")
            if not isinstance(params, dict):
                error = EchoAgentError(
                    "serialization_violation", "gap notification must be an object"
                )
                for feed in events.values():
                    feed.queue.fail(error)
                return
            try:
                stream = _parse_event_stream(params)
            except EchoAgentError as error:
                for feed in events.values():
                    feed.queue.fail(error)
                return
            feed = events.setdefault(
                stream.id, _EventFeed(_AsyncQueue(max_buffered_events))
            )
            try:
                if feed.accept_gap(params):
                    feed.queue.push(params)
            except EchoAgentError as error:
                feed.queue.fail(error)

        callback_client = _Callbacks(updates, max_buffered_updates)
        merged_env = dict(os.environ)
        if env:
            merged_env.update(env)
        context_manager = spawn_agent_process(
            callback_client,
            host_command,
            *args,
            cwd=cwd,
            env=merged_env,
            observers=[observe, observe_gap],
        )
        try:
            connection, process = await context_manager.__aenter__()
            hello = {
                "extension_protocol_version": 1,
                "contract_digest": catalog.contract_digest,
                "source_contract_digest": catalog.source_contract_digest,
                "required_features": sorted(set(required_features)),
                "required_capabilities": sorted(set(required_capabilities)),
            }
            initialized = await connection.initialize(
                1,
                client_capabilities=ClientCapabilities.model_validate(
                    {"_meta": {"echo_agent": hello}}
                ),
                client_info=Implementation(
                    name="echo-agent-sdk-python", version="0.1.0"
                ),
            )
            raw_capability = (
                initialized.agent_capabilities.field_meta.get("echo_agent")
                if initialized.agent_capabilities
                else None
            )
            if not isinstance(raw_capability, dict):
                raise EchoAgentError(
                    "extension_capability_mismatch",
                    "Host did not advertise echo-agent extension",
                )
            cls._validate_capability(
                raw_capability, required_features, required_capabilities, catalog
            )
            client = cls(
                connection,
                process,
                context_manager,
                catalog,
                raw_capability,
                updates,
                events,
                callback_client,
                max_buffered_events,
                max_buffered_updates,
                None,
            )
            client._process_watch = asyncio.create_task(
                client._watch_process(), name="echo-agent-sdk-python.process"
            )
            return client
        except asyncio.CancelledError:
            await context_manager.__aexit__(None, None, None)
            raise
        except Exception:
            await context_manager.__aexit__(None, None, None)
            raise

    @staticmethod
    def _validate_capability(
        capability: Mapping[str, Any],
        required_features: Iterable[str],
        required_capabilities: Iterable[str],
        catalog: FacadeCatalog,
    ) -> None:
        if capability.get("contract_digest") != catalog.contract_digest:
            raise EchoAgentError(
                "extension_capability_mismatch", "contract digest mismatch"
            )
        if capability.get("source_contract_digest") != catalog.source_contract_digest:
            raise EchoAgentError(
                "extension_capability_mismatch", "source contract digest mismatch"
            )
        features = capability.get("features", [])
        declared = (
            {str(value) for value in features} if isinstance(features, list) else set()
        )
        for feature in required_features:
            if feature not in declared:
                raise EchoAgentError(
                    "feature_unavailable", f"Host lacks feature {feature}"
                )
        capabilities = capability.get("capabilities", [])
        names = {
            str(value.get("capability"))
            for value in capabilities
            if isinstance(value, dict) and isinstance(value.get("capability"), str)
        }
        for name in required_capabilities:
            if name not in names:
                raise EchoAgentError(
                    "extension_capability_mismatch", f"Host lacks capability {name}"
                )

    async def request(self, method: str, params: Any = None) -> Any:
        try:
            return await self.connection.ext_method(
                method.removeprefix("_"), params or {}
            )
        except asyncio.CancelledError:
            raise
        except Exception as error:
            raise EchoAgentError.from_exception(error, method) from error

    async def _watch_process(self) -> None:
        try:
            await self.process.wait()
        except asyncio.CancelledError:
            raise
        except (OSError, asyncio.subprocess.SubprocessError) as error:
            failure = EchoAgentError(
                "host_exited", "Host process watcher failed", details=str(error)
            )
        else:
            failure = EchoAgentError(
                "host_exited",
                "Host process exited before the SDK client closed",
                details={"returncode": self.process.returncode},
            )
        self._callbacks.cancel_all()
        for feed in self._events.values():
            feed.queue.fail(failure)
        for queue in self._updates.values():
            queue.fail(failure)

    async def notify(self, method: str, params: Any = None) -> None:
        try:
            await self.connection.ext_notification(
                method.removeprefix("_"), params or {}
            )
        except asyncio.CancelledError:
            raise
        except Exception as error:
            raise EchoAgentError.from_exception(error, method) from error

    async def invoke(
        self, operation: str, handle: WireHandle | None, arguments: Iterable[Any] = ()
    ) -> Any:
        response = await self.request(
            "echo_agent/facade/invoke",
            {
                "operation": operation,
                "signature_digest": self.catalog.signature(operation),
                "handle": handle.to_dict() if handle else None,
                "arguments": [to_wire(value) for value in arguments],
            },
        )
        return from_wire(
            response.get("value") if isinstance(response, dict) else response
        )

    async def count_tokens(self, tokenizer: TokenizerReference, text: str) -> int:
        """Count with the exact Host-owned tokenizer of a compression call."""

        value = await self.invoke(
            "echo_core::tokenizer::Tokenizer::count_tokens",
            tokenizer.resource,
            [tokenizer.owner_session_id, text],
        )
        return int(_canonical_u64_text(value, "token count"))

    async def classify_turn_outcome(self, event_wire: Mapping[str, Any]) -> Any | None:
        """Classify an AgentEventWire through the framework authority."""

        if not isinstance(event_wire, Mapping):
            raise TypeError("event_wire must be a mapping")
        operation = "echo_orchestration::runtime::turn_driver::TurnOutcome::classify"
        # Keep explicit Variant/Null wire values intact; Rust owns the event
        # grammar and outcome classification rules.
        return await self.invoke(operation, None, [event_wire])

    async def call(
        self, operation: str, handle: WireHandle | None, arguments: Iterable[Any] = ()
    ) -> Any:
        resolved = self.catalog.resolve(operation)
        if resolved.method == "_echo_agent/facade/invoke":
            return await self.invoke(operation, handle, arguments)
        return await self.family(resolved.family, operation, handle, arguments)

    async def family(
        self,
        family: str,
        operation: str,
        session: WireHandle | None,
        arguments: Iterable[Any] = (),
    ) -> Any:
        method = f"echo_agent/{family}/op"
        response = await self.request(
            method,
            {
                "operation": operation,
                "signature_digest": self.catalog.family_signature(
                    f"_{method}", operation
                ),
                "handle": session.to_dict() if session else None,
                "arguments": [to_wire(value) for value in arguments],
            },
        )
        return from_wire(
            response.get("value") if isinstance(response, dict) else response
        )

    def updates_for(self, session_id: str) -> AsyncIterator[Any]:
        return self._updates.setdefault(
            session_id, _AsyncQueue(self._update_queue_size)
        ).__aiter__()

    def events_for(
        self, stream_id: str, stream: WireHandle | None = None
    ) -> AsyncIterator[Any]:
        feed = self._events.setdefault(
            stream_id, _EventFeed(_AsyncQueue(self._event_queue_size))
        )
        if stream is not None:
            try:
                if stream.id != stream_id:
                    raise EchoAgentError(
                        "handle_mismatch",
                        "event stream id does not match the requested feed",
                    )
                feed.bind(stream)
            except EchoAgentError as error:
                feed.queue.fail(error, discard_pending=True)
        return self._iterate_events(feed.queue, stream)

    async def _iterate_events(
        self, queue: _AsyncQueue, stream: WireHandle | None
    ) -> AsyncIterator[Any]:
        pending: Any = None
        try:
            async for value in queue:
                if pending is not None and stream is not None:
                    await self._ack_event(stream, pending)
                pending = value
                yield value
        finally:
            if pending is not None and stream is not None and not self._closed:
                with contextlib.suppress(Exception):
                    await self._ack_event(stream, pending)

    async def _ack_event(self, stream: WireHandle, value: Any) -> None:
        if not isinstance(value, dict):
            return
        envelope = value.get("envelope")
        gap = value.get("gap")
        sequence = (
            envelope.get("sequence")
            if isinstance(envelope, dict)
            else gap.get("snapshot_watermark")
            if isinstance(gap, dict)
            else None
        )
        try:
            event_stream = _parse_event_stream(value)
        except EchoAgentError:
            return
        if event_stream != stream:
            return
        if (
            not isinstance(sequence, str)
            or not sequence.isascii()
            or not sequence.isdigit()
        ):
            return
        if sequence.startswith("0") and sequence != "0":
            return
        parsed = int(sequence)
        if parsed < 1 or parsed <= self._event_cursors.get(stream, 0):
            return
        await self.notify(
            "echo_agent/event/ack",
            {
                "ack": {
                    "stream": stream.to_dict(),
                    "last_processed_sequence": sequence,
                }
            },
        )
        self._event_cursors[stream] = parsed

    def stream_writer(self, stream: WireHandle) -> ExtensionStreamWriter:
        return ExtensionStreamWriter(self, stream)

    async def create_agent(
        self, config: Any | None = None, idempotency_id: str | None = None
    ) -> AgentHandle:
        response = await self.request(
            "echo_agent/agent/create",
            {
                "config": config or {"variant": "host_default"},
                "idempotency_id": idempotency_id,
            },
        )
        return AgentHandle(self, WireHandle.from_dict(response["agent"]))

    async def register_extension(
        self,
        kind: str,
        implementation_id: str,
        descriptor: Any,
        handler: Any,
        timeout: dict[str, Any] | None = None,
    ) -> ExtensionRegistration:
        response = await self.request(
            "echo_agent/extension/register",
            {
                "kind": kind,
                "implementation_id": implementation_id,
                "descriptor": descriptor,
                "timeout": timeout,
            },
        )
        registration = ExtensionRegistration(
            self, WireHandle.from_dict(response["extension"])
        )
        self._callbacks.extensions[registration.wire.id] = handler
        return registration

    async def register_typed_extension(
        self,
        implementation_id: str,
        descriptor: ExtensionDescriptor
        | ToolDescriptor
        | LlmClientDescriptor
        | StoreDescriptor
        | CriticDescriptor
        | ContextCompressorDescriptor
        | AgentComponentDescriptor,
        handler: Any,
        timeout: dict[str, Any] | None = None,
    ) -> ExtensionRegistration:
        typed_handler = handler
        if hasattr(descriptor, "to_wire"):
            descriptor = descriptor.to_wire()  # type: ignore[assignment]
            if isinstance(descriptor, Mapping) and isinstance(
                descriptor.get("kind"), str
            ):
                typed_handler = _typed_handler(
                    cast(ExtensionKind, descriptor["kind"]), handler
                )
        if (
            not isinstance(descriptor, Mapping)
            or descriptor.get("descriptor_version") != 1
        ):
            raise EchoAgentError(
                "invalid_value", "only extension descriptor_version 1 is supported"
            )
        return await self.register_extension(
            cast(str, descriptor["kind"]),
            implementation_id,
            descriptor,
            typed_handler,
            timeout,
        )

    async def _register_kind(
        self,
        expected: ExtensionKind,
        implementation_id: str,
        descriptor: ExtensionDescriptor
        | ToolDescriptor
        | LlmClientDescriptor
        | StoreDescriptor
        | CriticDescriptor,
        handler: Any,
    ) -> ExtensionRegistration:
        if hasattr(descriptor, "to_wire"):
            typed_descriptor = descriptor.to_wire()
            typed_handler = _typed_handler(expected, handler)
        else:
            typed_descriptor = descriptor
            typed_handler = handler
        if (
            not isinstance(typed_descriptor, Mapping)
            or typed_descriptor.get("kind") != expected
        ):
            raise EchoAgentError(
                "invalid_value",
                f"expected {expected} extension descriptor, received {typed_descriptor.get('kind') if isinstance(typed_descriptor, Mapping) else None}",
            )
        return await self.register_typed_extension(
            implementation_id, typed_descriptor, typed_handler
        )

    async def register_tool(
        self,
        implementation_id: str,
        descriptor: ExtensionDescriptor | ToolDescriptor,
        handler: Any,
    ) -> ExtensionRegistration:
        return await self._register_kind("tool", implementation_id, descriptor, handler)

    async def register_llm_client(
        self,
        implementation_id: str,
        descriptor: ExtensionDescriptor | LlmClientDescriptor,
        handler: Any,
    ) -> ExtensionRegistration:
        return await self._register_kind(
            "llm_client", implementation_id, descriptor, handler
        )

    async def register_store(
        self,
        implementation_id: str,
        descriptor: ExtensionDescriptor | StoreDescriptor,
        handler: Any,
    ) -> ExtensionRegistration:
        return await self._register_kind(
            "store", implementation_id, descriptor, handler
        )

    async def register_critic(
        self,
        implementation_id: str,
        descriptor: ExtensionDescriptor | CriticDescriptor,
        handler: Any,
    ) -> ExtensionRegistration:
        return await self._register_kind(
            "critic", implementation_id, descriptor, handler
        )

    async def register_human_loop_provider(
        self, implementation_id: str, descriptor: ExtensionDescriptor, handler: Any
    ) -> ExtensionRegistration:
        return await self._register_kind(
            "human_loop_provider", implementation_id, descriptor, handler
        )

    async def register_hook(
        self, implementation_id: str, descriptor: ExtensionDescriptor, handler: Any
    ) -> ExtensionRegistration:
        return await self._register_kind("hook", implementation_id, descriptor, handler)

    async def register_agent_callback(
        self, implementation_id: str, descriptor: ExtensionDescriptor, handler: Any
    ) -> ExtensionRegistration:
        return await self._register_kind(
            "agent_callback", implementation_id, descriptor, handler
        )

    async def register_intervention_callback(
        self, implementation_id: str, descriptor: ExtensionDescriptor, handler: Any
    ) -> ExtensionRegistration:
        return await self._register_kind(
            "intervention_callback", implementation_id, descriptor, handler
        )

    async def register_agent_factory(
        self, implementation_id: str, descriptor: ExtensionDescriptor, handler: Any
    ) -> ExtensionRegistration:
        return await self._register_kind(
            "agent_factory", implementation_id, descriptor, handler
        )

    async def register_custom_agent(
        self, implementation_id: str, descriptor: ExtensionDescriptor, handler: Any
    ) -> ExtensionRegistration:
        return await self._register_kind(
            "custom_agent", implementation_id, descriptor, handler
        )

    async def register_channel_plugin(
        self, implementation_id: str, descriptor: ExtensionDescriptor, handler: Any
    ) -> ExtensionRegistration:
        return await self._register_kind(
            "channel_plugin", implementation_id, descriptor, handler
        )

    async def register_channel_message_handler(
        self, implementation_id: str, descriptor: ExtensionDescriptor, handler: Any
    ) -> ExtensionRegistration:
        return await self._register_kind(
            "channel_message_handler", implementation_id, descriptor, handler
        )

    async def register_context_compressor(
        self,
        implementation_id: str,
        descriptor: ContextCompressorDescriptor,
        handler: ContextCompressor,
    ) -> ExtensionRegistration:
        return await self._register_kind(
            "context_compressor",
            implementation_id,
            descriptor,
            handler,
        )

    async def register_agent_component(
        self,
        implementation_id: str,
        descriptor: AgentComponentDescriptor,
        handler: AgentComponent,
    ) -> ExtensionRegistration:
        return await self._register_kind(
            "agent_component", implementation_id, descriptor, handler
        )

    async def unregister_extension(self, extension: WireHandle) -> Any:
        self._callbacks.extensions.pop(extension.id, None)
        return await self.request(
            "echo_agent/extension/unregister", {"extension": extension.to_dict()}
        )

    async def close(self) -> None:
        async with self._close_lock:
            if self._closed:
                return
            self._closed = True
            self._callbacks.cancel_all()
            for feed in self._events.values():
                feed.queue.close()
            for queue in self._updates.values():
                queue.close()
            process_watch = self._process_watch
            self._process_watch = None
            if process_watch is not None:
                process_watch.cancel()
                with contextlib.suppress(asyncio.CancelledError):
                    await process_watch
        await self._context_manager.__aexit__(None, None, None)

    async def __aenter__(self) -> Self:
        return self

    async def __aexit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        tb: TracebackType | None,
    ) -> None:
        await self.close()


class AgentHandle:
    def __init__(self, client: EchoAgentClient, wire: WireHandle) -> None:
        self.client = client
        self.wire = wire
        self._closed = False

    async def describe(self) -> Any:
        self._ensure_open("echo_agent/agent/describe")
        return await self.client.request(
            "echo_agent/agent/describe", {"agent": self.wire.to_dict()}
        )

    async def create_session(self, cwd: str | None = None) -> SessionHandle:
        self._ensure_open("echo_agent/session/create")
        working_dir = {"encoding": "utf8", "path": cwd} if cwd else None
        response = await self.client.request(
            "echo_agent/session/create",
            {"agent": self.wire.to_dict(), "working_dir": working_dir},
        )
        return SessionHandle(
            self.client,
            self.wire,
            WireHandle.from_dict(response["session"]),
            str(response["acp_session_id"]),
            WireHandle.from_dict(response["task_run"]),
        )

    async def close(self) -> None:
        if self._closed:
            return
        await self.client.request(
            "echo_agent/agent/close", {"agent": self.wire.to_dict()}
        )
        self._closed = True

    def _ensure_open(self, operation: str) -> None:
        if self._closed:
            raise EchoAgentError(
                "closed_handle", "Agent handle is closed", operation=operation
            )


class ExtensionRegistration:
    def __init__(self, client: EchoAgentClient, wire: WireHandle) -> None:
        self.client = client
        self.wire = wire
        self._closed = False

    async def close(self) -> None:
        if self._closed:
            return
        self._closed = True
        await self.client.unregister_extension(self.wire)


def _validate_agent_component_stream_chunk(value: Mapping[str, Any]) -> None:
    component = value.get("component")
    event = value.get("event")
    if not isinstance(event, Mapping):
        raise TypeError("Agent component stream chunk event must be an object")
    if component == "sandbox":
        if (
            set(event) != {"event", "channel", "chunk"}
            or event.get("event") != "output"
        ):
            raise ValueError("sandbox stream chunks must be output events")
        if event.get("channel") not in {"stdout", "stderr"} or not isinstance(
            event.get("chunk"), str
        ):
            raise TypeError("sandbox output channel or chunk is invalid")
        return
    if component == "workflow":
        kind = event.get("event")
        fields = {
            "node_start": {"event", "node_name", "step_index"},
            "node_end": {"event", "node_name", "step_index", "elapsed"},
            "token": {"event", "node_name", "token"},
            "node_error": {"event", "node_name", "error"},
        }.get(kind)
        if fields is None or set(event) != fields:
            raise ValueError(
                "workflow stream chunk does not match a non-terminal event"
            )
        return
    raise ValueError("Agent component stream chunk has an invalid component")


def _validate_agent_component_stream_complete(value: Mapping[str, Any]) -> None:
    component = value.get("component")
    terminal = value.get("terminal")
    if not isinstance(terminal, Mapping):
        raise TypeError("Agent component stream terminal must be an object")
    if component == "sandbox":
        kind = terminal.get("terminal")
        if kind == "complete":
            if set(terminal) != {"terminal", "result"}:
                raise ValueError("sandbox complete terminal fields are invalid")
            _component_wire(terminal, "result")
            return
        if kind == "failed":
            failure = terminal.get("failure")
            if (
                set(terminal) != {"terminal", "failure"}
                or not isinstance(failure, Mapping)
                or set(failure) != {"kind", "message"}
                or failure.get("kind") not in {"cancelled", "io_error"}
                or not isinstance(failure.get("message"), str)
            ):
                raise ValueError("sandbox failed terminal fields are invalid")
            return
        raise ValueError("sandbox stream terminal must be complete or failed")
    if component == "workflow":
        if set(terminal) != {"result", "total_steps", "elapsed"}:
            raise ValueError("workflow completed terminal fields are invalid")
        if not isinstance(terminal.get("result"), str):
            raise TypeError("workflow result must be text")
        _canonical_u64_text(terminal.get("total_steps"), "total_steps")
        if not isinstance(terminal.get("elapsed"), Mapping):
            raise TypeError("workflow elapsed must be a duration")
        return
    raise ValueError("Agent component stream terminal has an invalid component")


class ExtensionStreamWriter:
    def __init__(self, client: EchoAgentClient, stream: WireHandle) -> None:
        self.client = client
        self.stream = stream
        self._sequence = 0
        self._closed = False

    def _next(self) -> str:
        if self._closed:
            raise RuntimeError("extension stream is already closed")
        if self._sequence >= MAX_U64:
            raise OverflowError("extension stream sequence exceeds u64")
        self._sequence += 1
        return str(self._sequence)

    async def chunk(self, value: Any) -> None:
        await self.client.notify(
            "echo_agent/extension/stream",
            {
                "event": "chunk",
                "stream": self.stream.to_dict(),
                "sequence": self._next(),
                "value": value,
            },
        )

    async def agent_component_chunk(self, value: Mapping[str, Any]) -> None:
        _validate_agent_component_stream_chunk(value)
        await self.chunk({"kind": "agent_component", "value": dict(value)})

    async def complete(self, value: Any) -> None:
        await self.client.notify(
            "echo_agent/extension/stream",
            {
                "event": "complete",
                "stream": self.stream.to_dict(),
                "sequence": self._next(),
                "value": value,
            },
        )
        self._closed = True

    async def agent_component_complete(self, value: Mapping[str, Any]) -> None:
        _validate_agent_component_stream_complete(value)
        await self.complete({"kind": "agent_component", "value": dict(value)})

    async def failed(self, error: Any) -> None:
        await self.client.notify(
            "echo_agent/extension/stream",
            {
                "event": "failed",
                "stream": self.stream.to_dict(),
                "sequence": self._next(),
                "error": error,
            },
        )
        self._closed = True

    async def cancelled(self) -> None:
        await self.client.notify(
            "echo_agent/extension/stream",
            {
                "event": "cancelled",
                "stream": self.stream.to_dict(),
                "sequence": self._next(),
            },
        )
        self._closed = True


class SessionHandle:
    def __init__(
        self,
        client: EchoAgentClient,
        agent: WireHandle,
        wire: WireHandle,
        acp_session_id: str,
        task_run: WireHandle,
    ) -> None:
        self.client = client
        self.agent = agent
        self.wire = wire
        self.acp_session_id = acp_session_id
        self.task_run = task_run
        self._closed = False
        self._runs: dict[str, RunHandle] = {}

    async def prompt(self, text: str) -> Any:
        self._ensure_open("session/prompt")
        return await self.client.connection.prompt(
            self.acp_session_id,
            [
                {"type": "text", "text": text},
            ],
        )

    def updates(self) -> AsyncIterator[Any]:
        self._ensure_open("session/update")
        return self.client.updates_for(self.acp_session_id)

    async def start_run(
        self,
        text: str,
        *,
        execute: bool = False,
        idempotency_id: str | None = None,
    ) -> RunHandle:
        self._ensure_open("echo_agent/run/start")
        response = await self.client.request(
            "echo_agent/run/start",
            {
                "session": self.wire.to_dict(),
                "input": {
                    "kind": "execute" if execute else "chat",
                    "task" if execute else "text": text,
                },
                "idempotency_id": idempotency_id,
            },
        )
        run = RunHandle(
            self.client,
            WireHandle.from_dict(response["run"]),
            WireHandle.from_dict(response["stream"]),
        )
        self._runs[run.wire.id] = run
        return run

    async def invoke(self, operation: str, arguments: Iterable[Any] = ()) -> Any:
        self._ensure_open(operation)
        resolved = self.client.catalog.resolve(operation)
        if resolved.method == "_echo_agent/facade/invoke":
            return await self.client.call(
                operation, self.agent, [self.wire, *arguments]
            )
        return await self.client.call(operation, self.wire, arguments)

    async def cancel(self) -> None:
        self._ensure_open("session/cancel")
        await self.client.connection.cancel(self.acp_session_id)

    async def close(self) -> None:
        if self._closed:
            return
        await self.client.request(
            "echo_agent/session/close", {"session": self.wire.to_dict()}
        )
        self._closed = True
        for run in self._runs.values():
            run._mark_closed()
        queue = self.client._updates.get(self.acp_session_id)
        if queue is not None:
            queue.close()

    def _ensure_open(self, operation: str) -> None:
        if self._closed:
            raise EchoAgentError(
                "closed_handle", "Session handle is closed", operation=operation
            )

    async def __aenter__(self) -> Self:
        self._ensure_open("session/enter")
        return self

    async def __aexit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        tb: TracebackType | None,
    ) -> None:
        await self.close()


class RunHandle:
    def __init__(
        self, client: EchoAgentClient, wire: WireHandle, stream: WireHandle
    ) -> None:
        self.client = client
        self.wire = wire
        self.stream = stream
        self._closed = False

    @property
    def events(self) -> AsyncIterator[Any]:
        self._ensure_open("_echo_agent/event")
        return self.client.events_for(self.stream.id, self.stream)

    async def get(self) -> Any:
        self._ensure_open("echo_agent/run/get")
        return await self.client.request(
            "echo_agent/run/get", {"run": self.wire.to_dict()}
        )

    async def status(self) -> str:
        """Return the settled receipt status from the canonical source route."""

        operation = "echo_orchestration::runtime::turn_driver::TurnReceipt::status"
        self._ensure_open(operation)
        return cast(str, await self.client.call(operation, self.wire))

    async def outcome_status(self) -> str:
        """Return the settled outcome status from the canonical source route."""

        operation = "echo_orchestration::runtime::turn_driver::TurnOutcome::status"
        self._ensure_open(operation)
        return cast(str, await self.client.call(operation, self.wire))

    async def usage(self) -> ExecutionUsage:
        """Return lossless receipt usage counters without numeric conversion."""

        operation = "echo_orchestration::runtime::turn_driver::TurnReceipt::usage"
        self._ensure_open(operation)
        return cast(ExecutionUsage, await self.client.call(operation, self.wire))

    async def wait(
        self,
        timeout: float | timedelta | None = None,
    ) -> Any:
        self._ensure_open("echo_agent/run/wait")
        params: dict[str, Any] = {"run": self.wire.to_dict()}
        if timeout is not None:
            params["timeout"] = _timeout_wire(timeout)
        return await self.client.request("echo_agent/run/wait", params)

    async def cancel(self) -> Any:
        self._ensure_open("echo_agent/run/cancel")
        return await self.client.request(
            "echo_agent/run/cancel", {"run": self.wire.to_dict()}
        )

    async def steer(self, text: str) -> Any:
        self._ensure_open("echo_agent/run/steer")
        return await self.client.request(
            "echo_agent/run/steer", {"run": self.wire.to_dict(), "text": text}
        )

    async def replay(self, after_sequence: str = "0", max_events: int = 512) -> Any:
        self._ensure_open("echo_agent/run/replay")
        return await self.client.request(
            "echo_agent/run/replay",
            {
                "stream": self.stream.to_dict(),
                "after_sequence": after_sequence,
                "max_events": str(max_events),
            },
        )

    async def close(self) -> None:
        """Cancel the run and release this SDK-side event subscription.

        The Host owns Run records and releases them with Session close; there
        is no separate run/close wire method. Closing a Run therefore issues
        the canonical cancellation once and closes only the local mailbox.
        """

        if self._closed:
            return
        try:
            await self.client.request(
                "echo_agent/run/cancel", {"run": self.wire.to_dict()}
            )
            settled = await self.wait(timeout=5.0)
            if not bool(settled.get("settled")):
                raise EchoAgentError(
                    "cancellation_timeout",
                    "run did not settle before close timeout",
                    "after_delay",
                    "echo_agent/run/wait",
                )
        except EchoAgentError as error:
            if error.code not in {
                "closed_handle",
                "stale_handle",
                "host_exited",
                "host_shutting_down",
            }:
                raise
        finally:
            self._mark_closed()

    def _mark_closed(self) -> None:
        if self._closed:
            return
        self._closed = True
        feed = self.client._events.get(self.stream.id)
        if feed is not None:
            feed.queue.close()

    def _ensure_open(self, operation: str) -> None:
        if self._closed:
            raise EchoAgentError(
                "closed_handle", "Run handle is closed", operation=operation
            )

    async def __aenter__(self) -> Self:
        self._ensure_open("run/enter")
        return self

    async def __aexit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        tb: TracebackType | None,
    ) -> None:
        await self.close()


def _timeout_wire(timeout: float | timedelta) -> dict[str, Any]:
    if isinstance(timeout, timedelta):
        seconds = timeout.total_seconds()
    elif isinstance(timeout, bool) or not isinstance(timeout, (int, float)):
        raise TypeError("run wait timeout must be seconds or timedelta")
    else:
        seconds = float(timeout)
    if not math.isfinite(seconds) or seconds < 0:
        raise ValueError("run wait timeout must be finite and non-negative")
    whole = math.floor(seconds)
    nanos = round((seconds - whole) * 1_000_000_000)
    if nanos >= 1_000_000_000:
        whole += 1
        nanos = 0
    return {"seconds": str(whole), "nanos": nanos}
