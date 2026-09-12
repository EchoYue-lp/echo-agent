import json
from pathlib import Path

from echo_agent_sdk import PromptDiagnostics


def test_prompt_diagnostics_preserve_record_and_count_semantics() -> None:
    diagnostics = PromptDiagnostics()
    diagnostics.record("system", "base")
    diagnostics.record("system", "overlay")
    assert diagnostics.count("system") == 2
    assert diagnostics.count("missing") == 0


def test_prompt_diagnostics_mappings_are_complete() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith(
            "/prompt_diagnostics_values"
        )
    ]
    assert len(entries) == 3
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
