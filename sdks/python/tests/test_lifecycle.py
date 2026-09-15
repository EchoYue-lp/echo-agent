import asyncio
from datetime import timedelta

import pytest

from echo_agent_sdk import WireHandle
from echo_agent_sdk.client import (
    AgentComponentCall,
    AgentComponentDescriptor,
    AgentComponentOutcome,
    AuditLoggerResult,
    AuditLogRequest,
    CompressionCall,
    CompressionOutcome,
    ContextCompressorDescriptor,
    CriticCall,
    CriticDescriptor,
    CritiqueOutcome,
    EchoAgentClient,
    EmbedderEmbedRequest,
    EmbedderResult,
    ExtensionErrorOutcome,
    ExtensionResultOutcome,
    ExtensionStreamWriter,
    LlmChatCall,
    LlmClientDescriptor,
    RunHandle,
    SandboxEmptyRequest,
    SkillLoadAllowsRequest,
    SkillLoadPolicyResult,
    StoreCall,
    StoreDescriptor,
    ToolCall,
    ToolDescriptor,
    WorkflowCheckpointClaimAttemptRequest,
    WorkflowCheckpointResult,
    WorkflowCheckpointSaveIfGenerationRequest,
    WorkflowRunRequest,
    _AsyncQueue,
    _Callbacks,
    _EventFeed,
    _timeout_wire,
    _typed_handler,
)
from echo_agent_sdk.errors import EchoAgentError


@pytest.mark.asyncio
async def test_bounded_subscription_surfaces_event_gap() -> None:
    queue = _AsyncQueue(maxsize=1)
    queue.push({"sequence": "1"})
    queue.push({"sequence": "2"})

    assert await queue.get() == {"sequence": "1"}
    with pytest.raises(EchoAgentError) as raised:
        await queue.get()
    assert raised.value.code == "event_gap"
    assert raised.value.details == {"reason": "client_queue_full", "capacity": 1}


def test_event_feed_rejects_stale_generation_and_malformed_gap_without_advancing() -> (
    None
):
    feed = _EventFeed(_AsyncQueue(maxsize=4))
    stream = WireHandle("stream-1", "1", "stream")
    event = {
        "stream": stream.to_dict(),
        "envelope": {"stream_id": stream.id, "sequence": "1"},
    }
    assert feed.accept_event(event)
    assert feed.last_sequence == 1

    forward_gap = dict(event)
    forward_gap["envelope"] = {"stream_id": stream.id, "sequence": "3"}
    with pytest.raises(EchoAgentError) as raised:
        feed.accept_event(forward_gap)
    assert raised.value.code == "event_gap"
    assert feed.last_sequence == 1

    stale = dict(event)
    stale["stream"] = WireHandle("stream-1", "2", "stream").to_dict()
    with pytest.raises(EchoAgentError) as raised:
        feed.accept_event(stale)
    assert raised.value.code == "handle_mismatch"
    assert feed.last_sequence == 1

    invalid_gap = {
        "stream": stream.to_dict(),
        "gap": {
            "from_sequence": "2",
            "to_sequence": "3",
            "reason": "retention floor",
            "snapshot_watermark": "2",
        },
    }
    with pytest.raises(EchoAgentError):
        feed.accept_gap(invalid_gap)
    assert feed.last_sequence == 1

    wrong_start = {
        "stream": stream.to_dict(),
        "gap": {
            "from_sequence": "3",
            "to_sequence": "3",
            "reason": "retention floor",
            "snapshot_watermark": "3",
        },
    }
    with pytest.raises(EchoAgentError):
        feed.accept_gap(wrong_start)
    assert feed.last_sequence == 1


def test_event_feed_rejects_stale_and_malformed_gap_handles_without_advancing() -> None:
    stream = WireHandle("stream-1", "1", "stream")
    valid_event = {
        "stream": stream.to_dict(),
        "envelope": {"stream_id": stream.id, "sequence": "1"},
    }
    gap = {
        "from_sequence": "2",
        "to_sequence": "2",
        "reason": "retention floor",
        "snapshot_watermark": "2",
    }
    invalid_streams = [
        WireHandle("stream-1", "2", "stream").to_dict(),
        WireHandle("stream-1", "1", "run").to_dict(),
        {"id": "stream-1", "generation": "01", "kind": "stream"},
    ]

    for invalid_stream in invalid_streams:
        feed = _EventFeed(_AsyncQueue(maxsize=4))
        assert feed.accept_event(valid_event)
        with pytest.raises(EchoAgentError):
            feed.accept_gap({"stream": invalid_stream, "gap": gap})
        assert feed.last_sequence == 1


