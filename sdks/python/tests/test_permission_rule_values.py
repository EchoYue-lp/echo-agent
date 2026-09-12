import json
from pathlib import Path

import pytest

from echo_agent_sdk import RuleSource


def test_permission_rule_sources_preserve_canonical_and_alias_parsing() -> None:
    assert str(RuleSource.SESSION) == "session"
    assert RuleSource.parse("local_settings") is RuleSource.LOCAL_SETTINGS
    assert RuleSource.parse("manual") is RuleSource.USER_SETTINGS
    with pytest.raises(ValueError, match="unknown permission rule source"):
        RuleSource.parse("unknown")


def test_permission_rule_source_mappings_are_complete() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith(
            "/permission_rule_values"
        )
    ]
    assert len(entries) == 10
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
