import json
from pathlib import Path

from echo_agent_sdk import SkillContent, SkillResourceEntry


def test_skill_content_preserves_structured_prompt_block() -> None:
    content = SkillContent(
        "demo",
        "/tmp/demo",
        "  Do the thing.  ",
        ("read",),
        (SkillResourceEntry("script", "scripts/run.sh"),),
    )
    block = content.to_prompt_block()
    assert '<skill_content name="demo">' in block
    assert "<allowed_tools>" in block
    assert '<file kind="script">scripts/run.sh</file>' in block


def test_skill_content_mappings_are_complete() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith(
            "/skill_content_values"
        )
    ]
    assert len(entries) == 2
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
