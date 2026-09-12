import json
from pathlib import Path

from echo_agent_sdk import SkillValidationReport


def test_skill_validation_reports_preserve_violation_gate_semantics() -> None:
    assert SkillValidationReport("skill").is_valid()
    assert not SkillValidationReport("skill", ("missing name",)).is_valid()


def test_skill_validation_mappings_are_complete() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith(
            "/skill_validation_values"
        )
    ]
    assert len(entries) == 2
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
