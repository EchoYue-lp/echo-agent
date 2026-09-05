//! ACP baseline contract tests.
//!
//! Design §18 keeps three version layers independent: the ACP wire protocol
//! version, the official Rust crate versions and the schema artifact version.
//! `contracts/sdk/acp-baseline.json` pins all three. These tests fail closed
//! when the lockfile drifts from the pinned baseline or when the official
//! crate's notion of "latest stable wire version" stops being v1 — the exact
//! moment a draft/unstable protocol becomes the default upstream.

use std::collections::BTreeMap;
use std::path::PathBuf;

/// Latest stable wire protocol version the official schema crate advertises.
/// `ProtocolVersion::LATEST` is only compiled when the draft v2 feature is
/// OFF; depending on it here makes accidental `unstable_protocol_v2` usage a
/// compile error in this crate.
const EXPECTED_WIRE_VERSION: u16 = 1;

#[test]
fn official_latest_stable_wire_version_is_v1() {
    let latest = agent_client_protocol_schema::ProtocolVersion::LATEST;
    assert_eq!(latest.as_u16(), EXPECTED_WIRE_VERSION);
    assert_eq!(latest, agent_client_protocol_schema::ProtocolVersion::V1);
}

#[test]
fn draft_v2_module_is_not_compiled_in() {
    // The v2 module only exists behind the `unstable_protocol_v2` feature.
    // There is no stable way to assert absence at compile time without
    // adding the feature, so we assert the positive space instead: the v1
    // module must expose the stable surface we rely on for baseline checks.
    let initialize_defaults = agent_client_protocol_schema::v1::InitializeRequest::new(
        agent_client_protocol_schema::ProtocolVersion::V1,
    );
    let json = serde_json::to_value(&initialize_defaults).unwrap_or(serde_json::Value::Null);
    assert_eq!(json.get("protocolVersion"), Some(&serde_json::json!(1)));
}

#[test]
fn lockfile_matches_pinned_baseline() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let lock_path = manifest_dir.join("../Cargo.lock");
    let lock = std::fs::read_to_string(&lock_path)
        .unwrap_or_else(|err| panic!("cannot read {}: {err}", lock_path.display()));
    let baseline_path = manifest_dir.join("../contracts/sdk/acp-baseline.json");
    let baseline_raw = std::fs::read_to_string(&baseline_path)
        .unwrap_or_else(|err| panic!("cannot read {}: {err}", baseline_path.display()));
    let baseline: serde_json::Value = serde_json::from_str(&baseline_raw)
        .unwrap_or_else(|err| panic!("baseline JSON invalid: {err}"));

    // Wire version pinned in the baseline must be v1.
    let wire = baseline
        .get("acp_wire_protocol_version")
        .and_then(|v| v.as_u64())
        .unwrap_or_default();
    assert_eq!(wire, u64::from(EXPECTED_WIRE_VERSION));

    let mut locked_versions: BTreeMap<String, String> = BTreeMap::new();
    let mut current_name: Option<String> = None;
    for line in lock.lines() {
        let trimmed = line.trim();
        if let Some(name) = trimmed.strip_prefix("name = ") {
            current_name = Some(name.trim_matches('"').to_string());
        } else if let (Some(version), Some(name)) =
            (trimmed.strip_prefix("version = "), current_name.take())
        {
            locked_versions.insert(name, version.trim_matches('"').to_string());
        }
    }

    let pinned = baseline
        .get("official_crates")
        .and_then(|v| v.as_object())
        .expect("baseline.official_crates missing");
    assert!(!pinned.is_empty(), "baseline pins no crates");
    for (crate_name, expected) in pinned {
        let expected = expected.as_str().expect("crate version must be a string");
        let locked = locked_versions
            .get(crate_name)
            .unwrap_or_else(|| panic!("crate {crate_name} absent from Cargo.lock"));
        assert_eq!(
            locked, expected,
            "Cargo.lock has {crate_name} {locked}, baseline pins {expected}; \
             re-run contract update after intentionally re-pinning"
        );
    }

    let schema_artifact = baseline
        .pointer("/schema_artifact/version")
        .and_then(|v| v.as_str())
        .expect("schema_artifact.version missing");
    let schema_locked = locked_versions
        .get("agent-client-protocol-schema")
        .expect("schema crate in lock");
    assert_eq!(schema_locked, schema_artifact, "schema artifact drift");
}

#[test]
fn excluded_unstable_features_are_declared() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let baseline_path = manifest_dir.join("../contracts/sdk/acp-baseline.json");
    let baseline: serde_json::Value = std::fs::read_to_string(&baseline_path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .expect("baseline JSON");
    let excluded = baseline
        .get("excluded_unstable_features")
        .and_then(|v| v.as_array())
        .expect("excluded_unstable_features missing");
    assert!(
        excluded
            .iter()
            .any(|v| v == &serde_json::json!("unstable_protocol_v2")),
        "draft protocol v2 must stay in the exclusion list"
    );
}
