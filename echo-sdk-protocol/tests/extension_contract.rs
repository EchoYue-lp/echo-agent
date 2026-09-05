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
use echo_sdk_protocol::event::{GapNotification, ReplayRequest, ReplayResponse, WireEventEnvelope};
use echo_sdk_protocol::handle::WireHandle;
use echo_sdk_protocol::methods::{ExtensionInvokeOutcome, ExtensionStreamEvent};
use echo_sdk_protocol::scalar::{WireBytes, WirePath, WireU64, WireValue};
use echo_sdk_protocol::schema::{
    self, FixtureKind, build_extension_schema_doc, build_extension_validator,
    validate_schema_boundaries,
};

fn fixtures_of(target: &str) -> Vec<schema::Fixture> {
    schema::all_fixtures()
        .into_iter()
        .filter(|f| f.target == target)
        .collect()
}

fn parse_fixture<T: serde::de::DeserializeOwned>(fixture: &schema::Fixture) -> Result<T, String> {
    serde_json::from_value(fixture.payload.clone())
        .map_err(|error| format!("fixture {} rejected: {error}", fixture.name))
}

#[test]
fn valid_scalar_fixtures_round_trip_losslessly() -> Result<(), String> {
    for fixture in fixtures_of("WireU64")
        .into_iter()
        .filter(|f| f.kind == FixtureKind::Valid)
    {
        let value: WireU64 = parse_fixture(&fixture)?;
        let back = serde_json::to_value(&value).unwrap_or(serde_json::Value::Null);
        assert_eq!(
            back, fixture.payload,
            "fixture {} is not lossless",
            fixture.name
        );
    }
    Ok(())
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
fn every_invalid_fixture_declares_a_stable_error_code() {
    let allowed = ["invalid_value", "invalid_request"];
    for fixture in schema::all_fixtures()
        .into_iter()
        .filter(|fixture| fixture.kind == FixtureKind::Invalid)
    {
        let code = fixture.expect_error.as_deref().unwrap_or_default();
        assert!(
            allowed.contains(&code),
            "fixture {} has invalid error code",
            fixture.name
        );
    }
}

#[test]
fn path_fixtures_round_trip_across_encodings() -> Result<(), String> {
    for fixture in fixtures_of("WirePath") {
        let parsed: Result<WirePath, _> = serde_json::from_value(fixture.payload.clone());
        match (fixture.kind, parsed) {
            (FixtureKind::Valid, Ok(path)) => {
                assert!(path.validate().is_ok(), "fixture {} invalid", fixture.name);
                let back = serde_json::to_value(&path).unwrap_or(serde_json::Value::Null);
                assert_eq!(
                    back, fixture.payload,
                    "fixture {} is not lossless",
                    fixture.name
                );
            }
            (FixtureKind::Invalid, Ok(path)) => {
                assert!(
                    path.validate().is_err(),
                    "fixture {} must fail validation",
                    fixture.name
                );
            }
            (FixtureKind::Invalid, Err(_)) => {}
            (FixtureKind::Valid, Err(error)) => {
                return Err(format!("fixture {} rejected: {error}", fixture.name));
            }
        }
    }
    Ok(())
}

#[test]
fn binary_and_unknown_fixtures_enforce_bounds() {
    for fixture in fixtures_of("WireBytes") {
        let value: WireBytes =
            serde_json::from_value(fixture.payload.clone()).unwrap_or(WireBytes {
                base64: String::new(),
            });
        assert_eq!(value.validate().is_ok(), fixture.kind == FixtureKind::Valid);
    }
    for fixture in fixtures_of("WireValue") {
        let value: Result<WireValue, _> = serde_json::from_value(fixture.payload.clone());
        let valid = value.as_ref().is_ok_and(|value| value.validate().is_ok());
        assert_eq!(valid, fixture.kind == FixtureKind::Valid);
    }
}

#[test]
fn extension_outcome_and_stream_are_tagged() {
    for fixture in fixtures_of("ExtensionInvokeOutcome") {
        let parsed: Result<ExtensionInvokeOutcome, _> =
            serde_json::from_value(fixture.payload.clone());
        let valid = parsed
            .as_ref()
            .is_ok_and(|outcome| outcome.validate().is_ok());
        assert_eq!(valid, fixture.kind == FixtureKind::Valid);
    }
    for fixture in fixtures_of("ExtensionStreamEvent") {
        let parsed: Result<ExtensionStreamEvent, _> =
            serde_json::from_value(fixture.payload.clone());
        let valid = parsed.as_ref().is_ok_and(|event| event.validate().is_ok());
        assert_eq!(valid, fixture.kind == FixtureKind::Valid);
    }
}

#[test]
fn handle_fixtures_enforce_non_empty_identities() -> Result<(), String> {
    for fixture in fixtures_of("WireHandle") {
        let handle: WireHandle = parse_fixture(&fixture)?;
        let valid = handle.validate().is_ok();
        match fixture.kind {
            FixtureKind::Valid => assert!(valid, "fixture {} must validate", fixture.name),
            FixtureKind::Invalid => assert!(!valid, "fixture {} must be rejected", fixture.name),
        }
    }
    Ok(())
}

#[test]
fn error_fixtures_round_trip_typed_codes() -> Result<(), String> {
    for fixture in fixtures_of("EchoSdkError") {
        let error: EchoSdkError = parse_fixture(&fixture)?;
        let valid = error.validate().is_ok();
        assert_eq!(valid, fixture.kind == FixtureKind::Valid);
        if valid {
            let back = serde_json::to_value(&error).unwrap_or(serde_json::Value::Null);
            assert_eq!(
                back, fixture.payload,
                "fixture {} is not lossless",
                fixture.name
            );
        }
    }
    Ok(())
}

#[test]
fn event_fixtures_preserve_every_envelope_fact() -> Result<(), String> {
    for fixture in fixtures_of("WireEventEnvelope") {
        let parsed: Result<WireEventEnvelope, _> = serde_json::from_value(fixture.payload.clone());
        match (fixture.kind, parsed) {
            (FixtureKind::Valid, Ok(envelope)) => {
                let valid = envelope.validate().is_ok();
                assert!(valid, "fixture {} must validate", fixture.name);
                let back = serde_json::to_value(&envelope).unwrap_or(serde_json::Value::Null);
                assert_eq!(
                    back, fixture.payload,
                    "fixture {} is not lossless",
                    fixture.name
                );
            }
            (FixtureKind::Invalid, Ok(envelope)) => {
                assert!(
                    envelope.validate().is_err(),
                    "fixture {} must be rejected",
                    fixture.name
                );
            }
            (FixtureKind::Invalid, Err(_)) => {}
            (FixtureKind::Valid, Err(error)) => {
                return Err(format!("fixture {} rejected: {error}", fixture.name));
            }
        }
    }
    Ok(())
}

#[test]
fn replay_fixtures_round_trip_with_gap() -> Result<(), String> {
    for fixture in fixtures_of("ReplayResponse") {
        let response: ReplayResponse = parse_fixture(&fixture)?;
        assert!(response.validate().is_ok());
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
    Ok(())
}

#[test]
fn live_gap_identifies_its_stream() -> Result<(), String> {
    for fixture in fixtures_of("GapNotification") {
        let notification: GapNotification = parse_fixture(&fixture)?;
        assert_eq!(
            notification.validate().is_ok(),
            fixture.kind == FixtureKind::Valid
        );
    }
    Ok(())
}

#[test]
fn replay_requests_validate_stream_and_bounds() -> Result<(), String> {
    for fixture in fixtures_of("ReplayRequest") {
        let parsed: Result<ReplayRequest, _> = serde_json::from_value(fixture.payload.clone());
        let valid = parsed
            .as_ref()
            .is_ok_and(|request| request.validate().is_ok());
        assert_eq!(valid, fixture.kind == FixtureKind::Valid);
    }
    Ok(())
}

#[test]
fn capability_fixtures_validate_shape_rules() -> Result<(), String> {
    for fixture in fixtures_of("EchoAgentCapability") {
        let capability: EchoAgentCapability = parse_fixture(&fixture)?;
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
    Ok(())
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
        let is_standard = catalog::official_acp_v1_methods().contains(method);
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
fn reverse_callback_stream_directions_are_consistent() {
    let direction = |name: &str| {
        METHOD_CATALOG
            .iter()
            .find(|method| method.name == name)
            .map(|method| method.direction)
    };
    assert_eq!(
        direction("_echo_agent/extension/invoke"),
        Some(Direction::ReverseRequest)
    );
    assert_eq!(
        direction("_echo_agent/extension/stream"),
        Some(Direction::ClientNotification)
    );
    assert_eq!(
        direction("_echo_agent/extension/cancel"),
        Some(Direction::HostNotification)
    );
}

#[test]
fn schema_document_is_deterministic_and_within_boundaries() -> Result<(), String> {
    let first = build_extension_schema_doc();
    let second = build_extension_schema_doc();
    assert_eq!(first, second);
    assert!(validate_schema_boundaries(&first).is_empty());
    // The catalog embedded in the schema lists every method with direction.
    let embedded = first
        .get("method_catalog")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "method_catalog missing".to_string())?;
    assert_eq!(embedded.len(), METHOD_CATALOG.len());
    let validator = build_extension_validator(&first);
    assert!(
        validator.is_ok(),
        "generated schema has unresolved references"
    );
    let definitions = first
        .get("definitions")
        .and_then(|value| value.as_object())
        .ok_or_else(|| "definitions missing".to_string())?;
    for method in METHOD_CATALOG {
        assert!(definitions.contains_key(method.params_schema()));
        if let Some(result) = method.result_schema() {
            assert!(definitions.contains_key(result));
        }
        if let Some(error) = method.error_schema() {
            assert!(definitions.contains_key(error));
        }
    }
    Ok(())
}

#[test]
fn schema_rejects_structurally_invalid_fixtures() -> Result<(), String> {
    let document = build_extension_schema_doc();
    let definitions = document
        .get("definitions")
        .cloned()
        .ok_or_else(|| "definitions missing".to_string())?;
    let schema_checked = [
        "scalar-u64-leading-zeros-invalid",
        "scalar-u64-float-invalid",
        "scalar-u64-overflow-invalid",
        "scalar-path-relative-invalid",
        "scalar-path-base64-invalid",
        "scalar-path-unix-relative-encoded-invalid",
        "scalar-path-windows-relative-encoded-invalid",
        "scalar-path-native-empty-invalid",
        "scalar-bytes-base64-invalid",
        "scalar-bytes-length-invalid",
        "scalar-unknown-empty-tag-invalid",
        "handle-empty-id-invalid",
        "event-envelope-sequence-zero-invalid",
        "event-replay-empty-stream-invalid",
        "event-replay-zero-limit-invalid",
        "extension-outcome-empty-invalid",
        "extension-outcome-ambiguous-invalid",
        "extension-stream-sequence-zero-invalid",
    ];
    for fixture in schema::all_fixtures()
        .into_iter()
        .filter(|fixture| schema_checked.contains(&fixture.name.as_str()))
    {
        let target_schema = serde_json::json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "$ref": format!("#/definitions/{}", fixture.target),
            "definitions": definitions.clone(),
        });
        let validator = build_extension_validator(&target_schema)
            .map_err(|error| format!("schema for {} failed: {error}", fixture.target))?;
        assert!(
            !validator.is_valid(&fixture.payload),
            "schema accepted invalid fixture {}",
            fixture.name
        );
    }
    Ok(())
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
