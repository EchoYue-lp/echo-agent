import json
from pathlib import Path

from echo_agent_sdk import DeliveryOutcome, DeliveryPhase


def test_delivery_values_preserve_stable_snake_case_spellings() -> None:
    assert DeliveryOutcome.OUTCOME_UNKNOWN.as_str() == "outcome_unknown"
    assert DeliveryPhase.EFFECT_STARTED.as_str() == "effect_started"
    assert DeliveryPhase.TURN_SETTLED.as_str() == "turn_settled"


def test_delivery_value_mappings_are_complete() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith("/delivery_values")
    ]
    assert len(entries) == 16
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
