from __future__ import annotations

import json
from pathlib import Path

import pytest

from echo_agent_sdk import TaskState


def test_a2a_task_state_matches_rust_terminal_and_transition_table() -> None:
    assert not TaskState.SUBMITTED.is_terminal()
    assert TaskState.COMPLETED.is_terminal()
    assert TaskState.FAILED.is_terminal()
    assert TaskState.CANCELED.is_terminal()
    assert TaskState.SUBMITTED.can_transition_to(TaskState.WORKING)
    assert TaskState.WORKING.can_transition_to(TaskState.INPUT_REQUIRED)
    assert TaskState.INPUT_REQUIRED.can_transition_to(TaskState.WORKING)
    assert not TaskState.COMPLETED.can_transition_to(TaskState.WORKING)
    assert not TaskState.SUBMITTED.can_transition_to(TaskState.COMPLETED)
    assert str(TaskState.INPUT_REQUIRED) == "input-required"
    with pytest.raises(TypeError, match="unknown A2A task state"):
        TaskState.SUBMITTED.can_transition_to("unknown")  # type: ignore[arg-type]


def test_a2a_task_state_intrinsics_have_completed_python_mappings() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith("/a2a_task_state")
    ]
    assert len(entries) == 10
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
