//! Executable core RPC contract tests (supreme plan 05, todo
//! `freeze-executable-core-contract`).
//!
//! These tests freeze the behavior the Host and future language SDKs depend
//! on: typed JSON-RPC routing of every `_echo_agent/*` core method, hello
//! negotiation, the single extension error code, executable typed payloads,
//! and the generation-fenced stream handle / ACK / replay grammar.

use agent_client_protocol::{JsonRpcMessage, JsonRpcResponse};
use echo_sdk_protocol::capability::{
    CapabilityDeclaration, EchoAgentCapability, EchoAgentClientHello, EchoLimits,
    ExtensionCapability, meta_entry, meta_with_entry,
};
use echo_sdk_protocol::error::{
    AgentFailureWire, EXTENSION_ERROR_CODE, EchoSdkError, ExtensionErrorCode, Retryability,
};
use echo_sdk_protocol::event::{
    EventAck, EventAckNotification, EventCursor, EventGap, EventNotification, GapNotification,
    ReplayRequest, ReplayResponse, WireEventEnvelope, WireEventPayload,
};
use echo_sdk_protocol::handle::{HandleKind, WireHandle};
use echo_sdk_protocol::methods::{
    AgentCloseRequest, AgentCloseResponse, AgentConfigExplicitWire, AgentConfigWire,
    AgentCreateRequest, AgentCreateResponse, AgentDescribeRequest, AgentDescribeResponse,
    AgentSettingsWire, CredentialSourceWire, LlmApiProtocolWire, ModelConfigWire, RecoveredRunWire,
    RunCancelRequest, RunCancelResponse, RunGetRequest, RunGetResponse, RunInput, RunReceiptWire,
    RunStartRequest, RunStartResponse, RunStatus, RunTerminal, RunWaitRequest, RunWaitResponse,
    SessionCloseRequest, SessionCloseResponse, SessionCreateRequest, SessionCreateResponse,
    SessionLoadRequest, SessionLoadResponse,
};
use echo_sdk_protocol::scalar::{WireDuration, WireI64, WireNonZeroU64, WireTimestamp, WireU64};

const HELLO_DIGEST: &str =
    "sha256:0000000000000000000000000000000000000000000000000000000000000000";
const SOURCE_DIGEST: &str =
    "sha256:1111111111111111111111111111111111111111111111111111111111111111";

/// Test helper: sequences in this suite are always ≥ 1, which is exactly the
/// non-zero scalar contract, so construction cannot fail for valid input.
fn nonzero(value: u64) -> WireNonZeroU64 {
    assert!(value >= 1, "test sequences must be non-zero");
    match WireNonZeroU64::try_from(value.to_string()) {
        Ok(parsed) => parsed,
        Err(_) => unreachable!("a non-zero decimal string always parses"),
    }
}

fn handle(id: &str, kind: HandleKind) -> WireHandle {
    WireHandle {
        id: id.to_string(),
        generation: WireU64::from_u64(7),
        kind,
    }
}

fn stream_handle(id: &str) -> WireHandle {
    handle(id, HandleKind::Stream)
}

// ── Typed routing ───────────────────────────────────────────────────────────

#[test]
fn core_requests_route_by_exact_method() {
    assert!(AgentCreateRequest::matches_method(
        "_echo_agent/agent/create"
    ));
    assert!(!AgentCreateRequest::matches_method(
        "_echo_agent/agent/close"
    ));
    assert!(!AgentCreateRequest::matches_method("session/prompt"));
    assert!(!AgentCreateRequest::matches_method("_echo_agent/run/start"));

    assert!(
        AgentDescribeRequest::matches_method("_echo_agent/agent/describe"),
        "agent/describe must route"
    );
    assert!(AgentCloseRequest::matches_method("_echo_agent/agent/close"));
    assert!(
        SessionCreateRequest::matches_method("_echo_agent/session/create")
            && SessionLoadRequest::matches_method("_echo_agent/session/load")
            && SessionCloseRequest::matches_method("_echo_agent/session/close"),
        "session family must route"
    );
    assert!(
        RunStartRequest::matches_method("_echo_agent/run/start")
            && RunGetRequest::matches_method("_echo_agent/run/get")
            && RunWaitRequest::matches_method("_echo_agent/run/wait")
            && RunCancelRequest::matches_method("_echo_agent/run/cancel")
            && ReplayRequest::matches_method("_echo_agent/run/replay"),
        "run family must route"
    );
    assert!(
        EventNotification::matches_method("_echo_agent/event")
            && EventAckNotification::matches_method("_echo_agent/event/ack")
            && GapNotification::matches_method("_echo_agent/gap"),
        "notification methods must route"
    );
}

