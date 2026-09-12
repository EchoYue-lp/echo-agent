import json
from pathlib import Path

import pytest

from echo_agent_sdk import RuleBehavior, RuleBehaviorKind


def test_permission_rule_behavior_preserves_parse_defaults_and_decisions() -> None:
    assert RuleBehavior.parse("allow").to_decision().kind is RuleBehaviorKind.ALLOW
    deny = RuleBehavior.parse("deny").to_decision()
    assert deny.reason == "denied by rule"
    ask = RuleBehavior.parse("ask").to_decision()
    assert ask.suggestions == ("allow", "deny")
    with pytest.raises(ValueError, match="unknown permission rule behavior"):
        RuleBehavior.parse("unknown")


def test_permission_rule_behavior_mappings_are_complete() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith(
            "/permission_rule_behavior"
        )
    ]
    assert len(entries) == 6
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
