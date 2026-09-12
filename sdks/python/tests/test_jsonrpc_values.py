import json
from pathlib import Path

from echo_agent_sdk import JsonRpcNotification, JsonRpcRequest


def test_jsonrpc_values_preserve_mcp_constructors() -> None:
    assert JsonRpcRequest.new("tools/list").jsonrpc == "2.0"
    assert JsonRpcRequest.new("tools/list").id is None
    assert JsonRpcNotification.new(
        "notifications/initialized", {"ok": True}
    ).params == {"ok": True}


def test_jsonrpc_value_mappings_are_complete() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith("/jsonrpc_values")
    ]
    assert len(entries) == 4
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