def test_event_feed_accepts_contiguous_gap_and_next_event() -> None:
    feed = _EventFeed(_AsyncQueue(maxsize=4))
    stream = WireHandle("stream-1", "1", "stream")
    assert feed.accept_event(
        {
            "stream": stream.to_dict(),
            "envelope": {"stream_id": stream.id, "sequence": "1"},
        }
    )
    gap = {
        "stream": stream.to_dict(),
        "gap": {
            "from_sequence": "2",
            "to_sequence": "3",
            "reason": "retention floor",
            "snapshot_watermark": "3",
        },
    }
    assert feed.accept_gap(gap)
    assert not feed.accept_gap(gap)
    assert feed.accept_event(
        {
            "stream": stream.to_dict(),
            "envelope": {"stream_id": stream.id, "sequence": "4"},
        }
    )
    assert feed.last_sequence == 4


@pytest.mark.asyncio
async def test_current_subscription_discards_buffered_stale_generation_items() -> None:
    stale = WireHandle("stream-1", "1", "stream")
    current = WireHandle("stream-1", "2", "stream")
    feed = _EventFeed(_AsyncQueue(maxsize=4), handle=stale)
    feed.queue.push(
        {
            "stream": stale.to_dict(),
            "envelope": {"stream_id": stale.id, "sequence": "1"},
        }
    )
    client = object.__new__(EchoAgentClient)
    client._events = {stale.id: feed}
    client._event_queue_size = 4
    client._closed = False

    events = client.events_for(current.id, current)
    with pytest.raises(EchoAgentError) as raised:
        await anext(events)

    assert raised.value.code == "handle_mismatch"
    assert not feed.queue._items


@pytest.mark.asyncio
async def test_current_subscription_discards_stale_items_from_an_already_failed_feed() -> (
    None
):
    stale = WireHandle("stream-1", "1", "stream")
    current = WireHandle("stream-1", "2", "stream")
    feed = _EventFeed(_AsyncQueue(maxsize=4), handle=stale)
    feed.queue.push(
        {
            "stream": stale.to_dict(),
            "envelope": {"stream_id": stale.id, "sequence": "1"},
        }
    )
    feed.queue.fail(EchoAgentError("event_gap", "earlier feed failure"))
    client = object.__new__(EchoAgentClient)
    client._events = {stale.id: feed}
    client._event_queue_size = 4
    client._closed = False

    events = client.events_for(current.id, current)
    with pytest.raises(EchoAgentError) as raised:
        await anext(events)

    assert raised.value.code == "event_gap"
    assert not feed.queue._items


def test_event_feed_rejects_wrong_kind_without_advancing() -> None:
    feed = _EventFeed(_AsyncQueue(maxsize=4))
    with pytest.raises(EchoAgentError) as raised:
        feed.accept_event(
            {
                "stream": WireHandle("stream-1", "1", "run").to_dict(),
                "envelope": {"stream_id": "stream-1", "sequence": "1"},
            }
        )

    assert raised.value.code == "serialization_violation"
    assert feed.handle is None
    assert feed.last_sequence == 0


@pytest.mark.asyncio
async def test_cancelled_subscription_waiter_is_removed_without_losing_value() -> None:
    queue = _AsyncQueue(maxsize=1)
    waiter = asyncio.create_task(queue.get())
    await asyncio.sleep(0)
    waiter.cancel()
    with pytest.raises(asyncio.CancelledError):
        await waiter

    queue.push("retained")
    assert await queue.get() == "retained"


def test_run_wait_timeout_is_lossless_and_bounded() -> None:
    assert _timeout_wire(1.25) == {"seconds": "1", "nanos": 250_000_000}
    assert _timeout_wire(timedelta(seconds=2, microseconds=1)) == {
        "seconds": "2",
        "nanos": 1_000,
    }
    with pytest.raises(ValueError):
        _timeout_wire(-1)
    with pytest.raises(ValueError):
        _timeout_wire(float("inf"))


