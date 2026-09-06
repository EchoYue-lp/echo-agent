//! Extension schema, digest and golden fixtures.
//!
//! The extension contract is generated from the Rust DTOs and the method
//! catalog — never hand-written — so the schema cannot drift from the code.
//! The generator is deterministic: the same source revision produces the
//! same canonical JSON, the same sha256 contract digest and the same
//! fixture bytes (design §20.2: regeneration leaves the worktree clean).
//!
//! Only echo-agent extension types appear here. Official ACP schemas
//! (JSON-RPC envelope, initialize, Session, Prompt, ContentBlock, stop
//! reasons) stay external: `validate_schema_boundaries` rejects any
//! generated definition that would shadow an official ACP concept.

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::capability::{EchoAgentCapability, EchoAgentClientHello};
use crate::catalog::METHOD_CATALOG;
use crate::error::{
    AgentFailureWire, EchoSdkError, ErrorDetails, ExtensionErrorCode, Retryability,
};
use crate::event::{
    EventCursor, EventGap, EventNotification, GapNotification, ReplayRequest, ReplayResponse,
    WireEventEnvelope, WireEventPayload,
};
use crate::handle::{HandleKind, WireHandle};
use crate::methods::*;
use crate::scalar::{
    ABSOLUTE_UNIX_PATH_FORMAT, ABSOLUTE_UTF8_PATH_FORMAT, ABSOLUTE_WINDOWS_PATH_FORMAT,
    BASE64_NO_PAD_FORMAT, WireBytes, WireDuration, WireField, WireI64, WireMapEntry,
    WireNonZeroU64, WirePath, WireTimestamp, WireU64, WireValue, is_absolute_unix_path_base64,
    is_absolute_utf8_path, is_absolute_windows_path_base64, is_base64_no_pad,
};

/// Names that would signal the schema is re-defining official ACP concepts.
/// The generated definitions must not contain any of these titles.
const FORBIDDEN_ACP_TITLES: &[&str] = &[
    "JsonRpcMessage",
    "JsonRpcRequest",
    "JsonRpcResponse",
    "JsonRpcNotification",
    "InitializeRequest",
    "InitializeResponse",
    "Session",
    "SessionNewRequest",
    "SessionPromptRequest",
    "ContentBlock",
    "StopReason",
    "ProtocolVersion",
];

/// Canonical JSON rendering: `serde_json` maps are BTree-ordered here (the
/// workspace does not enable `preserve_order`), so pretty printing yields a
/// byte-stable document.
pub fn canonical_json(value: &serde_json::Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default()
}

/// sha256 digest over the canonical document, hex-encoded.
pub fn digest_of(value: &serde_json::Value) -> String {
    let canonical = canonical_json(value);
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    format!("sha256:{}", hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    const TABLE: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(TABLE[(byte >> 4) as usize] as char);
        out.push(TABLE[(byte & 0x0f) as usize] as char);
    }
    out
}

fn insert_schema<T: schemars::JsonSchema>(
    definitions: &mut serde_json::Map<String, serde_json::Value>,
    name: &str,
) {
    let schema = schemars::schema_for!(T);
    let mut value = serde_json::to_value(&schema).unwrap_or(serde_json::Value::Null);
    if let Some(object) = value.as_object_mut() {
        object.remove("$schema");
        if let Some(nested) = object
            .remove("definitions")
            .and_then(|definitions| definitions.as_object().cloned())
        {
            for (nested_name, nested_schema) in nested {
                definitions.entry(nested_name).or_insert(nested_schema);
            }
        }
    }
    definitions.insert(name.to_string(), value);
}

macro_rules! schema_entry {
    ($map:expr, $ty:ty) => {
        insert_schema::<$ty>(&mut $map, stringify!($ty));
    };
}

