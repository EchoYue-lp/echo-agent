import json
from pathlib import Path

from echo_agent_sdk import DependencyKind, SkillSource


def test_dependency_and_skill_source_values_preserve_spellings() -> None:
    assert DependencyKind.PYTHON_PKG.as_str() == "python_pkg"
    assert DependencyKind.NODE_MODULE.as_str() == "node_module"
    assert SkillSource.MCP.as_str() == "mcp"


def test_dependency_value_mappings_are_complete() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith("/dependency_values")
    ]
    assert len(entries) == 7
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
