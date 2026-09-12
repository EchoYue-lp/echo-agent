import json
from pathlib import Path

from echo_agent_sdk import SegmentRange


def test_segment_ranges_preserve_saturating_half_open_length() -> None:
    assert SegmentRange(2, 5).len() == 3
    assert SegmentRange(5, 2).len() == 0
    assert SegmentRange(5, 2).is_empty()


def test_segment_range_mappings_are_complete() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith(
            "/segment_range_values"
        )
    ]
    assert len(entries) == 3
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
