import json
from pathlib import Path

import pytest

from echo_agent_sdk import JwtClaims, JwtConfig


def test_jwt_config_and_claims_preserve_local_values() -> None:
    config = JwtConfig.hs256("secret").with_issuer("echo-agent").with_audience("a2a")
    assert config.is_enabled()
    assert "[redacted]" in repr(config)
    assert not JwtConfig.disabled().is_enabled()
    assert JwtClaims(sub="subject").subject() == "subject"
    with pytest.raises(ValueError, match="invalid RSA public key"):
        JwtConfig.rs256("invalid")


def test_jwt_mappings_are_complete() -> None:
    root = Path(__file__).resolve().parents[3]
    manifest = json.loads((root / "contracts/sdk/parity-manifest.json").read_text())
    entries = [
        entry
        for entry in manifest["entries"]
        if entry["canonical"]
        and entry["languages"]["python"]["contract_test"].endswith("/jwt_values")
    ]
    assert len(entries) == 9
    assert all(entry["languages"]["python"]["status"] == "done" for entry in entries)
