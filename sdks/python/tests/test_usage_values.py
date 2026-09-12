import json
from pathlib import Path

import pytest

from echo_agent_sdk import TokenUsageDetails, Usage


def test_usage_preserves_cache_priority_and_effective_totals() -> None:
    usage = Usage(prompt_tokens=100, completion_tokens=20, cache_read_input_tokens=30)
    assert usage.cached_prompt_tokens() == 30
    assert usage.effective_prompt_tokens() == 130
    assert usage.effective_total_tokens() == 150
    assert usage.cache_hit_rate() == 30 / 130
    assert (
        Usage(
            prompt_tokens_details=TokenUsageDetails(cached_tokens=4),
            cache_read_input_tokens=9,
        ).cached_prompt_tokens()
        == 4
    )
    with pytest.raises(ValueError):
        Usage(prompt_tokens=1 << 32)


def test_usage_mappings_are_complete() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith("/usage_values")
    ]
    assert len(entries) == 6
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
