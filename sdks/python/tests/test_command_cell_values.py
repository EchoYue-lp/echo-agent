import json
from pathlib import Path

from echo_agent_sdk import CommandCellPhase


def test_command_cell_phases_preserve_stable_spelling_and_terminal_semantics() -> None:
    assert CommandCellPhase.LAUNCH_FAILED.as_str() == "launch_failed"
    assert not CommandCellPhase.RUNNING.is_terminal()
    assert CommandCellPhase.SUCCEEDED.is_terminal()


def test_command_cell_phase_mappings_are_complete() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith(
            "/command_cell_values"
        )
    ]
    assert len(entries) == 10
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