/// Build the full extension schema document.
pub fn build_extension_schema_doc() -> serde_json::Value {
    let mut definitions = serde_json::Map::new();
    // Scalars.
    schema_entry!(definitions, WireU64);
    schema_entry!(definitions, WireNonZeroU64);
    schema_entry!(definitions, WireI64);
    schema_entry!(definitions, WireDuration);
    schema_entry!(definitions, WireTimestamp);
    schema_entry!(definitions, WirePath);
    schema_entry!(definitions, WireBytes);
    schema_entry!(definitions, WireField);
    schema_entry!(definitions, WireMapEntry);
    schema_entry!(definitions, WireValue);
    // Handles.
    schema_entry!(definitions, WireHandle);
    schema_entry!(definitions, HandleKind);
    // Errors.
    schema_entry!(definitions, EchoSdkError);
    schema_entry!(definitions, ExtensionErrorCode);
    schema_entry!(definitions, Retryability);
    schema_entry!(definitions, ErrorDetails);
    // Events.
    schema_entry!(definitions, WireEventEnvelope);
    schema_entry!(definitions, WireEventPayload);
    schema_entry!(definitions, EventNotification);
    schema_entry!(definitions, EventCursor);
    schema_entry!(definitions, ReplayRequest);
    schema_entry!(definitions, ReplayResponse);
    schema_entry!(definitions, EventGap);
    schema_entry!(definitions, GapNotification);
    // Capability.
    schema_entry!(definitions, EchoAgentCapability);
    schema_entry!(definitions, EchoAgentClientHello);
    // Core profile typed payloads (agent config / run input / terminal /
    // receipt / recovery descriptions).
    schema_entry!(definitions, AgentConfigWire);
    schema_entry!(definitions, AgentConfigExplicitWire);
    schema_entry!(definitions, ModelConfigWire);
    schema_entry!(definitions, LlmApiProtocolWire);
    schema_entry!(definitions, CredentialSourceWire);
    schema_entry!(definitions, AgentSettingsWire);
    schema_entry!(definitions, AgentSnapshotWire);
    schema_entry!(definitions, AgentFailureWire);
    schema_entry!(definitions, RunStatus);
    schema_entry!(definitions, RunTerminal);
    schema_entry!(definitions, RunReceiptWire);
    schema_entry!(definitions, RecoveredRunWire);
    schema_entry!(definitions, EventAck);
    schema_entry!(definitions, EventAckNotification);
    // Method payloads (request/response DTOs of the frozen catalog families).
    schema_entry!(definitions, AgentCreateRequest);
    schema_entry!(definitions, AgentCreateResponse);
    schema_entry!(definitions, AgentDescribeRequest);
    schema_entry!(definitions, AgentDescribeResponse);
    schema_entry!(definitions, AgentCloseRequest);
    schema_entry!(definitions, AgentCloseResponse);
    schema_entry!(definitions, SessionCreateRequest);
    schema_entry!(definitions, SessionCreateResponse);
    schema_entry!(definitions, SessionLoadRequest);
    schema_entry!(definitions, SessionLoadResponse);
    schema_entry!(definitions, SessionCloseRequest);
    schema_entry!(definitions, SessionCloseResponse);
    schema_entry!(definitions, RunInput);
    schema_entry!(definitions, RunStartRequest);
    schema_entry!(definitions, RunStartResponse);
    schema_entry!(definitions, RunGetRequest);
    schema_entry!(definitions, RunGetResponse);
    schema_entry!(definitions, RunWaitRequest);
    schema_entry!(definitions, RunWaitResponse);
    schema_entry!(definitions, RunCancelRequest);
    schema_entry!(definitions, RunCancelResponse);
    schema_entry!(definitions, RunSteerRequest);
    schema_entry!(definitions, RunSteerResponse);
    schema_entry!(definitions, TaskCreateRequest);
    schema_entry!(definitions, TaskCreateResponse);
    schema_entry!(definitions, TaskUpdateRequest);
    schema_entry!(definitions, TaskUpdateResponse);
    schema_entry!(definitions, TaskListRequest);
    schema_entry!(definitions, TaskListResponse);
    schema_entry!(definitions, TaskSummary);
    schema_entry!(definitions, TaskExecuteRequest);
    schema_entry!(definitions, TaskExecuteResponse);
    schema_entry!(definitions, TaskControlRequest);
    schema_entry!(definitions, TaskControlResponse);
    schema_entry!(definitions, SubagentDispatchRequest);
    schema_entry!(definitions, SubagentDispatchResponse);
    schema_entry!(definitions, SubagentAwaitRequest);
    schema_entry!(definitions, SubagentAwaitResponse);
    schema_entry!(definitions, SubagentControlRequest);
    schema_entry!(definitions, SubagentControlResponse);
    schema_entry!(definitions, ExtensionRegisterRequest);
    schema_entry!(definitions, ExtensionRegisterResponse);
    schema_entry!(definitions, ExtensionUnregisterRequest);
    schema_entry!(definitions, ExtensionUnregisterResponse);
    schema_entry!(definitions, ExtensionInvokeCall);
    schema_entry!(definitions, ExtensionInvokeOutcome);
    schema_entry!(definitions, ExtensionCancelNotice);
    schema_entry!(definitions, ExtensionStreamEvent);
    schema_entry!(definitions, FeatureOperationRequest);
    schema_entry!(definitions, FeatureOperationResponse);
    schema_entry!(definitions, WorkingDirectory);

    let catalog: Vec<serde_json::Value> = METHOD_CATALOG
        .iter()
        .map(|method| {
            serde_json::json!({
                "name": method.name,
                "direction": method.direction.as_str(),
                "capability": method.capability.as_str(),
                "summary": method.summary,
                "params_schema": format!("#/definitions/{}", method.params_schema()),
                "result_schema": method
                    .result_schema()
                    .map(|name| format!("#/definitions/{name}")),
                "error_schema": method
                    .error_schema()
                    .map(|name| format!("#/definitions/{name}")),
            })
        })
        .collect();

    serde_json::json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "title": "echo-agent-extension-v1",
        "type": "object",
        "extension_protocol_version": crate::EXTENSION_PROTOCOL_VERSION,
        "namespace": crate::EXTENSION_NAMESPACE,
        "acp_wire_protocol_version": 1,
        "description": "echo-agent SDK extension contract over stable ACP v1. \
                        Standard ACP types are owned by the official \
                        agent-client-protocol-schema artifact and are intentionally \
                        absent from this document.",
        "method_catalog": catalog,
        "definitions": definitions,
    })
}