@pytest.mark.asyncio
async def test_session_update_preserves_standard_session_identity() -> None:
    updates: dict[str, _AsyncQueue] = {}
    callbacks = _Callbacks(updates, 2)

    await callbacks.session_update("session-1", {"kind": "message"})

    assert await updates["session-1"].get() == {
        "sessionId": "session-1",
        "update": {"kind": "message"},
    }


@pytest.mark.asyncio
async def test_event_consumption_acknowledges_highest_contiguous_sequence() -> None:
    sent: list[tuple[str, object]] = []

    class StubClient:
        def __init__(self) -> None:
            self._event_cursors: dict[WireHandle, int] = {}

        async def notify(self, method: str, params: object) -> None:
            sent.append((method, params))

    client = StubClient()
    stream = WireHandle("stream-1", "1", "stream")
    await EchoAgentClient._ack_event(
        client,  # type: ignore[arg-type]
        stream,
        {"stream": stream.to_dict(), "envelope": {"sequence": "1"}},
    )
    await EchoAgentClient._ack_event(
        client,  # type: ignore[arg-type]
        stream,
        {"stream": stream.to_dict(), "envelope": {"sequence": "1"}},
    )

    assert sent == [
        (
            "echo_agent/event/ack",
            {
                "ack": {
                    "stream": stream.to_dict(),
                    "last_processed_sequence": "1",
                }
            },
        )
    ]


@pytest.mark.asyncio
async def test_event_ack_does_not_advance_for_a_different_generation() -> None:
    sent: list[tuple[str, object]] = []

    class StubClient:
        def __init__(self) -> None:
            self._event_cursors: dict[WireHandle, int] = {}

        async def notify(self, method: str, params: object) -> None:
            sent.append((method, params))

    client = StubClient()
    stream = WireHandle("stream-1", "1", "stream")
    stale = WireHandle("stream-1", "2", "stream")
    await EchoAgentClient._ack_event(
        client,  # type: ignore[arg-type]
        stream,
        {"stream": stale.to_dict(), "envelope": {"sequence": "9"}},
    )

    assert sent == []
    assert client._event_cursors == {}


@pytest.mark.asyncio
async def test_event_ack_advances_cursor_only_after_notification_succeeds() -> None:
    class StubClient:
        def __init__(self) -> None:
            self._event_cursors: dict[WireHandle, int] = {}

        async def notify(self, _method: str, _params: object) -> None:
            raise EchoAgentError("transport_error", "ack transport failed")

    client = StubClient()
    stream = WireHandle("stream-1", "1", "stream")
    with pytest.raises(EchoAgentError):
        await EchoAgentClient._ack_event(
            client,  # type: ignore[arg-type]
            stream,
            {"stream": stream.to_dict(), "envelope": {"sequence": "2"}},
        )

    assert client._event_cursors == {}


@pytest.mark.asyncio
async def test_run_close_cancels_once_and_closes_local_events() -> None:
    class StubClient:
        def __init__(self) -> None:
            self._events = {"stream-1": _EventFeed(_AsyncQueue())}
            self.requests: list[tuple[str, object]] = []

        async def request(self, method: str, params: object) -> dict[str, object]:
            self.requests.append((method, params))
            if method.endswith("run/cancel"):
                return {"cancellation_initiated": True}
            return {"settled": True}

    client = StubClient()
    run = RunHandle(
        client, WireHandle("run-1", "1", "run"), WireHandle("stream-1", "1", "stream")
    )
    await run.close()
    await run.close()

    assert [method for method, _ in client.requests] == [
        "echo_agent/run/cancel",
        "echo_agent/run/wait",
    ]
    with pytest.raises(StopAsyncIteration):
        await client._events["stream-1"].queue.get()


