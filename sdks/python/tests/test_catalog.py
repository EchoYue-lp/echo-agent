from __future__ import annotations

import json
from pathlib import Path

from echo_agent_sdk import FacadeCatalog


def _repo_root() -> Path:
    return Path(__file__).resolve().parents[3]


def test_catalog_enumerates_every_canonical_operation() -> None:
    catalog = FacadeCatalog()
    document = json.loads(
        (_repo_root() / "sdks/shared/facade-operation-catalog.json").read_text(
            encoding="utf-8"
        )
    )
    expected = sum(
        (1 if route.get("operation") else 0)
        + len(route.get("operation_signatures", []))
        for route in document["routes"]
    )
    assert len(catalog.operations()) == expected
    assert len({operation.operation for operation in catalog.operations()}) == expected
    assert all(
        operation.signature.startswith("sha256:") for operation in catalog.operations()
    )


def test_every_executable_facade_item_has_a_completed_python_mapping() -> None:
    manifest = json.loads(
        (_repo_root() / "contracts/sdk/parity-manifest.json").read_text(
            encoding="utf-8"
        )
    )
    assert manifest["entries"]
    for entry in manifest["entries"]:
        mapping = entry["languages"]["python"]
        if entry["route"]["surface"] != "intrinsic":
            assert mapping["status"] == "done", entry["path"]
        assert mapping["contract_test"].startswith("sdk-parity/"), entry["path"]