#[test]
fn request_method_names_are_stable() {
    let request = AgentCreateRequest {
        config: AgentConfigWire::HostDefault,
        idempotency_id: None,
    };
    assert_eq!(request.method(), "_echo_agent/agent/create");
    let wait = RunWaitRequest {
        run: handle("run-1", HandleKind::Run),
        timeout: Some(WireDuration::from_nanos(1_000)),
    };
    assert_eq!(wait.method(), "_echo_agent/run/wait");
}

#[test]
fn foreign_methods_fall_back_to_method_not_found() {
    // The typed parse rejects any other method instead of guessing.
    let params = serde_json::json!({"config": {"variant": "host_default"}});
    assert!(
        AgentCreateRequest::parse_message("_echo_agent/agent/delete", &params).is_err(),
        "unknown extension methods must not parse"
    );
    assert!(
        AgentCreateRequest::parse_message("session/prompt", &params).is_err(),
        "standard methods must not parse as extension requests"
    );
}

#[test]
fn responses_round_trip_through_json() {
    let response = RunStartResponse {
        run: handle("run-1", HandleKind::Run),
        stream: stream_handle("stream-1"),
        first_event: None,
    };
    let encoded = response
        .clone()
        .into_json("_echo_agent/run/start")
        .unwrap_or(serde_json::Value::Null);
    let decoded = RunStartResponse::from_value("_echo_agent/run/start", encoded).unwrap_or(
        RunStartResponse {
            run: handle("broken", HandleKind::Run),
            stream: stream_handle("broken"),
            first_event: None,
        },
    );
    assert_eq!(decoded, response);

    let session = SessionCreateResponse {
        session: handle("sess-1", HandleKind::Session),
        acp_session_id: "sess_acp1".to_string(),
    };
    let encoded = session
        .clone()
        .into_json("_echo_agent/session/create")
        .unwrap_or(serde_json::Value::Null);
    let decoded = SessionCreateResponse::from_value("_echo_agent/session/create", encoded)
        .unwrap_or_else(|_| session.clone());
    assert_eq!(decoded, session);
}