def test_typed_descriptors_render_the_frozen_extension_shapes() -> None:
    tool = ToolDescriptor(
        name="search",
        description="Search documents",
        parameters={
            "kind": "record",
            "value": {"type_id": "json_schema", "fields": []},
        },
        schema_revision="4",
        required_input_modalities=("text",),
        required_permissions=("read",),
    )
    assert tool.to_wire() == {
        "kind": "tool",
        "descriptor_version": 1,
        "name": "search",
        "description": "Search documents",
        "parameters": {
            "kind": "record",
            "value": {"type_id": "json_schema", "fields": []},
        },
        "schema_revision": "4",
        "required_input_modalities": ["text"],
        "required_permissions": ["read"],
        "risk_level": "standard",
        "supports_streaming": False,
        "exempt_from_batch_timeout": False,
        "allows_parallel_batch_execution": True,
        "manages_own_timeout": False,
    }
    assert LlmClientDescriptor("model", True).to_wire() == {
        "kind": "llm_client",
        "descriptor_version": 1,
        "model_name": "model",
        "supports_streaming": True,
    }
    assert StoreDescriptor(("keyword", "semantic")).to_wire() == {
        "kind": "store",
        "descriptor_version": 1,
        "search_modes": ["keyword", "semantic"],
    }
    assert CriticDescriptor("quality-reviewer").to_wire() == {
        "kind": "critic",
        "descriptor_version": 1,
        "name": "quality-reviewer",
    }
    assert ContextCompressorDescriptor("python-compressor").to_wire() == {
        "kind": "context_compressor",
        "descriptor_version": 1,
        "name": "python-compressor",
    }
    assert AgentComponentDescriptor("audit_logger", "python-audit").to_wire() == {
        "kind": "agent_component",
        "descriptor_version": 1,
        "component": "audit_logger",
        "name": "python-audit",
        "capabilities": {
            "isolation_level": None,
            "supports_streaming": False,
            "supports_notifications": False,
        },
    }
    assert (
        AgentComponentDescriptor(
            "workflow", "python-workflow", supports_streaming=True
        ).to_wire()["capabilities"]["supports_streaming"]
        is True
    )
    with pytest.raises(ValueError, match="streaming is only valid"):
        AgentComponentDescriptor(
            "audit_logger", "invalid-stream", supports_streaming=True
        )


def test_typed_call_dataclasses_decode_host_invocations() -> None:
    payload = {
        "extension": {"id": "ext-1", "generation": "2", "kind": "extension"},
        "invocation_id": "call-1",
        "deadline": {"seconds": "20", "nanos": 0},
        "invocation": {
            "operation": "tool_execute",
            "input": {
                "parameters": {
                    "kind": "map",
                    "value": [
                        {
                            "key": {"kind": "string", "value": "query"},
                            "value": {"kind": "string", "value": "rust"},
                        }
                    ],
                },
                "context": {"conversation_id": "session-1"},
            },
        },
    }
    tool_call = ToolCall.from_payload(payload)
    assert tool_call.parameters == {"query": "rust"}
    assert tool_call.context == {"conversation_id": "session-1"}
    assert tool_call.extension == WireHandle("ext-1", "2", "extension")

    llm_call = LlmChatCall.from_payload(
        {
            **payload,
            "invocation": {
                "operation": "llm_chat",
                "input": {"messages": [{"role": "user", "content": "hello"}]},
            },
        }
    )
    assert llm_call.messages == ({"role": "user", "content": "hello"},)

    store_call = StoreCall.from_payload(
        {
            **payload,
            "invocation": {
                "operation": "store_put",
                "input": {
                    "namespace": ["memory"],
                    "key": "k",
                    "value": {"kind": "string", "value": "v"},
                },
            },
        }
    )
    assert store_call.namespace == ("memory",)
    assert store_call.key == "k"
    assert store_call.value == "v"

    critic_call = CriticCall.from_payload(
        {
            **payload,
            "invocation": {
                "operation": "critic_critique",
                "input": {
                    "task": "solve",
                    "answer": "42",
                    "context": "math",
                },
            },
        }
    )
    assert (critic_call.task, critic_call.answer, critic_call.context) == (
        "solve",
        "42",
        "math",
    )
    assert CritiqueOutcome(9.5, True, "correct", ("show the steps",)).to_wire() == {
        "outcome": "result",
        "result": {
            "operation": "critic_critique",
            "value": {
                "score": 9.5,
                "passed": True,
                "feedback": "correct",
                "suggestions": ["show the steps"],
            },
        },
    }


