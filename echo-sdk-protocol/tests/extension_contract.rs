//! Extension contract tests: fixtures, catalog and schema invariants.
//!
//! These mirror the golden fixtures exported to
//! `contracts/sdk/fixtures/extension/v1/`: valid samples must round-trip
//! losslessly through the DTO they target, invalid samples must be rejected
//! by serde or the DTO's validation. The catalog and schema boundary rules
//! are enforced mechanically as well, so the exported contract can never
//! drift from the code that defines it.

use echo_sdk_protocol::capability::EchoAgentCapability;
use echo_sdk_protocol::catalog::{self, Direction, METHOD_CATALOG, MethodDescriptor};
use echo_sdk_protocol::error::EchoSdkError;
use echo_sdk_protocol::event::{ReplayResponse, WireEventEnvelope};
use echo_sdk_protocol::handle::WireHandle;
use echo_sdk_protocol::scalar::{WirePath, WireU64};
use echo_sdk_protocol::schema::{
    self, FixtureKind, build_extension_schema_doc, validate_schema_boundaries,
};

fn fixtures_of(target: &str) -> Vec<schema::Fixture> {
    schema::all_fixtures()
        .into_iter()
        .filter(|f| f.target == target)
        .collect()
}

#[test]
fn valid_scalar_fixtures_round_trip_losslessly() {
    for fixture in fixtures_of("WireU64")
        .into_iter()
        .filter(|f| f.kind == FixtureKind::Valid)
    {
        let value: WireU64 = serde_json::from_value(fixture.payload.clone())
            .unwrap_or_else(|e| panic!("fixture {} rejected: {e}", fixture.name));
        let back = serde_json::to_value(&value).unwrap_or(serde_json::Value::Null);
        assert_eq!(
            back, fixture.payload,
            "fixture {} is not lossless",
            fixture.name
        );
    }
}

#[test]
fn invalid_scalar_fixtures_are_rejected() {
    for fixture in fixtures_of("WireU64")
        .into_iter()
        .filter(|f| f.kind == FixtureKind::Invalid)
    {
        let parsed: Result<WireU64, _> = serde_json::from_value(fixture.payload.clone());
        assert!(parsed.is_err(), "fixture {} must be rejected", fixture.name);
    }
}

#[test]
fn path_fixtures_round_trip_across_encodings() {
    for fixture in fixtures_of("WirePath") {
        let path: WirePath = serde_json::from_value(fixture.payload.clone())
            .unwrap_or_else(|e| panic!("fixture {} rejected: {e}", fixture.name));
        let back = serde_json::to_value(&path).unwrap_or(serde_json::Value::Null);
        assert_eq!(
            back, fixture.payload,
            "fixture {} is not lossless",
            fixture.name
        );
    }
}

#[test]
fn handle_fixtures_enforce_non_empty_identities() {
    for fixture in fixtures_of("WireHandle") {
        let handle: WireHandle = serde_json::from_value(fixture.payload.clone())
            .unwrap_or_else(|e| panic!("fixture {} rejected: {e}", fixture.name));
        let valid = handle.validate().is_ok();
        match fixture.kind {
            FixtureKind::Valid => assert!(valid, "fixture {} must validate", fixture.name),
            FixtureKind::Invalid => assert!(!valid, "fixture {} must be rejected", fixture.name),
        }
    }
}

#[test]
fn error_fixtures_round_trip_typed_codes() {
    for fixture in fixtures_of("EchoSdkError") {
        let error: EchoSdkError = serde_json::from_value(fixture.payload.clone())
            .unwrap_or_else(|e| panic!("fixture {} rejected: {e}", fixture.name));
        assert!(error.validate().is_ok());
        let back = serde_json::to_value(&error).unwrap_or(serde_json::Value::Null);
        assert_eq!(
            back, fixture.payload,
            "fixture {} is not lossless",
            fixture.name
        );
    }
}

#[test]
fn event_fixtures_preserve_every_envelope_fact() {
    for fixture in fixtures_of("WireEventEnvelope") {
        let envelope: WireEventEnvelope = serde_json::from_value(fixture.payload.clone())
            .unwrap_or_else(|e| panic!("fixture {} rejected: {e}", fixture.name));
        let valid = envelope.validate().is_ok();
        match fixture.kind {
            FixtureKind::Valid => {
                assert!(valid, "fixture {} must validate", fixture.name);
                let back = serde_json::to_value(&envelope).unwrap_or(serde_json::Value::Null);
                assert_eq!(
                    back, fixture.payload,
                    "fixture {} is not lossless",
                    fixture.name
                );
            }
            FixtureKind::Invalid => {
                assert!(!valid, "fixture {} must be rejected", fixture.name);
            }
        }
    }
}

