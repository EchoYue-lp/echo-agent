from echo_agent_sdk import (
    WireHandle,
    from_wire,
    to_wire,
    wire_bytes,
    wire_duration,
    wire_i64,
    wire_timestamp,
    wire_u64,
    wire_utf8_path,
)
from echo_agent_sdk.catalog import FacadeCatalog
from echo_agent_sdk.wire import parse_wire_handle


def test_handle_is_encoded_before_wire_value() -> None:
    handle = WireHandle("session-1", "1", "session")
    assert to_wire(handle) == {"kind": "handle", "value": handle.to_dict()}
    assert from_wire(to_wire(handle)) == handle


def test_handle_validation_matches_rust_shape_and_generation_contract() -> None:
    assert WireHandle("run-1", str((1 << 64) - 1), "run").to_dict() == {
        "id": "run-1",
        "generation": "18446744073709551615",
        "kind": "run",
    }

    import pytest

    invalid_handles = [
        ("   ", "1", "run"),
        ("r" * 257, "1", "run"),
        ("run-1", "01", "run"),
        ("run-1", str(1 << 64), "run"),
        ("run-1", "1", "unknown"),
    ]
    for fields in invalid_handles:
        with pytest.raises((TypeError, ValueError, OverflowError)):
            WireHandle(*fields)

    with pytest.raises(TypeError):
        parse_wire_handle({"id": "run-1", "generation": 1, "kind": "run"})


def test_outgoing_wire_wrappers_reject_noncanonical_handles_and_integers() -> None:
    import pytest

    with pytest.raises(ValueError):
        to_wire(
            {"kind": "handle", "value": {"id": "", "generation": "1", "kind": "run"}}
        )
    with pytest.raises(TypeError):
        to_wire({"kind": "u64", "value": 1})
    with pytest.raises(ValueError):
        to_wire({"kind": "u64", "value": "01"})


def test_nested_values_preserve_u64_as_text() -> None:
    encoded = to_wire({"count": 2**63})
    assert from_wire(encoded) == {"count": str(2**63)}


def test_nested_maps_and_lists_round_trip() -> None:
    value = {"answer": ["a", {"count": str(2**64 - 1)}]}
    assert from_wire(to_wire(value)) == value


def test_integer_bounds_match_wire_contract() -> None:
    assert to_wire(-(1 << 63)) == {
        "kind": "i64",
        "value": "-9223372036854775808",
    }
    assert to_wire((1 << 64) - 1) == {
        "kind": "u64",
        "value": "18446744073709551615",
    }
    import pytest

    with pytest.raises(OverflowError):
        to_wire(1 << 64)
    with pytest.raises(OverflowError):
        to_wire(-(1 << 63) - 1)
    with pytest.raises(TypeError):
        wire_u64(True)
    with pytest.raises(TypeError):
        wire_i64(1.0)  # type: ignore[arg-type]


def test_canonical_resolver_selects_source_and_family_routes() -> None:
    catalog = FacadeCatalog()
    assert (
        catalog.resolve("echo_core::agent::Agent::name").method
        == "_echo_agent/facade/invoke"
    )
    assert catalog.resolve("memory.store.put").method == "_echo_agent/memory/op"


def test_catalog_enumerates_every_source_and_family_operation_once() -> None:
    catalog = FacadeCatalog()
    operations = catalog.operations()

    assert len(operations) == len(catalog.source_operations()) + len(
        catalog.family_operations()
    )
    assert {family.family for family in catalog.families()} == {
        str(family["family"]) for family in catalog.document["families"]
    }
    assert len(
        {
            (item.operation, item.family, item.method, item.signature)
            for item in operations
        }
    ) == len(operations)
    assert sum(len(family.operations) for family in catalog.families()) == len(
        operations
    )
    assert all(
        item.signature.startswith("sha256:") and item.family and item.method
        for item in operations
    )
    assert operations == tuple(
        sorted(
            operations,
            key=lambda item: (
                item.operation,
                item.family,
                item.method,
                item.signature,
            ),
        )
    )


def test_typed_and_unknown_variants_retain_discriminator() -> None:
    value = {
        "kind": "unknown",
        "value": {"kind": "u64", "value": "18446744073709551615"},
    }
    assert from_wire(to_wire(value)) == value
    assert from_wire(
        {"kind": "path", "value": {"encoding": "utf8", "path": "/tmp/a"}}
    ) == {
        "kind": "path",
        "value": {"encoding": "utf8", "path": "/tmp/a"},
    }
    for typed in (
        {
            "kind": "record",
            "value": {
                "type_id": "example.Record",
                "fields": [{"name": "count", "value": {"kind": "u64", "value": "7"}}],
            },
        },
        {
            "kind": "variant",
            "value": {
                "type_id": "example.State",
                "variant": "Ready",
                "fields": [],
            },
        },
    ):
        assert from_wire(typed) is typed
        assert to_wire(typed) is typed


def test_scalar_helpers_emit_canonical_lossless_shapes() -> None:
    assert wire_u64((1 << 64) - 1) == {
        "kind": "u64",
        "value": "18446744073709551615",
    }
    assert wire_i64(-(1 << 63)) == {
        "kind": "i64",
        "value": "-9223372036854775808",
    }
    assert wire_bytes(b"\x00\xff") == {
        "kind": "bytes",
        "value": {"base64": "AP8"},
    }
    assert wire_utf8_path("/tmp/a") == {
        "kind": "path",
        "value": {"encoding": "utf8", "path": "/tmp/a"},
    }
    assert wire_duration(2, 3) == {
        "kind": "duration",
        "value": {"seconds": "2", "nanos": 3},
    }
    assert wire_timestamp(-1, 4) == {
        "kind": "timestamp",
        "value": {"unix_seconds": "-1", "nanos": 4},
    }


def test_decoding_rejects_malformed_handles_and_integer_text() -> None:
    import pytest

    with pytest.raises(ValueError):
        WireHandle("", "1", "session")
    with pytest.raises(ValueError):
        from_wire({"kind": "u64", "value": "01"})
    with pytest.raises(OverflowError):
        from_wire({"kind": "i64", "value": "9223372036854775808"})
    with pytest.raises(TypeError):
        from_wire({"kind": "u64", "value": 1})
    with pytest.raises(TypeError):
        from_wire({"kind": "i64", "value": True})
    with pytest.raises(OverflowError):
        from_wire({"kind": "u64", "value": "18446744073709551616"})


def test_scalar_helpers_reject_python_values_that_are_not_wire_scalars() -> None:
    import pytest

    with pytest.raises(TypeError):
        wire_duration(1, True)  # type: ignore[arg-type]
    with pytest.raises(TypeError):
        wire_timestamp(1, 0, rfc3339=123)  # type: ignore[arg-type]
    with pytest.raises(TypeError):
        wire_utf8_path(123)  # type: ignore[arg-type]
    with pytest.raises(ValueError):
        wire_utf8_path("é:/relative")
    with pytest.raises(TypeError):
        to_wire({1: "not a string key"})
