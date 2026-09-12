import json
from pathlib import Path

from echo_agent_sdk import ResourceLimits


def test_sandbox_resource_limits_preserve_policies() -> None:
    assert ResourceLimits.default().cpu_time_secs == 30
    assert ResourceLimits.default().memory_bytes == 256 * 1024 * 1024
    assert ResourceLimits.strict().max_processes == 8
    assert ResourceLimits.unrestricted().network
    assert ResourceLimits.unrestricted().cpu_time_secs is None


def test_sandbox_resource_limit_mappings_are_complete() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith(
            "/sandbox_resource_values"
        )
    ]
    assert len(entries) == 4
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