pub fn build_extension_validator(
    schema: &serde_json::Value,
) -> Result<jsonschema::Validator, jsonschema::ValidationError<'static>> {
    jsonschema::options()
        .with_format(BASE64_NO_PAD_FORMAT, is_base64_no_pad)
        .with_format(ABSOLUTE_UNIX_PATH_FORMAT, is_absolute_unix_path_base64)
        .with_format(
            ABSOLUTE_WINDOWS_PATH_FORMAT,
            is_absolute_windows_path_base64,
        )
        .with_format(ABSOLUTE_UTF8_PATH_FORMAT, is_absolute_utf8_path)
        .build(schema)
}

/// Contract digest of the extension schema (design §18: extension version,
/// wire version and crate versions are governed independently).
pub fn extension_contract_digest() -> String {
    digest_of(&build_extension_schema_doc())
}

// ── Source contract ─────────────────────────────────────────────────────────

/// Algorithm identifier of the source-compatibility digest.
pub const SOURCE_CONTRACT_ALGORITHM: &str = "sha256-length-prefixed-v1";

/// Fixed input order of the source contract. The Host embeds only the small
/// generated document — never the large inventory artifacts themselves.
pub const SOURCE_CONTRACT_INPUTS: &[&str] = &[
    "Cargo.lock",
    "contracts/sdk/public-api.txt",
    "contracts/sdk/parity-manifest.json",
];

/// Aggregate digest over the source-contract inputs: for every entry in the
/// fixed [`SOURCE_CONTRACT_INPUTS`] order, hash the u64 big-endian length of
/// the relative path, the path bytes, the u64 big-endian content length, and
/// the content bytes. Length prefixes make the stream unambiguous without
/// relying on delimiters.
pub fn aggregate_source_digest(entries: &[(&str, &[u8])]) -> String {
    let mut hasher = Sha256::new();
    for (path, content) in entries {
        let path_bytes = path.as_bytes();
        hasher.update((path_bytes.len() as u64).to_be_bytes());
        hasher.update(path_bytes);
        hasher.update((content.len() as u64).to_be_bytes());
        hasher.update(content);
    }
    format!("sha256:{}", hex(&hasher.finalize()))
}

/// Build the `contracts/sdk/source-contract.json` document. `entries` must
/// be given in the [`SOURCE_CONTRACT_INPUTS`] order; each entry is hashed
/// individually so a drift in one input is attributable.
pub fn build_source_contract_doc(entries: &[(&str, &[u8])]) -> serde_json::Value {
    let items: Vec<serde_json::Value> = entries
        .iter()
        .map(|(path, content)| {
            let mut hasher = Sha256::new();
            hasher.update(content);
            serde_json::json!({
                "path": path,
                "bytes": content.len(),
                "sha256": format!("sha256:{}", hex(&hasher.finalize())),
            })
        })
        .collect();
    serde_json::json!({
        "algorithm": SOURCE_CONTRACT_ALGORITHM,
        "inputs": items,
        "aggregate_digest": aggregate_source_digest(entries),
    })
}

/// Fail if the generated schema would shadow official ACP concepts.
pub fn validate_schema_boundaries(doc: &serde_json::Value) -> Vec<String> {
    let mut problems = Vec::new();
    if let Some(definitions) = doc.get("definitions").and_then(|v| v.as_object()) {
        for title in definitions.keys() {
            if FORBIDDEN_ACP_TITLES.contains(&title.as_str()) {
                problems.push(format!(
                    "definition {title} shadows an official ACP schema concept"
                ));
            }
        }
    }
    if let Some(catalog) = doc.get("method_catalog").and_then(|v| v.as_array()) {
        for method in catalog {
            let name = method
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            if !name.starts_with(crate::EXTENSION_NAMESPACE) {
                problems.push(format!(
                    "catalog method {name} outside the extension namespace"
                ));
            }
        }
    }
    problems
}

// ── Fixtures ────────────────────────────────────────────────────────────────

/// One golden fixture. Valid fixtures must round-trip losslessly through the
/// named DTO; invalid fixtures must be rejected by the DTO's validation (or
/// serde) with the expected error family.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Fixture {
    pub name: String,
    pub kind: FixtureKind,
    /// Which DTO the fixture targets (Rust type name).
    pub target: String,
    pub description: String,
    pub payload: serde_json::Value,
    /// For invalid fixtures: the expected failure family.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expect_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FixtureKind {
    Valid,
    Invalid,
}

/// Render the fixture directory content: `(relative_path, canonical_json)`.
pub fn build_fixtures() -> Vec<(String, String)> {
    let mut files = Vec::new();
    for fixture in all_fixtures() {
        let value = serde_json::to_value(&fixture).unwrap_or(serde_json::Value::Null);
        let path = format!("contracts/sdk/fixtures/extension/v1/{}.json", fixture.name);
        files.push((path, canonical_json(&value)));
    }
    files.sort();
    files
}

/// The full fixture set. Ordering is part of the contract; keep entries
/// grouped by DTO family.
pub fn all_fixtures() -> Vec<Fixture> {
    let mut fixtures = Vec::new();
    push_scalar_fixtures(&mut fixtures);
    push_handle_fixtures(&mut fixtures);
    push_error_fixtures(&mut fixtures);
    push_event_fixtures(&mut fixtures);
    push_ack_fixtures(&mut fixtures);
    push_bridge_fixtures(&mut fixtures);
    push_capability_fixtures(&mut fixtures);
    push_hello_fixtures(&mut fixtures);
    push_agent_config_fixtures(&mut fixtures);
    push_run_fixtures(&mut fixtures);
    push_boundary_fixtures(&mut fixtures);
    fixtures
}

