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


def test_every_external_sdk_contract_has_a_completed_python_mapping() -> None:
    manifest = json.loads(
        (_repo_root() / "contracts/sdk/parity-manifest.json").read_text(
            encoding="utf-8"
        )
    )
    assert manifest["entries"]
    counts: dict[str, int] = {}
    expected_scopes = {
        "external_contract",
        "host_or_rust_only",
        "language_intrinsic",
        "internal_helper",
        "deferred",
    }
    for entry in manifest["entries"]:
        mapping = entry["languages"]["python"]
        assert entry["sdk_scope"] in expected_scopes, entry["path"]
        if entry["canonical"]:
            counts[entry["sdk_scope"]] = counts.get(entry["sdk_scope"], 0) + 1
        if entry["sdk_scope"] == "external_contract":
            assert mapping["status"] == "done", entry["path"]
        assert mapping["contract_test"].startswith("sdk-parity/"), entry["path"]
    assert counts == {
        "external_contract": 5607,
        "host_or_rust_only": 1765,
        "language_intrinsic": 781,
        "internal_helper": 90,
        "deferred": 1441,
    }