@pytest.mark.asyncio
async def test_typed_handler_returns_host_outcome_without_local_execution() -> None:
    received: list[ToolCall] = []

    class ToolImpl:
        async def execute(self, call: ToolCall, _cancellation: asyncio.Event):
            received.append(call)
            return ExtensionResultOutcome(
                call.operation,
                {
                    "kind": "text",
                    "success": True,
                    "output": "ok",
                    "truncated": False,
                },
            )

    handler = _typed_handler("tool", ToolImpl())
    payload = {
        "extension": {"id": "ext-1", "generation": "2", "kind": "extension"},
        "invocation_id": "call-1",
        "deadline": {"seconds": "20", "nanos": 0},
        "invocation": {
            "operation": "tool_execute",
            "input": {"parameters": {"kind": "string", "value": "query"}},
        },
    }
    outcome = await handler(payload, asyncio.Event())
    assert outcome["outcome"] == "result"
    assert outcome["result"]["operation"] == "tool_execute"
    assert received[0].parameters == "query"
    assert ExtensionErrorOutcome("extension_failed", "no").to_wire() == {
        "outcome": "error",
        "error": {
            "code": "extension_failed",
            "message": "no",
            "retryable": "never",
        },
    }


@pytest.mark.asyncio
async def test_typed_critic_handler_returns_structured_critique() -> None:
    received: list[CriticCall] = []

    class CriticImpl:
        async def critique(self, call: CriticCall, _cancellation: asyncio.Event):
            received.append(call)
            return CritiqueOutcome(8.0, True, "sound", ())

    handler = _typed_handler("critic", CriticImpl())
    outcome = await handler(
        {
            "extension": {
                "id": "ext-1",
                "generation": "2",
                "kind": "extension",
            },
            "invocation_id": "critic-call-1",
            "deadline": {"seconds": "20", "nanos": 0},
            "invocation": {
                "operation": "critic_critique",
                "input": {
                    "task": "solve",
                    "answer": "42",
                    "context": "math",
                },
            },
        },
        asyncio.Event(),
    )
    assert outcome == {
        "outcome": "result",
        "result": {
            "operation": "critic_critique",
            "value": {
                "score": 8.0,
                "passed": True,
                "feedback": "sound",
                "suggestions": [],
            },
        },
    }
    assert received[0].task == "solve"


@pytest.mark.asyncio
async def test_typed_context_compressor_preserves_input_and_output() -> None:
    received: list[CompressionCall] = []

    class CompressorImpl:
        async def compress(
            self, call: CompressionCall, _cancellation: asyncio.Event
        ) -> CompressionOutcome:
            received.append(call)
            return CompressionOutcome(call.messages[-1:], call.messages[:-1], None)

    handler = _typed_handler("context_compressor", CompressorImpl())
    outcome = await handler(
        {
            "extension": {
                "id": "compressor-1",
                "generation": "1",
                "kind": "extension",
            },
            "invocation_id": "compress-call-1",
            "deadline": {"seconds": "20", "nanos": 0},
            "invocation": {
                "operation": "compressor_compress",
                "input": {
                    "messages": [
                        {
                            "role": "user",
                            "content": {"kind": "string", "value": "hello"},
                        },
                        {
                            "role": "assistant",
                            "content": {"kind": "string", "value": "hi"},
                        },
                    ],
                    "token_limit": "4096",
                    "current_query": "summarize",
                    "focus_instructions": None,
                    "tokenizer": {
                        "resource": {
                            "id": "tokenizer-1",
                            "generation": "1",
                            "kind": "facade_resource",
                        },
                        "owner_session_id": "session-1",
                    },
                },
            },
        },
        asyncio.Event(),
    )
    assert outcome["result"]["operation"] == "compressor_compress"
    assert outcome["result"]["value"]["messages"][0]["role"] == "assistant"
    assert received[0].token_limit == 4096
    assert received[0].current_query == "summarize"


@pytest.mark.asyncio
async def test_typed_agent_component_preserves_nested_operation() -> None:
    received: list[AgentComponentCall] = []

    class ComponentImpl:
        async def call(
            self, call: AgentComponentCall, _cancellation: asyncio.Event
        ) -> AgentComponentOutcome:
            received.append(call)
            return AgentComponentOutcome(AuditLoggerResult.logged())

    handler = _typed_handler("agent_component", ComponentImpl())
    outcome = await handler(
        {
            "extension": {"id": "audit-1", "generation": "1", "kind": "extension"},
            "invocation_id": "component-call-1",
            "deadline": {"seconds": "20", "nanos": 0},
            "invocation": {
                "operation": "agent_component_call",
                "input": {
                    "component": "audit_logger",
                    "call": {
                        "operation": "audit_log",
                        "input": {"event": {"kind": "map", "value": []}},
                    },
                },
            },
        },
        asyncio.Event(),
    )
    assert outcome["result"]["value"]["result"]["operation"] == "audit_log"
    assert received[0].component == "audit_logger"
    assert isinstance(received[0].request, AuditLogRequest)


