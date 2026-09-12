import json
from datetime import datetime, timezone
from pathlib import Path

import pytest

from echo_agent_sdk import (
    local_rfc3339_deserialize,
    local_rfc3339_serialize,
    now_local,
    now_millis,
    now_secs,
    option_local_rfc3339_deserialize,
    option_local_rfc3339_serialize,
    to_local,
)


def test_time_helpers_preserve_instant_and_null_semantics() -> None:
    before = now_millis()
    assert now_millis() >= before
    assert now_secs() > 1_700_000_000
    source = datetime(2026, 7, 9, 1, 50, 48, 876_000, tzinfo=timezone.utc)
    local = local_rfc3339_serialize(source)
    assert local[-6] in "+-"
    assert local_rfc3339_deserialize(local) == source
    assert to_local(source).utcoffset() == datetime.now().astimezone().utcoffset()
    assert option_local_rfc3339_serialize(None) is None
    assert option_local_rfc3339_deserialize(None) is None
    assert now_local().tzinfo is not None
    with pytest.raises(ValueError):
        local_rfc3339_deserialize("bad")


def test_time_helper_mappings_are_complete() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith("/time_values")
    ]
    assert len(entries) == 8
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
