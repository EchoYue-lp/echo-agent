import json
from pathlib import Path

from echo_agent_sdk import TaskTerminalStatus


def test_task_terminal_statuses_preserve_stable_hook_spellings() -> None:
    assert TaskTerminalStatus.TIMED_OUT.as_str() == "timed_out"
    assert TaskTerminalStatus.SKIPPED.value == "skipped"


def test_task_terminal_value_mappings_are_complete() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith(
            "/task_terminal_values"
        )
    ]
    assert len(entries) == 7
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
