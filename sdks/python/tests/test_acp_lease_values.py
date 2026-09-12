import json
from pathlib import Path

from echo_agent_sdk import ExtensionLeaseError


def test_acp_lease_errors_preserve_display_text() -> None:
    assert (
        ExtensionLeaseError.ADMISSION_CLOSED.as_str() == "extension admission is closed"
    )
    assert ExtensionLeaseError.EXCLUSIVE_CONFLICT.as_str() == (
        "extension is already executing an exclusive invocation"
    )


def test_acp_lease_error_mappings_are_complete() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith("/acp_lease_values")
    ]
    assert len(entries) == 5
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
