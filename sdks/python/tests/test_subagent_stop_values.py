import json
from pathlib import Path

from echo_agent_sdk import SubagentStopStatus


def test_subagent_stop_statuses_preserve_stable_hook_spellings() -> None:
    assert SubagentStopStatus.TIMED_OUT.as_str() == "timed_out"
    assert SubagentStopStatus.COMPLETED.value == "completed"


def test_subagent_stop_value_mappings_are_complete() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith(
            "/subagent_stop_values"
        )
    ]
    assert len(entries) == 6
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
