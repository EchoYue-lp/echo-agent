import json
from pathlib import Path

import pytest

from echo_agent_sdk import SubagentCommandIdentity


def test_subagent_command_identities_preserve_validation_and_attempt_projection() -> (
    None
):
    identity = SubagentCommandIdentity("run", "task", "exec", 2, 0, "command")
    assert identity.attempt_identity().execution_id == "exec"
    with pytest.raises(ValueError):
        SubagentCommandIdentity("", "task", "exec", 2, 0, "command")
    with pytest.raises(ValueError):
        SubagentCommandIdentity("run", "task", "exec", 0, 0, "command")


def test_subagent_command_identity_mappings_are_complete() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith(
            "/subagent_command_identity_values"
        )
    ]
    assert len(entries) == 6
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