def test_agent_component_rejects_invalid_discriminated_input() -> None:
    payload = {
        "extension": {"id": "audit-1", "generation": "1", "kind": "extension"},
        "invocation_id": "component-invalid",
        "deadline": {"seconds": "20", "nanos": 0},
        "invocation": {
            "operation": "agent_component_call",
            "input": {
                "component": "audit_logger",
                "call": {"operation": "audit_log", "input": {}},
            },
        },
    }
    with pytest.raises(TypeError, match="event must be a WireValue"):
        AgentComponentCall.from_payload(payload)


def test_agent_component_extended_variants_and_unit_inputs_are_closed() -> None:
    unit_payload = {
        "extension": {"id": "sandbox-1", "generation": "1", "kind": "extension"},
        "invocation_id": "component-unit",
        "deadline": {"seconds": "20", "nanos": 0},
        "invocation": {
            "operation": "agent_component_call",
            "input": {
                "component": "sandbox_executor",
                "call": {"operation": "sandbox_is_available"},
            },
        },
    }
    unit = AgentComponentCall.from_payload(unit_payload)
    assert isinstance(unit.request, SandboxEmptyRequest)

    embed_payload = {
        **unit_payload,
        "invocation_id": "component-embed",
        "invocation": {
            "operation": "agent_component_call",
            "input": {
                "component": "embedder",
                "call": {
                    "operation": "embedder_embed",
                    "input": {"text": "hello"},
                },
            },
        },
    }
    embed = AgentComponentCall.from_payload(embed_payload)
    assert isinstance(embed.request, EmbedderEmbedRequest)
    outcome = AgentComponentOutcome(EmbedderResult.embedded([0.25, 0.75])).to_wire()
    assert outcome["result"]["value"]["result"]["value"]["vector"] == [0.25, 0.75]

    with pytest.raises(ValueError, match="kind does not match"):
        AgentComponentOutcome(
            type(
                "InvalidResult",
                (),
                {
                    "component": "run_store",
                    "operation": "embedder_embed",
                    "value": {"vector": [1.0]},
                },
            )()
        ).to_wire()


def test_workflow_checkpoint_generation_claim_and_heartbeat_contracts() -> None:
    descriptor = AgentComponentDescriptor(
        "workflow_checkpoint_store",
        "checkpoint-store",
        claim_heartbeat_interval_ms=1_000,
    ).to_wire()
    assert descriptor["capabilities"]["claim_heartbeat_interval_ms"] == "1000"
    with pytest.raises(ValueError, match="require a claim heartbeat"):
        AgentComponentDescriptor("workflow_checkpoint_store", "missing")
    with pytest.raises(ValueError, match="only valid"):
        AgentComponentDescriptor(
            "audit_logger", "wrong", claim_heartbeat_interval_ms=1_000
        )
    with pytest.raises(ValueError, match="1 to 300000"):
        AgentComponentDescriptor(
            "workflow_checkpoint_store",
            "too-large",
            claim_heartbeat_interval_ms=300_001,
        )

    payload = {
        "extension": {"id": "checkpoint-1", "generation": "1", "kind": "extension"},
        "invocation_id": "checkpoint-cas",
        "deadline": {"seconds": "20", "nanos": 0},
        "invocation": {
            "operation": "agent_component_call",
            "input": {
                "component": "workflow_checkpoint_store",
                "call": {
                    "operation": "workflow_checkpoint_save_if_generation",
                    "input": {
                        "checkpoint": {"kind": "map", "value": []},
                        "expected_generation": "7",
                    },
                },
            },
        },
    }
    save = AgentComponentCall.from_payload(payload)
    assert isinstance(save.request, WorkflowCheckpointSaveIfGenerationRequest)
    assert save.request.expected_generation == 7

    claim_payload = {
        **payload,
        "invocation_id": "checkpoint-ack",
        "invocation": {
            "operation": "agent_component_call",
            "input": {
                "component": "workflow_checkpoint_store",
                "call": {
                    "operation": "workflow_checkpoint_ack_claim",
                    "input": {
                        "checkpoint_id": "checkpoint-1",
                        "attempt_id": "attempt-1",
                    },
                },
            },
        },
    }
    claim = AgentComponentCall.from_payload(claim_payload)
    assert isinstance(claim.request, WorkflowCheckpointClaimAttemptRequest)
    assert claim.request.attempt_id == "attempt-1"

    committed = AgentComponentOutcome(
        WorkflowCheckpointResult.saved_if_generation(True)
    ).to_wire()
    assert committed["result"]["value"]["result"]["value"] == {"committed": True}
    acked = AgentComponentOutcome(WorkflowCheckpointResult.claim_acked()).to_wire()
    assert acked["result"]["value"]["result"]["operation"] == (
        "workflow_checkpoint_ack_claim"
    )


