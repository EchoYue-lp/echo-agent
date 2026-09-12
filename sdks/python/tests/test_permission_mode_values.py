import json
from pathlib import Path

import pytest

from echo_agent_sdk import PermissionMode


def test_permission_modes_preserve_aliases_and_helper_semantics() -> None:
    assert PermissionMode.parse("autoedit") is PermissionMode.ACCEPT_EDITS
    assert PermissionMode.parse("ask") is PermissionMode.DEFAULT
    assert PermissionMode.ACCEPT_EDITS.allows_write()
    assert PermissionMode.STRICT_CONFIRM.requires_interaction()
    assert PermissionMode.AUTO.uses_classifier()
    with pytest.raises(ValueError, match="invalid permission mode"):
        PermissionMode.parse("unknown")


def test_permission_mode_helper_mappings_are_complete() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith(
            "/permission_mode_values"
        )
    ]
    assert len(entries) == 6
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
