import json
from pathlib import Path

from echo_agent_sdk import GuardDecision


def test_guard_decisions_preserve_variant_payloads() -> None:
    assert GuardDecision.pass_decision().kind.value == "pass"
    assert GuardDecision.block("unsafe").is_blocked()
    assert GuardDecision.warn(["one"]).reasons == ("one",)
    transformed = GuardDecision.transform("safe", ["redacted"])
    assert transformed.content == "safe"
    assert not transformed.is_blocked()


def test_guard_decision_mappings_are_complete() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith("/guard_values")
    ]
    assert len(entries) == 6
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
