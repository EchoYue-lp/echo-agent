import json
from pathlib import Path

from echo_agent_sdk import ObservedIsolation


def test_observed_isolation_preserves_trim_default_and_unicode_bounds() -> None:
    assert ObservedIsolation.new("  worktree  ").as_str() == "worktree"
    assert ObservedIsolation.new("   ").as_str() == "unknown"
    assert len(ObservedIsolation.new("😀" * 600).as_str()) == 512


def test_observed_isolation_mappings_are_complete() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith(
            "/observed_isolation_values"
        )
    ]
    assert len(entries) == 4
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
