import json
from pathlib import Path

from echo_agent_sdk import ContextInheritance


def test_context_inheritance_preserves_defaults() -> None:
    assert ContextInheritance.fresh_default().inherit_history is None
    assert ContextInheritance.fork_default().inherit_memory
    assert ContextInheritance.for_mode("team").inherit_tools == ()


def test_context_inheritance_mappings_are_complete() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith(
            "/context_inheritance_values"
        )
    ]
    assert len(entries) == 11
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
