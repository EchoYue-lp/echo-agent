import json
from pathlib import Path

import pytest

from echo_agent_sdk import LlmTimeouts, TokenBudget, TokenBudgetConfig


def test_token_budget_and_timeout_policies_preserve_semantics() -> None:
    budget = TokenBudget.new(100_000)
    assert budget.system_prompt_budget() == 10_000
    assert budget.tool_definitions_budget() == 5_000
    assert budget.conversation_budget() == 65_000
    allocation = budget.allocate(5_000, 2_000, 75_000)
    assert not allocation.ok()
    assert allocation.needs_compression()
    assert allocation.conversation_excess == 2_000
    assert (
        budget.with_allocations(0.05, 0.05, 0.05, 0.05).conversation_budget() == 80_000
    )
    with pytest.raises(ValueError):
        budget.with_allocations(0.8, 0.3, 0, 0)
    assert not TokenBudgetConfig.disabled().is_enabled
    assert (
        TokenBudgetConfig.enabled().with_total_window(10_000).build(1_000).total_window
        == 10_000
    )
    assert LlmTimeouts.default().without_idle_timeout().idle_timeout() is None
    assert LlmTimeouts.default().with_overall_timeout(0).overall_timeout() is None


def test_token_budget_mappings_are_complete() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith(
            "/token_budget_values"
        )
    ]
    assert len(entries) == 35
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
