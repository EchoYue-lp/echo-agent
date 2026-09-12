import json
from pathlib import Path

from echo_agent_sdk import execution_usage_duration_millis


def test_execution_usage_duration_defaults_to_zero() -> None:
    assert execution_usage_duration_millis({}) == 0
    assert execution_usage_duration_millis({"duration_ms": "42"}) == 42
    assert execution_usage_duration_millis({"duration_ms": True}) == 0


def test_execution_usage_mapping_is_complete() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith(
            "/execution_usage_values"
        )
    ]
    assert len(entries) == 1
    assert entries[0]["languages"]["python"]["status"] == "done"
