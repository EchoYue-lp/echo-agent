import json
from pathlib import Path

from echo_agent_sdk import HOOK_EVENT_ALL, HookEvent, HookEventCategory


def test_hook_events_preserve_names_categories_and_matchers() -> None:
    assert len(HOOK_EVENT_ALL) == 31
    assert HookEvent.PRE_TOOL_USE.as_str() == "PreToolUse"
    assert HookEvent.from_name("TaskCompleted") is HookEvent.TASK_COMPLETED
    assert HookEvent.from_name("missing") is None
    assert HookEvent.PRE_TOOL_USE.category() is HookEventCategory.TOOL
    assert HookEvent.STOP_FAILURE.category() is HookEventCategory.ERROR
    assert HookEvent.RULE_PROMOTED.category() is HookEventCategory.EVOLUTION
    assert HookEvent.PERMISSION_DENIED.is_tool_event()
    assert not HookEvent.SESSION_START.is_tool_event()
    assert HookEvent.STOP_FAILURE.supports_matcher()


def test_hook_event_mappings_are_complete() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith("/hook_event_values")
    ]
    assert len(entries) == 45
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
