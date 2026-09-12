import json
from pathlib import Path

from echo_agent_sdk import TurnMode


def test_turn_modes_preserve_stable_stream_flavors() -> None:
    assert TurnMode.CHAT.as_str() == "chat"
    assert TurnMode.EXECUTE.as_str() == "execute"


def test_turn_mode_mappings_are_complete() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith("/turn_mode_values")
    ]
    assert len(entries) == 3
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
