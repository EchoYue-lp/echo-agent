import json
from pathlib import Path

import pytest

from echo_agent_sdk import RuleMatcher


def test_permission_rule_matchers_preserve_parse_display_matching() -> None:
    pattern = RuleMatcher.parse("pattern:Bash(rm:*)")
    assert pattern.matches("Bash(rm:rf)", [])
    assert RuleMatcher.parse("pattern:*").matches("anything", [])
    assert RuleMatcher.parse("pattern:Bash(rm:?f)").matches("Bash(rm:rf)", [])
    assert RuleMatcher.parse("pattern:Bash(*:*)").matches("Bash(git:status)", [])
    assert pattern.matches_matcher_str("Bash(rm:*)")
    assert RuleMatcher.parse("perm:read").display() == "permission:read"
    with pytest.raises(ValueError, match="unsupported permission matcher"):
        RuleMatcher.parse("unknown")
    with pytest.raises(TypeError, match="permission matcher must be text"):
        RuleMatcher.parse(None)  # type: ignore[arg-type]


def test_permission_rule_matcher_mappings_are_complete() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith(
            "/permission_rule_matcher"
        )
    ]
    assert len(entries) == 9
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
