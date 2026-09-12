import json
from pathlib import Path

import pytest

from echo_agent_sdk import HookAction


def test_hook_actions_preserve_tagged_values_and_validation() -> None:
    command = HookAction("command", command="echo ok")
    assert command.kind() == "command"
    command.validate()
    HookAction("permission", decision="ask").validate()
    HookAction("http", url="http://localhost:8080/hook").validate()
    with pytest.raises(ValueError):
        HookAction("permission", decision="maybe").validate()


def test_hook_action_mappings_are_complete() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith(
            "/hook_action_values"
        )
    ]
    assert len(entries) == 30
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