fn fixture(
    name: &str,
    kind: FixtureKind,
    target: &str,
    description: &str,
    payload: serde_json::Value,
    expect_error: Option<&str>,
) -> Fixture {
    Fixture {
        name: name.to_string(),
        kind,
        target: target.to_string(),
        description: description.to_string(),
        payload,
        expect_error: expect_error.map(str::to_string),
    }
}

fn push_scalar_fixtures(fixtures: &mut Vec<Fixture>) {
    fixtures.push(fixture(
        "scalar-u64-max-valid",
        FixtureKind::Valid,
        "WireU64",
        "u64::MAX round-trips as a decimal string untouched.",
        serde_json::json!("18446744073709551615"),
        None,
    ));
    fixtures.push(fixture(
        "scalar-u64-leading-zeros-invalid",
        FixtureKind::Invalid,
        "WireU64",
        "Non-canonical decimal spellings are rejected.",
        serde_json::json!("007"),
        Some("invalid_value"),
    ));
    fixtures.push(fixture(
        "scalar-u64-float-invalid",
        FixtureKind::Invalid,
        "WireU64",
        "Floats never substitute for integer strings.",
        serde_json::json!("1.5"),
        Some("invalid_value"),
    ));
    fixtures.push(fixture(
        "scalar-u64-overflow-invalid",
        FixtureKind::Invalid,
        "WireU64",
        "Decimal strings above u64::MAX are rejected by Rust and JSON Schema.",
        serde_json::json!("18446744073709551616"),
        Some("invalid_value"),
    ));
    fixtures.push(fixture(
        "scalar-path-unix-lossless-valid",
        FixtureKind::Valid,
        "WirePath",
        "Non-UTF-8 Unix path bytes survive via base64 with display fallback.",
        serde_json::json!({
            "encoding": "unix",
            "bytes_base64": "L3RtcC/DkfCvMPIh",
            "display": "/tmp/<lossy>"
        }),
        None,
    ));
    fixtures.push(fixture(
        "scalar-path-windows-valid",
        FixtureKind::Valid,
        "WirePath",
        "Windows UTF-16 unit sequence preserved verbatim.",
        serde_json::json!({
            "encoding": "windows",
            "utf16_base64": "QwA6AFwAXAA"
        }),
        None,
    ));
    fixtures.push(fixture(
        "scalar-path-root-valid",
        FixtureKind::Valid,
        "WirePath",
        "Filesystem roots are valid absolute paths.",
        serde_json::json!({"encoding": "utf8", "path": "/"}),
        None,
    ));
    fixtures.push(fixture(
        "scalar-duration-nanos-valid",
        FixtureKind::Valid,
        "WireDuration",
        "Full duration range uses decimal seconds plus sub-second nanos.",
        serde_json::json!({"seconds": "9007199", "nanos": 254740993}),
        None,
    ));
    fixtures.push(fixture(
        "scalar-unknown-additive-valid",
        FixtureKind::Valid,
        "WireValue",
        "Unknown additive values keep their type tag and bounded payload.",
        serde_json::json!({
            "kind": "unknown",
            "value": {
                "type_tag": "agent_event/tool_progress_v2",
                "payload": {"kind": "bool", "value": true}
            }
        }),
        None,
    ));
    fixtures.push(fixture(
        "scalar-path-relative-invalid",
        FixtureKind::Invalid,
        "WirePath",
        "Relative paths are not accepted by the lossless path contract.",
        serde_json::json!({"encoding": "utf8", "path": "relative/file"}),
        Some("invalid_value"),
    ));
    fixtures.push(fixture(
        "scalar-path-base64-invalid",
        FixtureKind::Invalid,
        "WirePath",
        "Encoded native paths must contain canonical base64.",
        serde_json::json!({"encoding": "unix", "bytes_base64": "not base64"}),
        Some("invalid_value"),
    ));
    fixtures.push(fixture(
        "scalar-path-unix-relative-encoded-invalid",
        FixtureKind::Invalid,
        "WirePath",
        "Canonical base64 is still invalid when its Unix path is relative.",
        serde_json::json!({"encoding": "unix", "bytes_base64": "cmVsYXRpdmU"}),
        Some("invalid_value"),
    ));
    fixtures.push(fixture(
        "scalar-path-windows-relative-encoded-invalid",
        FixtureKind::Invalid,
        "WirePath",
        "Canonical UTF-16 base64 is invalid when its Windows path is relative.",
        serde_json::json!({"encoding": "windows", "utf16_base64": "ZgBvAG8A"}),
        Some("invalid_value"),
    ));
    fixtures.push(fixture(
        "scalar-path-native-empty-invalid",
        FixtureKind::Invalid,
        "WirePath",
        "An empty native path cannot be absolute.",
        serde_json::json!({"encoding": "unix", "bytes_base64": ""}),
        Some("invalid_value"),
    ));
    fixtures.push(fixture(
        "scalar-bytes-base64-invalid",
        FixtureKind::Invalid,
        "WireBytes",
        "Binary payloads reject malformed base64.",
        serde_json::json!({"base64": "***"}),
        Some("invalid_value"),
    ));
    fixtures.push(fixture(
        "scalar-bytes-empty-valid",
        FixtureKind::Valid,
        "WireBytes",
        "An empty binary payload has the empty canonical no-pad base64 encoding.",
        serde_json::json!({"base64": ""}),
        None,
    ));
    fixtures.push(fixture(
        "scalar-bytes-length-invalid",
        FixtureKind::Invalid,
        "WireBytes",
        "A one-character no-pad base64 value cannot encode complete bytes.",
        serde_json::json!({"base64": "A"}),
        Some("invalid_value"),
    ));
    fixtures.push(fixture(
        "scalar-unknown-empty-tag-invalid",
        FixtureKind::Invalid,
        "WireValue",
        "Unknown additive values require a non-empty type identity.",
        serde_json::json!({"kind": "unknown", "value": {"type_tag": "", "payload": null}}),
        Some("invalid_value"),
    ));
}

