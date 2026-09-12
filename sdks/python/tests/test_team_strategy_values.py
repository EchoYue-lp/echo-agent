import json
from pathlib import Path

from echo_agent_sdk import TeamStrategy


def test_team_strategies_preserve_values_and_descriptions() -> None:
    strategy = TeamStrategy.debate("judge", ["a", "b"])
    assert strategy.name() == "debate"
    assert strategy.debaters == ("a", "b")
    assert (
        strategy.description()
        == "Debaters propose independently and a judge synthesizes"
    )


def test_team_strategy_mappings_are_complete() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith(
            "/team_strategy_values"
        )
    ]
    assert len(entries) == 7
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
