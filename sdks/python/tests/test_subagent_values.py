import json
from pathlib import Path

import pytest

from echo_agent_sdk import SubagentCommandPhase, SubagentStatus


def test_subagent_values_preserve_stable_phase_and_status_semantics() -> None:
    assert SubagentCommandPhase.MAILBOX_ACCEPTED.as_str() == "mailbox_accepted"
    assert (
        SubagentCommandPhase.parse("turn_settled") is SubagentCommandPhase.TURN_SETTLED
    )
    assert SubagentCommandPhase.parse("unknown") is None
    assert SubagentStatus.TIMED_OUT.as_str() == "timed_out"
    assert SubagentStatus.parse("completed") is SubagentStatus.COMPLETED
    with pytest.raises(ValueError, match="unknown Subagent status"):
        SubagentStatus.parse("unknown")


def test_subagent_value_mappings_are_complete() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith("/subagent_values")
    ]
    assert len(entries) == 15
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
