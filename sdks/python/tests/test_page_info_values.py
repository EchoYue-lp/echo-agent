import json
from pathlib import Path

from echo_agent_sdk import PageInfo, ToolResult


def test_page_info_applies_metadata_truncation_and_continuation() -> None:
    result = PageInfo("cursor-2", True, True, 4, 2).apply_to(
        ToolResult.success_result("items")
    )
    assert result.truncated
    assert result.metadata["page.next_cursor"] == "cursor-2"
    assert "[page]" in result.output


def test_page_info_mappings_are_complete() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith("/page_info_values")
    ]
    assert len(entries) == 2
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
