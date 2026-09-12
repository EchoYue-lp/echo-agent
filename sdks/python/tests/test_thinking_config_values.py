import json
from pathlib import Path

import pytest

from echo_agent_sdk import ThinkingConfig, ThinkingLevel


def test_thinking_config_preserves_provider_projections() -> None:
    assert ThinkingConfig.medium().kind == "level"
    assert ThinkingConfig.parse_spec("auto") is None
    assert ThinkingConfig.parse_spec("4000").kind == "budget_tokens"
    assert ThinkingConfig.parse_spec("high").value is ThinkingLevel.HIGH
    assert ThinkingConfig.disabled().to_reasoning_effort() == "minimal"
    assert ThinkingConfig.disabled().to_anthropic_effort() is None
    assert ThinkingConfig.medium().to_anthropic_budget(10_000) == 5_000
    assert ThinkingConfig.budget_tokens(20_000).to_anthropic_budget(10_000) == 9_999
    assert not ThinkingConfig.level(ThinkingLevel.MINIMAL).to_enable_thinking()
    assert ThinkingConfig.level(ThinkingLevel.HIGH).to_glm_thinking_type() == "enabled"
    assert ThinkingConfig.budget_tokens(50_000).to_glm_reasoning_effort() == "max"
    with pytest.raises(ValueError):
        ThinkingConfig.parse_spec("bogus")


def test_thinking_config_mappings_are_complete() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith(
            "/thinking_config_values"
        )
    ]
    assert len(entries) == 14
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
