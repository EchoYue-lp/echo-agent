import json
from pathlib import Path

from echo_agent_sdk import CommandCellArtifactStatus, CommandCellTerminalCause


def test_command_cell_terminal_artifact_values_preserve_spellings() -> None:
    assert (
        CommandCellTerminalCause.OUTPUT_DRAIN_FAILED.as_str() == "output_drain_failed"
    )
    assert CommandCellArtifactStatus.BELOW_THRESHOLD.as_str() == "below_threshold"


def test_command_cell_terminal_artifact_mappings_are_complete() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith(
            "/command_cell_status_values"
        )
    ]
    assert len(entries) == 15
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
