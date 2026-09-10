import pytest

from echo_agent_sdk import EchoAgentClient


class StubClient(EchoAgentClient):
    def __init__(self) -> None:
        self.calls: list[tuple[str, object, tuple[object, ...]]] = []

    async def invoke(
        self,
        operation: str,
        handle: object,
        arguments: list[object],
    ) -> object:
        self.calls.append((operation, handle, tuple(arguments)))
        return arguments[0]


@pytest.mark.asyncio
async def test_classify_turn_outcome_delegates_raw_variant_and_null_wire_values() -> (
    None
):
    client = StubClient()
    variant = {
        "kind": "variant",
        "value": {
            "type_id": "echo_sdk_protocol::methods::AgentEventWire",
            "variant": "token",
            "fields": [],
        },
    }
    null = {"kind": "null"}

    assert await client.classify_turn_outcome(variant) is variant
    assert await client.classify_turn_outcome(null) is null
    assert client.calls == [
        (
            "echo_orchestration::runtime::turn_driver::TurnOutcome::classify",
            None,
            (variant,),
        ),
        (
            "echo_orchestration::runtime::turn_driver::TurnOutcome::classify",
            None,
            (null,),
        ),
    ]


@pytest.mark.asyncio
async def test_classify_turn_outcome_requires_a_mapping() -> None:
    client = StubClient()

    with pytest.raises(TypeError):
        await client.classify_turn_outcome("not an event")  # type: ignore[arg-type]
