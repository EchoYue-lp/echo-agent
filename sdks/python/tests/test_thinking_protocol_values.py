import json
from pathlib import Path

from echo_agent_sdk import ThinkingProtocol


def test_thinking_protocols_preserve_wire_names_and_emission() -> None:
    assert ThinkingProtocol.OPENAI_REASONING_EFFORT.value == "openai_reasoning_effort"
    assert not ThinkingProtocol.NONE.emits_field()
    assert not ThinkingProtocol.MODEL_MANAGED.emits_field()
    assert not ThinkingProtocol.ANTHROPIC_ADAPTIVE.emits_field()
    assert ThinkingProtocol.OLLAMA_THINK.emits_field()


def test_thinking_protocol_mappings_are_complete() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith(
            "/thinking_protocol_values"
        )
    ]
    assert len(entries) == 13
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