fn push_handle_fixtures(fixtures: &mut Vec<Fixture>) {
    fixtures.push(fixture(
        "handle-run-valid",
        FixtureKind::Valid,
        "WireHandle",
        "Run handle with generation fence.",
        serde_json::json!({"id": "run-42", "generation": "7", "kind": "run"}),
        None,
    ));
    fixtures.push(fixture(
        "handle-empty-id-invalid",
        FixtureKind::Invalid,
        "WireHandle",
        "Empty domain identities are rejected (never borrowed request ids).",
        serde_json::json!({"id": "", "generation": "1", "kind": "agent"}),
        Some("invalid_value"),
    ));
}

fn push_error_fixtures(fixtures: &mut Vec<Fixture>) {
    fixtures.push(fixture(
        "error-stale-handle-valid",
        FixtureKind::Valid,
        "EchoSdkError",
        "Typed stale-handle error with operation identity and retryability.",
        serde_json::json!({
            "code": "stale_handle",
            "message": "run handle predates host restart",
            "retryable": "never",
            "operation": "run/start",
            "handle": {"id": "run-9", "generation": "1", "kind": "run"}
        }),
        None,
    ));
    fixtures.push(fixture(
        "error-feature-unavailable-valid",
        FixtureKind::Valid,
        "EchoSdkError",
        "Missing compiled feature fails closed.",
        serde_json::json!({
            "code": "feature_unavailable",
            "message": "feature `mcp` is not compiled into this host",
            "retryable": "never",
            "operation": "memory/op"
        }),
        None,
    ));
    fixtures.push(fixture(
        "error-event-gap-valid",
        FixtureKind::Valid,
        "EchoSdkError",
        "Gap errors carry the snapshot watermark.",
        serde_json::json!({
            "code": "event_gap",
            "message": "consumer fell below the retention floor",
            "retryable": "never"
        }),
        None,
    ));
    fixtures.push(fixture(
        "error-details-duplicate-invalid",
        FixtureKind::Invalid,
        "EchoSdkError",
        "Error detail keys are bounded and unique.",
        serde_json::json!({
            "code": "invalid_value",
            "message": "duplicate detail key",
            "retryable": "never",
            "details": {"fields": [
                {"key": "field", "value": "one"},
                {"key": "field", "value": "two"}
            ]}
        }),
        Some("invalid_value"),
    ));
}

