import json
from pathlib import Path

import pytest

from echo_agent_sdk import EventId, EventIdentity, StreamId


def test_event_identities_preserve_validation_and_immutable_updates() -> None:
    assert EventId.new("evt-1").as_str() == "evt-1"
    assert str(StreamId.new("stream-1")) == "stream-1"
    with pytest.raises(ValueError):
        EventId.new("  ")
    identity = (
        EventIdentity.new("stream-1", "turn-1")
        .with_run_id("run-1")
        .with_message_id("message-1")
        .with_execution_id("exec-1")
        .with_conversation_id("conversation-1")
        .with_parent_event_id("event-0")
    )
    assert identity.stream_id.as_str() == "stream-1"
    assert identity.turn_id == "turn-1"
    assert identity.parent_event_id == "event-0"
    assert EventIdentity.for_run("run-2").execution_id == "run-2"
    assert (
        EventIdentity.for_chat(
            "conversation-2", "turn-2", "message-2", "run-2"
        ).message_id
        == "message-2"
    )
    assert (
        EventIdentity.from_runtime_context(
            {"run_id": "run-3", "execution_id": "exec-3"}
        ).turn_id
        == "exec-3"
    )
    assert (
        EventIdentity.from_invocation(
            {"runtime": {"conversation_id": "conversation-3"}}
        ).conversation_id
        == "conversation-3"
    )


def test_event_identity_mappings_are_complete() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith(
            "/event_identity_values"
        )
    ]
    assert len(entries) == 29
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
