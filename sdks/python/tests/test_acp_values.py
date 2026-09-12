import json
from pathlib import Path

import pytest

from echo_agent_sdk import AcpLedgerLimits, ConnectionMode, ExtensionSettlement


def test_acp_runtime_values_preserve_settlement_and_ledger_limits() -> None:
    assert ConnectionMode.STANDARD.as_str() == "standard"
    assert ExtensionSettlement.TIMED_OUT.as_str() == "timed_out"
    assert ExtensionSettlement.ANSWERED.is_answered()
    assert not ExtensionSettlement.CANCELLED.is_answered()
    assert AcpLedgerLimits.default() == AcpLedgerLimits(10_000, 8 * 1024 * 1024)
    assert AcpLedgerLimits(2, 3).max_bytes == 3
    with pytest.raises(ValueError):
        AcpLedgerLimits(-1, 1)


def test_acp_runtime_value_mappings_are_complete() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith("/acp_values")
    ]
    assert len(entries) == 13
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