fn push_event_fixtures(fixtures: &mut Vec<Fixture>) {
    fixtures.push(fixture(
        "event-envelope-full-valid",
        FixtureKind::Valid,
        "WireEventEnvelope",
        "Complete envelope: every framework identity fact plus typed AgentEvent payload.",
        serde_json::json!({
            "schema_version": 4,
            "event_id": "event-3",
            "content_hash": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "sequence": "3",
            "stream_id": "stream-1",
            "run_id": "run-1",
            "turn_id": "turn-1",
            "timestamp": {"unix_seconds": "1757000000", "nanos": 0},
            "payload": {
                "event_type": "token",
                "data": {"kind": "string", "value": "hello"}
            }
        }),
        None,
    ));
    fixtures.push(fixture(
        "event-envelope-sequence-zero-invalid",
        FixtureKind::Invalid,
        "WireEventEnvelope",
        "Framework sequences start at one; zero is rejected.",
        serde_json::json!({
            "schema_version": 4,
            "event_id": "event-0",
            "content_hash": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "sequence": "0",
            "stream_id": "stream-1",
            "turn_id": "turn-1",
            "timestamp": {"unix_seconds": "0", "nanos": 1},
            "payload": {"event_type": "think_start"}
        }),
        Some("invalid_value"),
    ));
    fixtures.push(fixture(
        "event-replay-gap-valid",
        FixtureKind::Valid,
        "ReplayResponse",
        "Replay below the watermark returns bounded events plus a typed gap.",
        serde_json::json!({
            "requested_after_sequence": "4",
            "events": [],
            "next_cursor": {"stream_id": "stream-1", "last_processed_sequence": "9"},
            "gap": {
                "from_sequence": "5",
                "to_sequence": "9",
                "reason": "retention floor",
                "snapshot_watermark": "9"
            }
        }),
        None,
    ));
    fixtures.push(fixture(
        "event-live-gap-valid",
        FixtureKind::Valid,
        "GapNotification",
        "A live gap is explicitly associated with one stream handle.",
        serde_json::json!({
            "stream": {"id": "stream-1", "generation": "3", "kind": "stream"},
            "gap": {
                "from_sequence": "5",
                "to_sequence": "9",
                "reason": "retention floor",
                "snapshot_watermark": "9"
            }
        }),
        None,
    ));
    fixtures.push(fixture(
        "event-live-gap-watermark-invalid",
        FixtureKind::Invalid,
        "GapNotification",
        "A recovery snapshot cannot precede the missing range.",
        serde_json::json!({
            "stream": {"id": "stream-1", "generation": "3", "kind": "stream"},
            "gap": {
                "from_sequence": "5",
                "to_sequence": "9",
                "reason": "retention floor",
                "snapshot_watermark": "4"
            }
        }),
        Some("invalid_value"),
    ));
    fixtures.push(fixture(
        "event-replay-request-valid",
        FixtureKind::Valid,
        "ReplayRequest",
        "Replay is addressed by a generation-fenced stream handle.",
        serde_json::json!({
            "stream": {"id": "stream-1", "generation": "3", "kind": "stream"},
            "after_sequence": "0",
            "max_events": "16"
        }),
        None,
    ));
    fixtures.push(fixture(
        "event-replay-request-wrong-kind-invalid",
        FixtureKind::Invalid,
        "ReplayRequest",
        "Replay requires a stream handle; run handles are rejected by kind.",
        serde_json::json!({
            "stream": {"id": "run-1", "generation": "3", "kind": "run"},
            "after_sequence": "0"
        }),
        Some("invalid_value"),
    ));
    fixtures.push(fixture(
        "event-replay-empty-stream-invalid",
        FixtureKind::Invalid,
        "ReplayRequest",
        "Replay requests require a stream identity.",
        serde_json::json!({
            "stream": {"id": "", "generation": "3", "kind": "stream"},
            "after_sequence": "0",
            "max_events": "1"
        }),
        Some("invalid_value"),
    ));
    fixtures.push(fixture(
        "event-replay-zero-limit-invalid",
        FixtureKind::Invalid,
        "ReplayRequest",
        "A present replay limit must be positive.",
        serde_json::json!({
            "stream": {"id": "stream-1", "generation": "3", "kind": "stream"},
            "after_sequence": "0",
            "max_events": "0"
        }),
        Some("invalid_value"),
    ));
}

fn push_ack_fixtures(fixtures: &mut Vec<Fixture>) {
    fixtures.push(fixture(
        "event-ack-valid",
        FixtureKind::Valid,
        "EventAckNotification",
        "Client acknowledges one contiguous cursor on a stream handle.",
        serde_json::json!({
            "ack": {
                "stream": {"id": "stream-1", "generation": "3", "kind": "stream"},
                "last_processed_sequence": "12"
            }
        }),
        None,
    ));
    fixtures.push(fixture(
        "event-ack-zero-cursor-invalid",
        FixtureKind::Invalid,
        "EventAckNotification",
        "Acknowledged sequences start at one.",
        serde_json::json!({
            "ack": {
                "stream": {"id": "stream-1", "generation": "3", "kind": "stream"},
                "last_processed_sequence": "0"
            }
        }),
        Some("invalid_value"),
    ));
}

fn push_bridge_fixtures(fixtures: &mut Vec<Fixture>) {
    fixtures.push(fixture(
        "extension-outcome-stream-valid",
        FixtureKind::Valid,
        "ExtensionInvokeOutcome",
        "A streaming callback returns one stream handle instead of a large response.",
        serde_json::json!({
            "outcome": "stream",
            "stream": {"id": "stream-9", "generation": "2", "kind": "stream"}
        }),
        None,
    ));
    fixtures.push(fixture(
        "extension-outcome-empty-invalid",
        FixtureKind::Invalid,
        "ExtensionInvokeOutcome",
        "A callback outcome must choose exactly one tagged variant.",
        serde_json::json!({}),
        Some("invalid_value"),
    ));
    fixtures.push(fixture(
        "extension-outcome-ambiguous-invalid",
        FixtureKind::Invalid,
        "ExtensionInvokeOutcome",
        "Independent result and error fields cannot be combined.",
        serde_json::json!({"result": null, "error": null}),
        Some("invalid_value"),
    ));
    fixtures.push(fixture(
        "extension-stream-chunk-valid",
        FixtureKind::Valid,
        "ExtensionStreamEvent",
        "Callback stream chunks preserve stream identity and monotonic sequence.",
        serde_json::json!({
            "event": "chunk",
            "stream": {"id": "stream-9", "generation": "2", "kind": "stream"},
            "sequence": "1",
            "value": {"kind": "string", "value": "token"}
        }),
        None,
    ));
    fixtures.push(fixture(
        "extension-stream-sequence-zero-invalid",
        FixtureKind::Invalid,
        "ExtensionStreamEvent",
        "Callback stream sequences start at one.",
        serde_json::json!({
            "event": "chunk",
            "stream": {"id": "stream-9", "generation": "2", "kind": "stream"},
            "sequence": "0",
            "value": {"kind": "string", "value": "token"}
        }),
        Some("invalid_value"),
    ));
}