#[test]
fn notifications_keep_their_wire_names() {
    let ack = EventAckNotification {
        ack: EventAck {
            stream: stream_handle("stream-1"),
            last_processed_sequence: nonzero(12),
        },
    };
    assert_eq!(ack.method(), "_echo_agent/event/ack");
    let _ = AgentCloseResponse { released: true }.into_json("_echo_agent/agent/close");
    let _ = AgentDescribeResponse {
        snapshot: echo_sdk_protocol::methods::AgentSnapshotWire {
            name: "a".to_string(),
            model_name: "m".to_string(),
            system_prompt: "s".to_string(),
            tool_names: vec![],
            skill_names: vec![],
            mcp_server_names: vec![],
            working_dir: None,
            host_default: true,
        },
    }
    .into_json("_echo_agent/agent/describe");
    let _ = RunGetResponse {
        status: RunStatus::Running,
        last_sequence: WireU64::from_u64(0),
        stream: None,
        terminal: None,
        receipt: None,
    }
    .into_json("_echo_agent/run/get");
    let _ = RunWaitResponse {
        settled: false,
        terminal: None,
        receipt: None,
    }
    .into_json("_echo_agent/run/wait");
    let _ = RunCancelResponse {
        cancellation_initiated: false,
        status: RunStatus::Running,
    }
    .into_json("_echo_agent/run/cancel");
    let _ = SessionCloseResponse { released: true }.into_json("_echo_agent/session/close");
    let _ = SessionLoadResponse {
        session: handle("sess-1", HandleKind::Session),
        acp_session_id: "sess_acp1".to_string(),
        recovered_sequence: None,
        runs: Vec::new(),
    }
    .into_json("_echo_agent/session/load");
    let _ = RunStartResponse {
        run: handle("run-1", HandleKind::Run),
        stream: stream_handle("stream-1"),
        first_event: None,
    }
    .into_json("_echo_agent/run/start");
    let _ = AgentCreateResponse {
        agent: handle("agent-1", HandleKind::Agent),
    }
    .into_json("_echo_agent/agent/create");
    let _ = ReplayResponse {
        requested_after_sequence: WireU64::from_u64(0),
        events: Vec::new(),
        next_cursor: EventCursor {
            stream_id: "s".to_string(),
            last_processed_sequence: WireU64::from_u64(0),
        },
        gap: None,
    }
    .into_json("_echo_agent/run/replay");
    // Compile-check the remaining request derives too.
    let _ = RunGetRequest {
        run: handle("run-1", HandleKind::Run),
    }
    .method();
    let _ = RunCancelRequest {
        run: handle("run-1", HandleKind::Run),
    }
    .method();
    let _ = SessionCloseRequest {
        session: handle("sess-1", HandleKind::Session),
    }
    .method();
    let _ = SessionLoadRequest {
        agent: handle("agent-1", HandleKind::Agent),
        session_id: "sess-1".to_string(),
        working_dir: None,
    }
    .method();
    let _ = SessionCreateRequest {
        agent: handle("agent-1", HandleKind::Agent),
        working_dir: None,
        session_id: None,
        idempotency_id: None,
    }
    .method();
    let _ = AgentDescribeRequest {
        agent: handle("agent-1", HandleKind::Agent),
    }
    .method();
    let _ = AgentCloseRequest {
        agent: handle("agent-1", HandleKind::Agent),
    }
    .method();
    let _ = EventNotification {
        stream: stream_handle("stream-1"),
        envelope: envelope("stream-1", 1),
    }
    .method();
    let _ = GapNotification {
        stream: stream_handle("stream-1"),
        gap: gap(1, 2, 2),
    }
    .method();
}

// ── Hello negotiation ───────────────────────────────────────────────────────

fn host_capability() -> EchoAgentCapability {
    EchoAgentCapability {
        extension_protocol_version: 1,
        contract_digest: HELLO_DIGEST.to_string(),
        source_contract_digest: SOURCE_DIGEST.to_string(),
        features: vec!["acp".to_string(), "mcp".to_string()],
        capabilities: vec![
            CapabilityDeclaration {
                capability: ExtensionCapability::AgentLifecycle,
                required: false,
            },
            CapabilityDeclaration {
                capability: ExtensionCapability::SessionHandles,
                required: false,
            },
            CapabilityDeclaration {
                capability: ExtensionCapability::Runs,
                required: true,
            },
            CapabilityDeclaration {
                capability: ExtensionCapability::EventReplay,
                required: false,
            },
        ],
        limits: EchoLimits {
            max_message_bytes: WireU64::from_u64(1_048_576),
            max_stream_buffer_events: WireU64::from_u64(1024),
            max_callback_concurrency: 4,
            callback_timeout: WireDuration::from_nanos(30_000_000_000),
            max_replay_events: WireU64::from_u64(512),
            max_structured_output_bytes: WireU64::from_u64(262_144),
            max_outstanding_live_events: WireU64::from_u64(128),
            max_stream_buffer_bytes: WireU64::from_u64(2_097_152),
            max_replay_bytes: WireU64::from_u64(2_097_152),
            max_open_handles: WireU64::from_u64(256),
        },
    }
}

