import json
from pathlib import Path

from echo_agent_sdk import ProviderCapabilities


def test_provider_capabilities_preserve_default_dialects() -> None:
    assert ProviderCapabilities.from_provider_name("anthropic").named_sse_events
    assert not ProviderCapabilities.from_provider_name(" anthropic ").named_sse_events
    assert ProviderCapabilities.from_provider_name("ollama").ndjson_streaming
    assert ProviderCapabilities.from_provider_name("custom").tool_support
    assert ProviderCapabilities.anthropic().tokenizer_name == "claude"
    assert ProviderCapabilities.openai_compatible().supports_tool_choice_none


def test_provider_capability_mappings_are_complete() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith(
            "/provider_capabilities_values"
        )
    ]
    assert len(entries) == 4
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
