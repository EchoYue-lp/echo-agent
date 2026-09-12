import json
from pathlib import Path

from echo_agent_sdk import (
    ModelProfile,
    ModelProfileOverride,
    ModelProfileResolver,
    ProviderCapabilities,
    ThinkingProtocol,
    infer_context_window,
)


def test_model_profiles_preserve_rust_provider_and_model_policy() -> None:
    openai = ModelProfile.from_provider_name("gpt-5.6-sol", "openai")
    assert openai.supports_reasoning
    assert openai.max_output_tokens == 16_384
    assert openai.context_window == 1_050_000
    assert openai.tokenizer_name == "o200k_base"
    assert openai.thinking_protocol is ThinkingProtocol.OPENAI_REASONING_EFFORT
    assert openai.supports_images
    assert not ModelProfile.from_provider_name("o3-mini", "openai").supports_images
    assert infer_context_window(" moonshot ", "kimi-k2.7-code") == 256_000
    assert infer_context_window("openai", "gpt-5.5") is None


def test_model_profile_resolver_applies_provider_then_exact_overrides() -> None:
    profile = (
        ModelProfileResolver.new()
        .register_provider_default(
            " OpenAI ",
            ModelProfileOverride(
                supports_parallel_tool_calls=False,
                context_window=99,
                excluded_tools=frozenset({"shell"}),
            ),
        )
        .register_exact(
            "openai",
            "gpt-5.6-sol",
            ModelProfileOverride(
                supports_parallel_tool_calls=True,
                supports_structured_output=False,
                prompt_suffix="exact",
                excluded_tools=frozenset({"browser"}),
            ),
        )
        .resolve(
            "openai",
            "gpt-5.6-sol",
            ProviderCapabilities.from_provider_name("openai"),
        )
    )
    assert profile.supports_parallel_tool_calls
    assert profile.context_window == 99
    assert not profile.capabilities.structured_output
    assert profile.prompt_suffix == "exact"
    assert profile.excluded_tools == frozenset({"browser", "shell"})


def test_model_profile_mappings_are_complete() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith(
            "/model_profile_values"
        )
    ]
    assert len(entries) == 32
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
