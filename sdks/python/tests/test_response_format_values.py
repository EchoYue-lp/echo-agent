import json
from pathlib import Path

from echo_agent_sdk import ResponseFormat


def test_response_formats_preserve_rust_tagged_values() -> None:
    assert not ResponseFormat.text().is_json()
    assert ResponseFormat.json_object().is_json()
    value = ResponseFormat.json_schema("answer", {"type": "object"})
    assert value.is_json()
    assert value.type == "json_schema"
    assert value.json_schema_spec is not None
    assert value.json_schema_spec.name == "answer"
    assert value.json_schema_spec.strict
    assert value.json_schema_spec.schema == {"type": "object"}


def test_response_format_mappings_are_complete() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith(
            "/response_format_values"
        )
    ]
    assert len(entries) == 6
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
