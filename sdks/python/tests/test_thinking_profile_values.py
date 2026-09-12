import json
from pathlib import Path

from echo_agent_sdk import (
    ThinkingLevel,
    ThinkingProfile,
    ThinkingProtocol,
    resolve_thinking_profile,
)


def test_thinking_profiles_preserve_provider_model_selection() -> None:
    assert not ThinkingProfile.unknown().supports_manual_control()
    assert (
        resolve_thinking_profile("openai", "gpt-5.6-sol").protocol
        is ThinkingProtocol.OPENAI_REASONING_EFFORT
    )
    assert (
        resolve_thinking_profile("anthropic", "claude-opus-4.6", "anthropic").protocol
        is ThinkingProtocol.ANTHROPIC_EFFORT
    )
    assert (
        resolve_thinking_profile("anthropic", "claude-opus-4-6", "anthropic").protocol
        is ThinkingProtocol.ANTHROPIC_EFFORT
    )
    assert (
        resolve_thinking_profile("anthropic", "claude-opus-4.6.7", "anthropic").protocol
        is ThinkingProtocol.NONE
    )
    assert (
        resolve_thinking_profile("zhipu", "glm-5-2").protocol
        is ThinkingProtocol.GLM_REASONING_EFFORT
    )
    assert (
        resolve_thinking_profile("ollama", "qwen3-32b").protocol
        is ThinkingProtocol.OLLAMA_THINK
    )
    assert (
        resolve_thinking_profile("dashscope", "deepseek-v4-pro").protocol
        is ThinkingProtocol.ENABLE_THINKING_FLAG
    )
    assert not resolve_thinking_profile("custom", "unknown").levels
    assert ThinkingProfile.new(
        ThinkingProtocol.OPENAI_REASONING_EFFORT, (ThinkingLevel.HIGH,)
    ).supports_manual_control()


def test_thinking_profile_mappings_are_complete() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith(
            "/thinking_profile_values"
        )
    ]
    assert len(entries) == 5
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
