import json
from pathlib import Path

import pytest

from echo_agent_sdk import RetryPolicy


def test_retry_policy_preserves_exponential_backoff() -> None:
    policy = RetryPolicy.new(5, 100).max_delay(800).jitter(False)
    assert policy.delay_for(0) == 0
    assert policy.delay_for(1) == 100
    assert policy.delay_for(2) == 200
    assert policy.delay_for(4) == 800
    assert RetryPolicy.no_retry().delay_for(1) == 0
    with pytest.raises(ValueError):
        RetryPolicy.new(-1, 100)


def test_retry_policy_mappings_are_complete() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith(
            "/retry_policy_values"
        )
    ]
    assert len(entries) == 9
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