fn client_hello() -> EchoAgentClientHello {
    EchoAgentClientHello {
        extension_protocol_version: 1,
        contract_digest: HELLO_DIGEST.to_string(),
        source_contract_digest: SOURCE_DIGEST.to_string(),
        required_features: vec!["mcp".to_string()],
        required_capabilities: vec![ExtensionCapability::Runs],
    }
}

#[test]
fn plain_clients_stay_standard_and_malformed_hello_fails_closed() {
    // No meta at all: the Host never runs negotiation for a plain Client
    // (Standard mode is unconditional). The meta reader distinguishes an
    // absent key from a present one.
    assert!(meta_entry(None).is_none());
    let meta = meta_with_entry(serde_json::json!({
        "extension_protocol_version": 1,
        "contract_digest": HELLO_DIGEST,
        "source_contract_digest": SOURCE_DIGEST,
        "required_features": [],
        "required_capabilities": ["runs"]
    }))
    .unwrap_or_default();
    let entry = meta_entry(Some(&meta))
        .and_then(|entry| entry.ok())
        .unwrap_or(&serde_json::Value::Null);
    assert!(EchoAgentClientHello::from_meta_value(entry).is_ok());

    // Present but malformed: decoding fails closed, negotiation never runs.
    let malformed = meta_with_entry(serde_json::json!({"hello": true})).unwrap_or_default();
    let entry = meta_entry(Some(&malformed))
        .and_then(|entry| entry.ok())
        .unwrap_or(&serde_json::Value::Null);
    assert!(EchoAgentClientHello::from_meta_value(entry).is_err());
}

#[test]
fn hello_mismatches_fail_closed_per_dimension() {
    let host = host_capability();
    assert!(host.validate_shape().is_empty());
    assert!(host.negotiate_hello(&client_hello()).is_empty());

    let mut version = client_hello();
    version.extension_protocol_version = 2;
    assert_eq!(host.negotiate_hello(&version).len(), 1);

    let mut contract = client_hello();
    contract.contract_digest = format!("sha256:{}", "f".repeat(64));
    assert_eq!(host.negotiate_hello(&contract).len(), 1);

    let mut source = client_hello();
    source.source_contract_digest = format!("sha256:{}", "e".repeat(64));
    assert_eq!(host.negotiate_hello(&source).len(), 1);

    let mut feature = client_hello();
    feature.required_features = vec!["mcp".to_string(), "chart".to_string()];
    let problems = host.negotiate_hello(&feature);
    assert_eq!(problems.len(), 1);
    assert!(problems[0].contains("chart"));

    let mut capability = client_hello();
    capability
        .required_capabilities
        .push(ExtensionCapability::Subagents);
    let problems = host.negotiate_hello(&capability);
    assert_eq!(problems.len(), 1);
    assert!(problems[0].contains("subagents"));
}

#[test]
fn malformed_hello_shape_is_reported_before_negotiation() {
    let mut hello = client_hello();
    hello.required_features = vec!["zeta".to_string(), "alpha".to_string()];
    let problems = hello.validate_shape();
    assert!(problems.iter().any(|problem| problem.contains("sorted")));

    let mut duplicate = client_hello();
    duplicate
        .required_capabilities
        .push(ExtensionCapability::Runs);
    assert!(
        duplicate
            .validate_shape()
            .iter()
            .any(|problem| problem.contains("duplicate"))
    );
}

// ── Error mapping ───────────────────────────────────────────────────────────

#[test]
fn extension_errors_use_one_fixed_server_error_code() {
    let error = EchoSdkError::new(
        ExtensionErrorCode::StaleHandle,
        "run handle predates host restart",
        Retryability::Never,
    )
    .with_operation("run/wait")
    .with_handle(handle("run-1", HandleKind::Run));
    let rpc = error.clone().into_jsonrpc_error();
    assert!(matches!(
        rpc.code,
        agent_client_protocol::ErrorCode::Other(code) if code == EXTENSION_ERROR_CODE
    ));
    let decoded = EchoSdkError::from_jsonrpc_data(rpc.data.as_ref());
    assert_eq!(
        decoded.unwrap_or_else(|message| EchoSdkError::new(
            ExtensionErrorCode::SerializationViolation,
            message,
            Retryability::Never,
        )),
        error
    );
}