#[test]
fn replay_fixtures_round_trip_with_gap() {
    for fixture in fixtures_of("ReplayResponse") {
        let response: ReplayResponse = serde_json::from_value(fixture.payload.clone())
            .unwrap_or_else(|e| panic!("fixture {} rejected: {e}", fixture.name));
        let back = serde_json::to_value(&response).unwrap_or(serde_json::Value::Null);
        assert_eq!(
            back, fixture.payload,
            "fixture {} is not lossless",
            fixture.name
        );
        assert!(
            response.gap.is_some(),
            "fixture {} should carry a gap",
            fixture.name
        );
    }
}

#[test]
fn capability_fixtures_validate_shape_rules() {
    for fixture in fixtures_of("EchoAgentCapability") {
        let capability: EchoAgentCapability = serde_json::from_value(fixture.payload.clone())
            .unwrap_or_else(|e| panic!("fixture {} rejected: {e}", fixture.name));
        let problems = capability.validate_shape();
        match fixture.kind {
            FixtureKind::Valid => {
                assert!(
                    problems.is_empty(),
                    "fixture {} invalid: {problems:?}",
                    fixture.name
                );
            }
            FixtureKind::Invalid => {
                assert!(
                    !problems.is_empty(),
                    "fixture {} must be rejected by shape validation",
                    fixture.name
                );
            }
        }
    }
}

#[test]
fn boundary_fixtures_reject_standard_and_foreign_methods() {
    for fixture in fixtures_of("MethodCatalog") {
        let method = fixture
            .payload
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let in_extension = METHOD_CATALOG.iter().any(|m| m.name == method);
        // Neither a standard ACP method nor an un-namespaced method may be
        // part of the extension contract; the first is owned by the official
        // schema, the second violates the `_echo_agent/` prefix rule.
        assert!(
            !in_extension,
            "fixture {} names a method inside the extension catalog",
            fixture.name
        );
        let is_standard = catalog::STANDARD_ACP_METHODS.contains(&method);
        let is_namespaced = method.starts_with(echo_sdk_protocol::EXTENSION_NAMESPACE);
        // "session/prompt" is standard-owned; "run/start" is un-namespaced.
        // Both must stay outside the extension surface.
        assert!(
            !in_extension && !is_namespaced && (is_standard || !is_namespaced),
            "fixture {} must stay outside the extension contract",
            fixture.name
        );
    }
}

#[test]
fn catalog_passes_mechanical_validation() {
    assert!(catalog::validate_catalog(METHOD_CATALOG).is_empty());
    // Every catalog entry is uniquely named and namespaced.
    let mut names = std::collections::BTreeSet::new();
    for method in METHOD_CATALOG {
        assert!(names.insert(method.name), "duplicate {}", method.name);
        assert!(method.name.starts_with("_echo_agent/"));
    }
}

#[test]
fn schema_document_is_deterministic_and_within_boundaries() {
    let first = build_extension_schema_doc();
    let second = build_extension_schema_doc();
    assert_eq!(first, second);
    assert!(validate_schema_boundaries(&first).is_empty());
    // The catalog embedded in the schema lists every method with direction.
    let embedded = first
        .get("method_catalog")
        .and_then(|v| v.as_array())
        .expect("method_catalog embedded");
    assert_eq!(embedded.len(), METHOD_CATALOG.len());
}

#[test]
fn digest_covers_schema_and_catalog() {
    let digest = schema::extension_contract_digest();
    assert_eq!(digest, schema::extension_contract_digest());
    // Any catalog change must move the digest: prove sensitivity by feeding a
    // mutated catalog document through the same digest function.
    let mut mutated = build_extension_schema_doc();
    if let Some(catalog_value) = mutated
        .get_mut("method_catalog")
        .and_then(|v| v.as_array_mut())
    {
        catalog_value.push(serde_json::json!({
            "name": "_echo_agent/probe/extra",
            "direction": Direction::Request.as_str(),
            "capability": "runs",
            "summary": "digest sensitivity probe",
        }));
    }
    assert_ne!(digest, schema::digest_of(&mutated));
}

#[test]
fn fixtures_directory_content_matches_in_memory_set() {
    // The committed fixture files must be exactly what the code generates;
    // the export tool enforces byte equality, here we check structure.
    let files = schema::build_fixtures();
    assert!(!files.is_empty());
    let all = schema::all_fixtures();
    let names: Vec<&str> = all.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(files.len(), names.len());
    for (path, content) in &files {
        assert!(path.starts_with("contracts/sdk/fixtures/extension/v1/"));
        let parsed: Result<schema::Fixture, _> = serde_json::from_str(content);
        assert!(parsed.is_ok(), "fixture file {path} not parseable");
    }
}

#[test]
fn catalog_helper_detects_violations() {
    let bad = vec![MethodDescriptor {
        name: "session/prompt",
        direction: Direction::Request,
        capability: echo_sdk_protocol::capability::ExtensionCapability::Runs,
        summary: "",
    }];
    let problems = catalog::validate_catalog(&bad);
    assert!(problems.iter().any(|p| p.contains("underscore")));
    assert!(problems.iter().any(|p| p.contains("standard ACP method")));
}
