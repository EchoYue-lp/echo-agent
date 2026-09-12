import json
from pathlib import Path

import pytest

from echo_agent_sdk import InterventionResult


def test_intervention_result_factories_preserve_local_decisions() -> None:
    assert InterventionResult.allow() == InterventionResult()
    assert InterventionResult.block_with_reason("reason").block_reason == "reason"
    assert InterventionResult.inject("context").injected_context == "context"
    assert InterventionResult.cancelled().cancel
    assert InterventionResult.modify_args({"key": "value"}).modified_args == {
        "key": "value"
    }
    with pytest.raises(ValueError):
        InterventionResult.block_with_reason("  ")


def test_intervention_result_mappings_are_complete() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith(
            "/intervention_values"
        )
    ]
    assert len(entries) == 7
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
