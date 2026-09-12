import json
from pathlib import Path

from echo_agent_sdk import LlmUsageStats


def test_subagent_usage_accumulates_and_emits_payload() -> None:
    stats = LlmUsageStats()
    stats.record("model", 100, 50, 150, 80, 10, True)
    stats.record("model", 200, 60, 260, 150, 20, False)
    payload = stats.to_payload("session")
    assert payload["prompt_tokens"] == 300
    assert payload["call_count"] == 2
    assert payload["usage_reported"] is True


def test_subagent_usage_mappings_are_complete() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith(
            "/subagent_usage_values"
        )
    ]
    assert len(entries) == 3
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
