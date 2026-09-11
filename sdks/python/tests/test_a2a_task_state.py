from __future__ import annotations

import json
from pathlib import Path

import pytest

from echo_agent_sdk import (
    A2AArtifact,
    A2AError,
    A2AMessage,
    A2ATaskStatus,
    AgentCard,
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


def test_a2a_agent_card_builder_preserves_local_value_semantics() -> None:
    skill = AgentSkill.new("search", "Search docs")
    card = (
        AgentCard.builder("eko", "https://example.test")
        .description("Local agent")
        .version("1.0.0")
        .provider(AgentProvider.new("Echo"))
        .skill(skill)
        .input_modes(["text/plain"])
        .output_modes(["text/plain", "application/json"])
        .streaming()
        .push_notifications()
        .build()
    )
    assert card.name == "eko"
    assert card.description == "Local agent"
    assert card.skills == (skill,)
    assert card.default_output_modes == ("text/plain", "application/json")
    assert card.capabilities.streaming
    assert card.capabilities.push_notifications


def test_a2a_agent_card_identity_mappings_are_ready() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith("/a2a_agent_card")
    ]
    assert len(entries) == 14
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)


def test_a2a_artifact_and_error_values_preserve_wire_fields() -> None:
    artifact = A2AArtifact.new(
        [{"type": "text", "text": "chunk"}],
        name="answer",
        index=2,
        append=True,
    )
    assert artifact.name == "answer"
    assert artifact.index == 2
    assert artifact.append
    assert artifact.parts[0]["text"] == "chunk"
    error = A2AError.new(-32001, "missing")
    assert error.code == -32001
    assert error.message == "missing"
    with pytest.raises(TypeError, match="i32"):
        A2AError.new(2**31, "overflow")
    with pytest.raises(TypeError, match="append"):
        A2AArtifact.new([], append="yes")  # type: ignore[arg-type]


def test_a2a_wire_value_mappings_are_ready() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith("/a2a_wire_values")
    ]
    assert len(entries) == 8
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
