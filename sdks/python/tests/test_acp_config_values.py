import json
from pathlib import Path

import pytest

from echo_agent_sdk import AcpAdapterConfig, AcpDuration


def test_acp_adapter_config_preserves_defaults_and_validation() -> None:
    config = AcpAdapterConfig.default()
    config.validate()
    assert config.max_sessions == 128
    assert config.shutdown_timeout == AcpDuration(5)
    with pytest.raises(ValueError, match="resource limits"):
        AcpAdapterConfig(
            name=config.name,
            title=config.title,
            version=config.version,
            max_sessions=0,
            max_prompt_chars=config.max_prompt_chars,
            max_update_chars=config.max_update_chars,
            max_updates_per_turn=config.max_updates_per_turn,
            max_total_update_chars=config.max_total_update_chars,
            max_extension_concurrency=config.max_extension_concurrency,
            shutdown_timeout=config.shutdown_timeout,
        ).validate()


def test_acp_adapter_config_mappings_are_complete() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith("/acp_config_values")
    ]
    assert len(entries) == 13
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
