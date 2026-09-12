import json
from pathlib import Path

from echo_agent_sdk import AgentSteerPhase, AgentSteerState, AgentSteerTurnOutcome


def test_steering_values_preserve_lifecycle_and_terminal_semantics() -> None:
    accepted = AgentSteerState.accepted()
    drained = AgentSteerState.drained_state()
    settled = AgentSteerState.turn_settled(AgentSteerTurnOutcome.COMPLETED, True)
    assert accepted.phase() is AgentSteerPhase.ACCEPTED
    assert not accepted.was_drained()
    assert drained.phase() is AgentSteerPhase.DRAINED
    assert drained.was_drained()
    assert settled.kind is AgentSteerPhase.TURN_SETTLED
    assert settled.outcome is AgentSteerTurnOutcome.COMPLETED
    assert settled.was_drained()
    assert AgentSteerTurnOutcome.FAILED.as_str() == "failed"
    assert AgentSteerTurnOutcome.parse("cancelled") is AgentSteerTurnOutcome.CANCELLED
    assert AgentSteerTurnOutcome.parse("unknown") is None


def test_steering_value_mappings_are_complete() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith("/steering_values")
    ]
    assert len(entries) == 13
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