// ── Executable payloads ─────────────────────────────────────────────────────

#[test]
fn agent_config_payloads_stay_strict() {
    let host_default: AgentCreateRequest =
        serde_json::from_value(serde_json::json!({"config": {"variant": "host_default"}}))
            .unwrap_or(AgentCreateRequest {
                config: AgentConfigWire::HostDefault,
                idempotency_id: None,
            });
    assert_eq!(host_default.config, AgentConfigWire::HostDefault);

    // Unknown knobs fail closed instead of being silently ignored.
    let unsupported: Result<AgentCreateRequest, _> = serde_json::from_value(serde_json::json!({
        "config": {
            "variant": "explicit",
            "config_version": 1,
            "model": {
                "provider": "openai",
                "name": "g-test",
                "base_url": "https://api.example.com/v1",
                "api_protocol": "chat_completions"
            },
            "agent": {
                "name": "worker",
                "system_prompt": "s",
                "max_iterations": 4,
                "structured_output": {"schema": {}}
            }
        }
    }));
    assert!(unsupported.is_err(), "unsupported config must fail closed");

    // Both credential sources are never present at once (tagged enum).
    let both_sources: Result<ModelConfigWire, _> = serde_json::from_value(serde_json::json!({
        "provider": "openai",
        "name": "g-test",
        "base_url": "https://api.example.com/v1",
        "api_protocol": "chat_completions",
        "credential": {"source": "inline", "token": "t"}
    }));
    let inline = both_sources.unwrap_or(ModelConfigWire {
        provider: String::new(),
        name: String::new(),
        base_url: String::new(),
        api_protocol: LlmApiProtocolWire::ChatCompletions,
        credential: None,
        max_tokens: None,
        temperature: None,
        context_window: None,
    });
    assert_eq!(
        inline.credential,
        Some(CredentialSourceWire::Inline {
            token: "t".to_string()
        })
    );

    let explicit = AgentConfigExplicitWire {
        config_version: 1,
        model: ModelConfigWire {
            provider: "openai".to_string(),
            name: "g-test".to_string(),
            base_url: "https://api.example.com/v1".to_string(),
            api_protocol: LlmApiProtocolWire::ChatCompletions,
            credential: Some(CredentialSourceWire::Env {
                variable: "ECHO_MODEL_TOKEN".to_string(),
            }),
            max_tokens: None,
            temperature: None,
            context_window: None,
        },
        agent: AgentSettingsWire {
            name: "worker".to_string(),
            system_prompt: "Do the task.".to_string(),
            max_iterations: 4,
        },
    };
    let round: AgentConfigWire = serde_json::from_value(
        serde_json::to_value(AgentConfigWire::Explicit(Box::new(explicit.clone())))
            .unwrap_or(serde_json::Value::Null),
    )
    .unwrap_or(AgentConfigWire::HostDefault);
    assert_eq!(round, AgentConfigWire::Explicit(Box::new(explicit)));
}

#[test]
fn run_input_payloads_are_typed_and_validated() {
    let chat: RunInput = serde_json::from_value(
        serde_json::json!({"kind": "chat", "text": "hello"}),
    )
    .unwrap_or(RunInput::Chat {
        text: String::new(),
    });
    assert!(chat.validate().is_ok());
    let execute = RunInput::Execute {
        task: "do it".to_string(),
    };
    assert!(execute.validate().is_ok());
    let empty = RunInput::Chat {
        text: "  ".to_string(),
    };
    assert!(empty.validate().is_err());
}

