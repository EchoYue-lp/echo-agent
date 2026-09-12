import json
from pathlib import Path

from echo_agent_sdk import ToolOutputArtifactConfig


def test_artifact_config_preserves_defaults_and_immutable_builders() -> None:
    config = ToolOutputArtifactConfig.new("/tmp/artifacts", "temporary_1h")
    updated = config.threshold_bytes_with(0).max_age_secs_with(60)
    assert config.threshold_bytes == 1_048_576
    assert updated.threshold_bytes == 1
    assert updated.max_age_secs == 60
    assert ToolOutputArtifactConfig.default().max_age_secs == 3_600


def test_artifact_config_mappings_are_complete() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith(
            "/tool_output_artifact_config_values"
        )
    ]
    assert len(entries) == 7
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
