import json
from pathlib import Path

from echo_agent_sdk import SubagentContext


def test_subagent_context_preserves_empty_and_content_semantics() -> None:
    assert not SubagentContext.empty().has_content()
    assert SubagentContext(parent_goal="goal").has_content()


def test_subagent_context_mappings_are_complete() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith(
            "/subagent_context_values"
        )
    ]
    assert len(entries) == 3
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
