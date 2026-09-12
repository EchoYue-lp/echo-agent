import json
from pathlib import Path

from echo_agent_sdk import ModelInputModality


def test_modality_defaults_preserve_rust_order_and_spellings() -> None:
    assert ModelInputModality.TEXT.value == "text"
    assert ModelInputModality.text_only() == [ModelInputModality.TEXT]
    assert ModelInputModality.all_supported() == [
        ModelInputModality.TEXT,
        ModelInputModality.IMAGE,
        ModelInputModality.AUDIO,
        ModelInputModality.VIDEO,
    ]


def test_modality_mappings_are_complete() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith(
            "/model_input_modality_values"
        )
    ]
    assert len(entries) == 7
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
