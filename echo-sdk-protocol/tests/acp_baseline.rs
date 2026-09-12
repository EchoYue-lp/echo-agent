//! Stable ACP v1 artifact, method-surface and feature-boundary checks.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const EXPECTED_WIRE_VERSION: u16 = 1;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn baseline() -> TestResult<serde_json::Value> {
    Ok(serde_json::from_str(&std::fs::read_to_string(
        repo_root().join("contracts/sdk/acp-baseline.json"),
    )?)?)
}

#[test]
fn official_latest_stable_wire_version_is_v1() {
    let latest = agent_client_protocol_schema::ProtocolVersion::LATEST;
    assert_eq!(latest.as_u16(), EXPECTED_WIRE_VERSION);
    assert_eq!(latest, agent_client_protocol_schema::ProtocolVersion::V1);
}

#[test]
fn draft_v2_module_is_not_compiled_in() {
    let initialize = agent_client_protocol_schema::v1::InitializeRequest::new(
        agent_client_protocol_schema::ProtocolVersion::V1,
    );
    let json = serde_json::to_value(&initialize).unwrap_or(serde_json::Value::Null);
    assert_eq!(json.get("protocolVersion"), Some(&serde_json::json!(1)));
}

#[test]
fn lockfile_matches_pinned_baseline() -> TestResult {
    let lock = std::fs::read_to_string(repo_root().join("Cargo.lock"))?;
    let baseline = baseline()?;
    let wire = baseline
        .get("acp_wire_protocol_version")
        .and_then(serde_json::Value::as_u64)
        .ok_or("acp_wire_protocol_version missing")?;
    assert_eq!(wire, u64::from(EXPECTED_WIRE_VERSION));

    let mut locked: BTreeMap<String, (String, Option<String>)> = BTreeMap::new();
    for block in lock.split("[[package]]") {
        let field = |name: &str| -> Option<String> {
            block.lines().find_map(|line| {
                line.trim()
                    .strip_prefix(&format!("{name} = "))
                    .map(|value| value.trim_matches('"').to_string())
            })
        };
        if let (Some(name), Some(version)) = (field("name"), field("version")) {
            locked.insert(name, (version, field("checksum")));
        }
    }

    let pinned = baseline
        .get("official_crates")
        .and_then(serde_json::Value::as_object)
        .ok_or("baseline.official_crates missing")?;
    assert!(!pinned.is_empty(), "baseline pins no crates");
    for (crate_name, expected) in pinned {
        let expected = expected
            .as_str()
            .ok_or_else(|| format!("version for {crate_name} is not a string"))?;
        let (version, _) = locked
            .get(crate_name)
            .ok_or_else(|| format!("crate {crate_name} absent from Cargo.lock"))?;
        assert_eq!(
            version, expected,
            "Cargo.lock version drift for {crate_name}"
        );
    }

    let checksums = baseline
        .get("official_checksums")
        .and_then(serde_json::Value::as_object)
        .ok_or("baseline.official_checksums missing")?;
    for (crate_name, expected) in checksums {
        let expected = expected
            .as_str()
            .ok_or_else(|| format!("checksum for {crate_name} is not a string"))?;
        let checksum = locked
            .get(crate_name)
            .and_then(|(_, checksum)| checksum.as_deref())
            .ok_or_else(|| format!("checksum for {crate_name} absent from Cargo.lock"))?;
        assert_eq!(
            checksum, expected,
            "Cargo.lock checksum drift for {crate_name}"
        );
    }

    let schema_version = baseline
        .pointer("/schema_artifact/version")
        .and_then(serde_json::Value::as_str)
        .ok_or("schema_artifact.version missing")?;
    let locked_schema = locked
        .get("agent-client-protocol-schema")
        .ok_or("schema crate absent from Cargo.lock")?;
    assert_eq!(locked_schema.0, schema_version, "schema artifact drift");
    Ok(())
}

#[test]
fn stable_method_surface_matches_official_v1_constants() -> TestResult {
    let baseline = baseline()?;
    let pinned: BTreeSet<&str> = baseline
        .get("stable_v1_methods")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .collect();
    assert_eq!(
        pinned,
        echo_sdk_protocol::catalog::official_acp_v1_methods(),
        "official ACP v1 method surface drifted"
    );
    Ok(())
}

#[test]
fn excluded_unstable_features_are_declared() -> TestResult {
    let baseline = baseline()?;
    let actual: BTreeSet<&str> = baseline
        .get("excluded_unstable_features")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .collect();
    let expected: BTreeSet<&str> = [
        "unstable",
        "unstable_end_turn_token_usage",
        "unstable_llm_providers",
        "unstable_mcp_over_acp",
        "unstable_nes",
        "unstable_plan_operations",
        "unstable_protocol_v2",
        "unstable_session_compaction",
        "unstable_session_fork",
        "unstable_tool_call_name",
    ]
    .into_iter()
    .collect();
    assert_eq!(actual, expected, "unstable feature exclusion set drifted");
    Ok(())
}