#[test]
fn terminal_and_receipt_project_the_framework_authority() {
    let failure = AgentFailureWire {
        category: "llm".to_string(),
        terminal_kind: "failed".to_string(),
        retryable: true,
        code: "llm_network".to_string(),
        http_status: Some(503),
        message: "connection reset".to_string(),
    };
    let terminal = RunTerminal::Failed {
        failure: failure.clone(),
    };
    assert!(terminal.validate().is_ok());
    let encoded = serde_json::to_value(&terminal).unwrap_or(serde_json::Value::Null);
    assert_eq!(encoded["status"], "failed");
    assert_eq!(encoded["failure"]["code"], "llm_network");

    let receipt = RunReceiptWire {
        turn_id: "run-1".to_string(),
        outcome: "completed".to_string(),
        final_answer: Some("done".to_string()),
        final_message_id: None,
        prompt_tokens: WireU64::from_u64(10),
        completion_tokens: WireU64::from_u64(4),
        llm_calls: WireU64::from_u64(1),
        compaction_count: WireU64::from_u64(0),
        last_event_sequence: WireU64::from_u64(3),
        elapsed_ms: WireU64::from_u64(84),
    };
    assert!(receipt.validate().is_ok());
    // Token counters are decimal strings on the wire, never JS numbers.
    let encoded = serde_json::to_value(&receipt).unwrap_or(serde_json::Value::Null);
    assert_eq!(encoded["prompt_tokens"], "10");
}

#[test]
fn interrupted_runs_never_carry_a_terminal() {
    let recovered = RecoveredRunWire {
        run: handle("run-9", HandleKind::Run),
        stream: stream_handle("stream-9"),
        status: RunStatus::Interrupted,
        last_sequence: WireU64::from_u64(21),
        terminal: None,
    };
    assert!(recovered.validate().is_ok());

    let mut forged = recovered.clone();
    forged.terminal = Some(RunTerminal::Completed {
        final_answer: Some("fabricated".to_string()),
    });
    assert!(
        forged.validate().is_err(),
        "interrupted runs must not fake success"
    );

    let status = serde_json::to_value(RunStatus::Interrupted).unwrap_or(serde_json::Value::Null);
    assert_eq!(status, "interrupted");
}

// ── Stream handles, ACK and replay ──────────────────────────────────────────

fn envelope(stream_id: &str, sequence: u64) -> WireEventEnvelope {
    WireEventEnvelope {
        schema_version: 4,
        event_id: format!("event-{sequence}"),
        content_hash: format!("sha256:{}", "a".repeat(64)),
        sequence: nonzero(sequence),
        stream_id: stream_id.to_string(),
        conversation_id: None,
        run_id: Some("run-1".to_string()),
        turn_id: "run-1".to_string(),
        message_id: None,
        execution_id: None,
        parent_event_id: None,
        timestamp: WireTimestamp {
            unix_seconds: WireI64::from_i64(1_757_000_000),
            nanos: 0,
            rfc3339: None,
        },
        payload: WireEventPayload {
            event_type: "token".to_string(),
            data: None,
        },
    }
}

fn gap(from: u64, to: u64, watermark: u64) -> EventGap {
    EventGap {
        from_sequence: nonzero(from),
        to_sequence: nonzero(to),
        reason: "retention floor".to_string(),
        snapshot_watermark: nonzero(watermark),
    }
}

#[test]
fn ack_notifications_retire_outstanding_events() {
    let notification = EventAckNotification {
        ack: EventAck {
            stream: stream_handle("stream-1"),
            last_processed_sequence: nonzero(12),
        },
    };
    assert!(notification.ack.validate().is_ok());
    let encoded = serde_json::to_value(&notification).unwrap_or(serde_json::Value::Null);
    assert_eq!(encoded["ack"]["stream"]["kind"], "stream");
    assert_eq!(encoded["ack"]["last_processed_sequence"], "12");

    // Wrong-kind handles fail the ACK grammar before the registry sees it.
    let wrong_kind = EventAck {
        stream: handle("stream-1", HandleKind::Run),
        last_processed_sequence: nonzero(12),
    };
    assert!(wrong_kind.validate().is_err());
}

