import json
from pathlib import Path

from echo_agent_sdk import LlmApiProtocol


def test_endpoint_helpers_preserve_rust_protocol_detection() -> None:
    assert LlmApiProtocol.CHAT_COMPLETIONS.endpoint_path() == "chat/completions"
    assert (
        LlmApiProtocol.from_endpoint("https://api.openai.com/v1/responses?trace=true")
        is LlmApiProtocol.RESPONSES
    )
    assert (
        LlmApiProtocol.from_endpoint("https://api.anthropic.com/v1/messages")
        is LlmApiProtocol.ANTHROPIC
    )
    assert (
        LlmApiProtocol.from_endpoint("https://gateway.example/v1/chat/completions")
        is LlmApiProtocol.CHAT_COMPLETIONS
    )
    assert LlmApiProtocol.try_from_endpoint("https://gateway.example/v1") is None
    assert (
        LlmApiProtocol.from_endpoint(
            "https://gateway.example/v1?upstream=https://api.anthropic.com/v1/"
        )
        is LlmApiProtocol.CHAT_COMPLETIONS
    )


def test_protocol_mappings_are_complete() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith(
            "/llm_api_protocol_values"
        )
    ]
    assert len(entries) == 7
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