def test_agent_component_stream_preserves_outer_and_nested_discriminators() -> None:
    payload = {
        "extension": {"id": "workflow-1", "generation": "1", "kind": "extension"},
        "stream": {"id": "workflow-stream", "generation": "1", "kind": "stream"},
        "invocation_id": "component-stream",
        "deadline": {"seconds": "20", "nanos": 0},
        "invocation": {
            "operation": "agent_component_call_stream",
            "input": {
                "component": "workflow",
                "call": {
                    "operation": "workflow_run_stream",
                    "input": {"input": "start"},
                },
            },
        },
    }
    call = AgentComponentCall.from_payload(payload)
    assert call.operation == "agent_component_call_stream"
    assert isinstance(call.request, WorkflowRunRequest)
    assert call.request.operation == "workflow_run_stream"


@pytest.mark.asyncio
async def test_skill_policy_and_component_stream_terminals_are_typed() -> None:
    payload = {
        "extension": {"id": "policy-1", "generation": "1", "kind": "extension"},
        "invocation_id": "policy-call",
        "deadline": {"seconds": "20", "nanos": 0},
        "invocation": {
            "operation": "agent_component_call",
            "input": {
                "component": "skill_load_policy",
                "call": {
                    "operation": "skill_load_allows",
                    "input": {
                        "descriptor": {
                            "name": "review",
                            "description": "Review code",
                            "location": {
                                "encoding": "utf8",
                                "path": "/tmp/review/SKILL.md",
                            },
                            "license": None,
                            "compatibility": None,
                            "metadata": {},
                            "source": None,
                            "allowed_tools": [],
                            "shell": None,
                            "paths": [],
                            "triggers": [],
                            "hooks": None,
                            "sandbox": None,
                            "depends_on": [],
                        }
                    },
                },
            },
        },
    }
    call = AgentComponentCall.from_payload(payload)
    assert isinstance(call.request, SkillLoadAllowsRequest)
    assert AgentComponentOutcome(SkillLoadPolicyResult.allowed(False)).to_wire()[
        "result"
    ]["value"]["result"]["value"] == {"allowed": False}

    notifications = []

    class Client:
        async def notify(self, method: str, params: object) -> None:
            notifications.append((method, params))

    writer = ExtensionStreamWriter(Client(), WireHandle("stream-typed", "1", "stream"))
    await writer.agent_component_chunk(
        {
            "component": "sandbox",
            "event": {"event": "output", "channel": "stdout", "chunk": "ok"},
        }
    )
    await writer.agent_component_complete(
        {
            "component": "workflow",
            "terminal": {
                "result": "done",
                "total_steps": "1",
                "elapsed": {"seconds": "0", "nanos": 1},
            },
        }
    )
    assert notifications[0][1]["event"] == "chunk"
    assert notifications[1][1]["event"] == "complete"

    invalid_writer = ExtensionStreamWriter(
        Client(), WireHandle("stream-invalid", "1", "stream")
    )
    with pytest.raises(ValueError, match="non-terminal"):
        await invalid_writer.agent_component_chunk(
            {
                "component": "workflow",
                "event": {
                    "event": "completed",
                    "result": "invalid chunk",
                },
            }
        )
