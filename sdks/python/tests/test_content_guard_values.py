import json
from pathlib import Path

from echo_agent_sdk import ContentGuardResult


def test_content_guard_values_preserve_variant_payloads() -> None:
    assert ContentGuardResult.pass_result().kind.value == "pass"
    assert ContentGuardResult.detected(["email"]).pii_types == ("email",)
    assert ContentGuardResult.rejected(["phone"]).is_rejected()
    assert not ContentGuardResult.redacted("safe").is_rejected()


def test_content_guard_value_mappings_are_complete() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith(
            "/content_guard_values"
        )
    ]
    assert len(entries) == 6
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