fn push_capability_fixtures(fixtures: &mut Vec<Fixture>) {
    fixtures.push(fixture(
        "capability-meta-valid",
        FixtureKind::Valid,
        "EchoAgentCapability",
        "initialize._meta payload a Host publishes for the extension profile.",
        serde_json::json!({
            "extension_protocol_version": 1,
            "contract_digest": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
            "source_contract_digest": "sha256:1111111111111111111111111111111111111111111111111111111111111111",
            "features": ["mcp", "subagent"],
            "capabilities": [
                {"capability": "runs", "required": true},
                {"capability": "event_replay", "required": false}
            ],
            "limits": {
                "max_message_bytes": "1048576",
                "max_stream_buffer_events": "1024",
                "max_callback_concurrency": 8,
                "callback_timeout": {"seconds": "30", "nanos": 0},
                "max_replay_events": "512",
                "max_structured_output_bytes": "262144",
                "max_outstanding_live_events": "256",
                "max_stream_buffer_bytes": "4194304",
                "max_replay_bytes": "4194304",
                "max_open_handles": "512"
            }
        }),
        None,
    ));
    fixtures.push(fixture(
        "capability-unsorted-features-invalid",
        FixtureKind::Invalid,
        "EchoAgentCapability",
        "Feature sets must be sorted/deduplicated for deterministic negotiation.",
        serde_json::json!({
            "extension_protocol_version": 1,
            "contract_digest": "sha256:0",
            "source_contract_digest": "sha256:1111111111111111111111111111111111111111111111111111111111111111",
            "features": ["subagent", "mcp"],
            "capabilities": [{"capability": "runs", "required": true}],
            "limits": {
                "max_message_bytes": "1",
                "max_stream_buffer_events": "1",
                "max_callback_concurrency": 1,
                "callback_timeout": {"seconds": "0", "nanos": 1},
                "max_replay_events": "1",
                "max_structured_output_bytes": "1",
                "max_outstanding_live_events": "1",
                "max_stream_buffer_bytes": "1",
                "max_replay_bytes": "1",
                "max_open_handles": "1"
            }
        }),
        Some("invalid_value"),
    ));
}

fn push_hello_fixtures(fixtures: &mut Vec<Fixture>) {
    fixtures.push(fixture(
        "hello-client-valid",
        FixtureKind::Valid,
        "EchoAgentClientHello",
        "clientCapabilities._meta payload requesting Extended mode.",
        serde_json::json!({
            "extension_protocol_version": 1,
            "contract_digest": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
            "source_contract_digest": "sha256:1111111111111111111111111111111111111111111111111111111111111111",
            "required_features": ["mcp"],
            "required_capabilities": ["runs"]
        }),
        None,
    ));
    fixtures.push(fixture(
        "hello-duplicate-capability-invalid",
        FixtureKind::Invalid,
        "EchoAgentClientHello",
        "Required capabilities must be unique.",
        serde_json::json!({
            "extension_protocol_version": 1,
            "contract_digest": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
            "source_contract_digest": "sha256:1111111111111111111111111111111111111111111111111111111111111111",
            "required_features": [],
            "required_capabilities": ["runs", "runs"]
        }),
        Some("invalid_value"),
    ));
}

fn push_agent_config_fixtures(fixtures: &mut Vec<Fixture>) {
    fixtures.push(fixture(
        "agent-config-host-default-valid",
        FixtureKind::Valid,
        "AgentCreateRequest",
        "Bind the Host default definition; no credential crosses the wire.",
        serde_json::json!({
            "config": {"variant": "host_default"}
        }),
        None,
    ));
    fixtures.push(fixture(
        "agent-config-explicit-env-credential-valid",
        FixtureKind::Valid,
        "AgentCreateRequest",
        "Explicit construction with environment credential sourcing.",
        serde_json::json!({
            "config": {
                "variant": "explicit",
                "config_version": 1,
                "model": {
                    "provider": "openai",
                    "name": "g-test",
                    "base_url": "https://api.example.com/v1/chat/completions",
                    "api_protocol": "chat_completions",
                    "credential": {"source": "env", "variable": "ECHO_MODEL_TOKEN"}
                },
                "agent": {
                    "name": "worker",
                    "system_prompt": "Do the task.",
                    "max_iterations": 8
                }
            },
            "idempotency_id": "cli-agent-1"
        }),
        None,
    ));
    fixtures.push(fixture(
        "agent-config-unsupported-knob-invalid",
        FixtureKind::Invalid,
        "AgentCreateRequest",
        "Unsupported builder knobs fail closed via deny_unknown_fields.",
        serde_json::json!({
            "config": {
                "variant": "explicit",
                "config_version": 1,
                "model": {
                    "provider": "openai",
                    "name": "g-test",
                    "base_url": "https://api.example.com/v1/chat/completions",
                    "api_protocol": "chat_completions"
                },
                "agent": {
                    "name": "worker",
                    "system_prompt": "Do the task.",
                    "max_iterations": 8,
                    "enable_memory": true
                }
            }
        }),
        Some("invalid_value"),
    ));
}