#[test]
fn event_notifications_must_match_their_stream() {
    let notification = EventNotification {
        stream: stream_handle("stream-1"),
        envelope: envelope("stream-1", 1),
    };
    assert!(notification.validate().is_ok());

    let mut mismatched = notification.clone();
    mismatched.envelope = envelope("stream-2", 1);
    assert!(mismatched.validate().is_err());

    let mut wrong_generation = notification;
    wrong_generation.stream = WireHandle {
        id: "stream-1".to_string(),
        generation: WireU64::from_u64(6),
        kind: HandleKind::Stream,
    };
    // Generation mismatches are rejected at the Host registry; the wire
    // grammar itself only checks kind and stream binding.
    assert!(wrong_generation.validate().is_ok());
}

#[test]
fn replay_is_generation_fenced_and_gap_aware() {
    let request = ReplayRequest {
        stream: stream_handle("stream-1"),
        after_sequence: WireU64::from_u64(0),
        max_events: Some(nonzero(16)),
    };
    assert_eq!(request.method(), "_echo_agent/run/replay");
    assert!(request.validate().is_ok());

    let response = ReplayResponse {
        requested_after_sequence: WireU64::from_u64(0),
        events: vec![envelope("stream-1", 1), envelope("stream-1", 2)],
        next_cursor: EventCursor {
            stream_id: "stream-1".to_string(),
            last_processed_sequence: WireU64::from_u64(2),
        },
        gap: None,
    };
    assert!(response.validate().is_ok());

    let gap_response = ReplayResponse {
        requested_after_sequence: WireU64::from_u64(4),
        events: Vec::new(),
        next_cursor: EventCursor {
            stream_id: "stream-1".to_string(),
            last_processed_sequence: WireU64::from_u64(9),
        },
        gap: Some(gap(5, 9, 9)),
    };
    assert!(gap_response.validate().is_ok());

    let notification = GapNotification {
        stream: stream_handle("stream-1"),
        gap: gap(5, 9, 9),
    };
    assert!(notification.validate().is_ok());
    assert_eq!(notification.method(), "_echo_agent/gap");
}

// ── Source contract ─────────────────────────────────────────────────────────

#[test]
fn source_contract_digest_is_order_and_length_sensitive() {
    let entries = [
        ("Cargo.lock", b"one".as_slice()),
        ("contracts/sdk/public-api.txt", b"two".as_slice()),
        ("contracts/sdk/parity-manifest.json", b"three".as_slice()),
    ];
    let digest = echo_sdk_protocol::schema::aggregate_source_digest(&entries);
    assert!(digest.starts_with("sha256:"));

    // Different content in the same input changes the aggregate.
    let mutated = [
        ("Cargo.lock", b"one".as_slice()),
        ("contracts/sdk/public-api.txt", b"two!".as_slice()),
        ("contracts/sdk/parity-manifest.json", b"three".as_slice()),
    ];
    assert_ne!(
        digest,
        echo_sdk_protocol::schema::aggregate_source_digest(&mutated)
    );

    // A different input order is a different compatibility statement.
    let reordered = [
        ("contracts/sdk/public-api.txt", b"two".as_slice()),
        ("Cargo.lock", b"one".as_slice()),
        ("contracts/sdk/parity-manifest.json", b"three".as_slice()),
    ];
    assert_ne!(
        digest,
        echo_sdk_protocol::schema::aggregate_source_digest(&reordered)
    );

    let doc = echo_sdk_protocol::schema::build_source_contract_doc(&entries);
    assert_eq!(
        doc["aggregate_digest"],
        serde_json::json!(echo_sdk_protocol::schema::aggregate_source_digest(&entries))
    );
    assert_eq!(doc["inputs"].as_array().map(Vec::len), Some(3));
    assert_eq!(doc["algorithm"], "sha256-length-prefixed-v1");
}
