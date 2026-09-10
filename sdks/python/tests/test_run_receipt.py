import pytest

from echo_agent_sdk import ExecutionUsage, RunUsage, WireHandle
from echo_agent_sdk.client import RunHandle
from echo_agent_sdk.errors import EchoAgentError


class StubClient:
    def __init__(self) -> None:
        self.calls: list[tuple[str, WireHandle, tuple[object, ...]]] = []
        self._events: dict[str, object] = {}

    async def call(
        self,
        operation: str,
        handle: WireHandle,
        arguments: tuple[object, ...] = (),
    ) -> object:
        self.calls.append((operation, handle, arguments))
        if operation.endswith("TurnReceipt::usage"):
            return {
                "duration_ms": "42",
                "tokens_used": "18446744073709551615",
                "iterations": None,
            }
        return "completed"


@pytest.mark.asyncio
async def test_run_receipt_helpers_use_canonical_routes_and_preserve_wire_u64() -> None:
    client = StubClient()
    run_wire = WireHandle("run-1", "7", "run")
    run = RunHandle(client, run_wire, WireHandle("stream-1", "3", "stream"))

    assert await run.status() == "completed"
    assert await run.outcome_status() == "completed"
    usage = await run.usage()

    assert usage == {
        "duration_ms": "42",
        "tokens_used": "18446744073709551615",
        "iterations": None,
    }
    assert RunUsage is ExecutionUsage
    assert [operation for operation, _, _ in client.calls] == [
        "echo_orchestration::runtime::turn_driver::TurnReceipt::status",
        "echo_orchestration::runtime::turn_driver::TurnOutcome::status",
        "echo_orchestration::runtime::turn_driver::TurnReceipt::usage",
    ]
    assert all(
        handle == run_wire and arguments == () for _, handle, arguments in client.calls
    )


@pytest.mark.asyncio
async def test_run_receipt_helpers_reject_a_closed_handle() -> None:
    client = StubClient()
    run = RunHandle(
        client,
        WireHandle("run-1", "7", "run"),
        WireHandle("stream-1", "3", "stream"),
    )
    run._mark_closed()

    with pytest.raises(EchoAgentError) as raised:
        await run.status()
    assert raised.value.code == "closed_handle"
