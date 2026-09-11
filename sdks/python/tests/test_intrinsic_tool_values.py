from __future__ import annotations

import json
from pathlib import Path

import pytest

from echo_agent_sdk import ToolCallParams, ToolResult


def test_tool_call_params_preserves_rust_parameter_typing() -> None:
    params = ToolCallParams.from_params(
        {"query": "echo", "limit": 3, "enabled": True, "nested": {"key": "value"}}
    )
    assert params.get_str("query") == "echo"
    assert params.get_number("limit") == 3.0
    assert params.get_bool("enabled") is True
    assert params.get("nested") == {"key": "value"}
    assert params.has("missing") is False
    assert params.len() == 4
    assert params.is_empty() is False
    params.validate_required("query", "string")
    with pytest.raises(TypeError, match="expected number, got string"):
        params.validate_required("query", "number")
    with pytest.raises(TypeError, match="Missing required parameter"):
        params.validate_required("missing", "string")
    assert ToolCallParams.from_value("not-an-object").is_empty()
    with pytest.raises(TypeError, match="params keys must be text"):
        ToolCallParams.from_params({1: "invalid"})  # type: ignore[dict-item]


def test_tool_result_constructors_and_modifiers_match_rust() -> None:
    success = (
        ToolResult.success_json({"answer": 42})
        .with_meta("source", "test")
        .with_mime_type("application/json")
        .with_truncated(True)
    )
    assert success.kind == {"kind": "json"}
    assert success.success is True
    assert success.output == '{"answer":42}'
    assert success.data is not None and success.data["kind"] == "map"
    assert success.metadata == {"source": "test"}
    assert success.truncated is True
    collision = ToolResult.success_json({"kind": "string", "value": "foo"})
    assert collision.data is not None and collision.data["kind"] == "map"
    assert [entry["key"]["value"] for entry in collision.data["value"]] == [
        "kind",
        "value",
    ]

    invalid = ToolResult.invalid_arguments("query required").with_output("bad input")
    assert invalid.success is False
    assert invalid.error == "query required"
    assert invalid.failure is not None
    assert invalid.failure["category"] == "invalid_arguments"
    assert invalid.failure["recovery"] == "correct_arguments"
    assert (
        ToolResult.failure_result("unavailable", "offline").failure["recovery"]
        == "restore_then_retry"
    )
    assert (
        ToolResult.failure_result("timeout", "slow").failure["recovery"]
        == "verify_then_retry"
    )
    assert (
        ToolResult.failure_result("partial_side_effect", "partial").failure["recovery"]
        == "verify_then_retry"
    )
    assert (
        ToolResult.failure_result("transient", "retry").failure["recovery"] == "retry"
    )
    assert ToolResult.failure_result("permanent", "stop").failure["recovery"] == "stop"
    with pytest.raises(ValueError, match="unknown tool failure category"):
        ToolResult.failure_result("unknown", "bad")  # type: ignore[arg-type]
    assert ToolResult.success_result("ok").with_failure(
        {
            "category": "transient",
            "recovery": "retry",
            "side_effect": "none",
            "retry_after_ms": "18446744073709551615",
            "idempotency_key": "key-1",
            "postcondition": "eventual success",
        }
    )
    with pytest.raises(ValueError, match="canonical"):
        ToolResult.success_result("ok").with_failure(
            {
                "category": "transient",
                "recovery": "retry",
                "side_effect": "none",
                "retry_after_ms": "01",
            }
        )
    with pytest.raises(TypeError, match="postcondition must be text"):
        ToolResult.success_result("ok").with_failure(
            {
                "category": "transient",
                "recovery": "retry",
                "side_effect": "none",
                "postcondition": 42,
            }
        )
    assert ToolResult.success_result("ok").success is True
    assert ToolResult.error_result("failed").kind["error_code"] == "tool_error"
    assert ToolResult.success_with_kind(
        {"kind": "image", "mime_type": "image/png"}, "image"
    )
    with pytest.raises(TypeError, match="mime_type"):
        ToolResult.success_with_kind({"kind": "image"}, "image")
    with pytest.raises(TypeError, match="unknown tool result kind"):
        ToolResult.success_with_kind({"kind": "unknown"}, "bad")


def test_local_tool_value_routes_have_completed_python_mappings() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["route"]["route"] == "intrinsic:language-local-wire-helper"
    ]
    assert len(entries) == 28
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