fn push_run_fixtures(fixtures: &mut Vec<Fixture>) {
    fixtures.push(fixture(
        "run-input-chat-valid",
        FixtureKind::Valid,
        "RunInput",
        "Chat runs carry exactly one typed text message.",
        serde_json::json!({"kind": "chat", "text": "Summarize the report."}),
        None,
    ));
    fixtures.push(fixture(
        "run-input-execute-valid",
        FixtureKind::Valid,
        "RunInput",
        "Execute runs carry the task directive.",
        serde_json::json!({"kind": "execute", "task": "run the benchmark"}),
        None,
    ));
    fixtures.push(fixture(
        "run-input-empty-text-invalid",
        FixtureKind::Invalid,
        "RunInput",
        "Empty turn input cannot start a run.",
        serde_json::json!({"kind": "chat", "text": ""}),
        Some("invalid_value"),
    ));
    fixtures.push(fixture(
        "run-terminal-completed-valid",
        FixtureKind::Valid,
        "RunTerminal",
        "A completed terminal carries the optional final answer.",
        serde_json::json!({"status": "completed", "final_answer": "done"}),
        None,
    ));
    fixtures.push(fixture(
        "run-terminal-failed-valid",
        FixtureKind::Valid,
        "RunTerminal",
        "A failed terminal carries the lossless framework failure.",
        serde_json::json!({
            "status": "failed",
            "failure": {
                "category": "llm",
                "terminal_kind": "failed",
                "retryable": true,
                "code": "llm_network",
                "http_status": 503,
                "message": "connection reset by peer"
            }
        }),
        None,
    ));
    fixtures.push(fixture(
        "run-receipt-completed-valid",
        FixtureKind::Valid,
        "RunReceiptWire",
        "Receipt counters use lossless integer strings.",
        serde_json::json!({
            "turn_id": "run-7",
            "outcome": "completed",
            "final_answer": "done",
            "final_message_id": "msg-7",
            "prompt_tokens": "120",
            "completion_tokens": "48",
            "llm_calls": "2",
            "compaction_count": "0",
            "last_event_sequence": "14",
            "elapsed_ms": "1834"
        }),
        None,
    ));
    fixtures.push(fixture(
        "run-recovered-interrupted-valid",
        FixtureKind::Valid,
        "RecoveredRunWire",
        "Interrupted runs expose fresh-generation handles without terminal.",
        serde_json::json!({
            "run": {"id": "run-9", "generation": "4", "kind": "run"},
            "stream": {"id": "stream-9", "generation": "4", "kind": "stream"},
            "status": "interrupted",
            "last_sequence": "21"
        }),
        None,
    ));
}

fn push_boundary_fixtures(fixtures: &mut Vec<Fixture>) {
    fixtures.push(fixture(
        "boundary-standard-method-rejected-invalid",
        FixtureKind::Invalid,
        "MethodCatalog",
        "Standard ACP method names cannot be re-registered as extension methods.",
        serde_json::json!({"method": "session/prompt"}),
        Some("invalid_request"),
    ));
    fixtures.push(fixture(
        "boundary-unnegotiated-method-rejected-invalid",
        FixtureKind::Invalid,
        "MethodCatalog",
        "Methods outside _echo_agent/* are not part of the extension contract.",
        serde_json::json!({"method": "run/start"}),
        Some("invalid_request"),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_is_deterministic() {
        let first = canonical_json(&build_extension_schema_doc());
        let second = canonical_json(&build_extension_schema_doc());
        assert_eq!(first, second);
        assert!(!first.is_empty());
    }

    #[test]
    fn schema_respects_acp_boundaries() {
        let doc = build_extension_schema_doc();
        assert!(validate_schema_boundaries(&doc).is_empty());
    }

    #[test]
    fn digest_is_stable_sha256() {
        let digest = extension_contract_digest();
        assert!(digest.starts_with("sha256:"));
        assert_eq!(digest.len(), "sha256:".len() + 64);
        assert_eq!(digest, extension_contract_digest());
    }

    #[test]
    fn fixture_names_are_unique() {
        let fixtures = all_fixtures();
        let names: Vec<&str> = fixtures.iter().map(|f| f.name.as_str()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(names.len(), sorted.len());
    }

    #[test]
    fn fixtures_cover_required_families() {
        let fixtures = all_fixtures();
        let targets: Vec<&str> = fixtures.iter().map(|f| f.target.as_str()).collect();
        for target in [
            "WireU64",
            "WirePath",
            "WireBytes",
            "WireValue",
            "WireHandle",
            "EchoSdkError",
            "WireEventEnvelope",
            "ReplayRequest",
            "ReplayResponse",
            "GapNotification",
            "EventAckNotification",
            "EchoAgentCapability",
            "EchoAgentClientHello",
            "AgentCreateRequest",
            "RunInput",
            "RunTerminal",
            "RunReceiptWire",
            "RecoveredRunWire",
            "ExtensionInvokeOutcome",
            "ExtensionStreamEvent",
            "MethodCatalog",
        ] {
            assert!(targets.contains(&target), "fixtures missing {target}");
        }
        let kinds: Vec<FixtureKind> = all_fixtures().iter().map(|f| f.kind).collect();
        assert!(kinds.contains(&FixtureKind::Valid));
        assert!(kinds.contains(&FixtureKind::Invalid));
    }
}
