from __future__ import annotations

import json
from pathlib import Path

import pytest

from echo_agent_sdk import (
    A2AMessage,
    A2ATaskStatus,
    AgentProvider,
    AgentSkill,
    TaskState,
)


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


def test_a2a_value_constructors_preserve_rust_semantics() -> None:
    message = A2AMessage.user_text("hello")
    assert message.role == "user"
    assert message.text_content() == "hello"
    assert A2AMessage.agent_text("answer").role == "agent"
    status = A2ATaskStatus.with_message(TaskState.WORKING, message)
    assert status.state is TaskState.WORKING
    assert status.message is message
    assert "T" in status.timestamp
    provider = AgentProvider.new("Echo").with_url("https://example.test")
    assert provider.organization == "Echo"
    assert provider.url == "https://example.test"
    skill = (
        AgentSkill.new("search", "Search docs")
        .with_examples(["rust"])
        .with_tags(["docs"])
    )
    assert skill.id == "search"
    assert skill.examples == ("rust",)
    assert skill.tags == ("docs",)
    with pytest.raises(TypeError, match="message text"):
        A2AMessage.user_text(None)  # type: ignore[arg-type]


def test_a2a_value_identity_mappings_are_ready() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith("/a2a_values")
    ]
    assert len(entries) == 14
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
