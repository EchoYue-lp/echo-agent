//! Core profile end-to-end acceptance (supreme plan 05, todo
//! `prove-and-document-core-profile`).
//!
//! Real official ACP Client against the real `echo-agent-sdk-host` child
//! process with the negotiated `_echo_agent/*` core profile: valid hello →
//! Agent/Session/Run lifecycle → full events → get/wait → replay/ack →
//! close; the fail-closed matrix (plain Client, mismatched hello, forced
//! extension calls); restart recovery on a shared state root (stale
//! handles, session load, interrupted runs); and the bounded stdin frame
//! limiter.

#![cfg(feature = "sdk-core-profile")]

#[cfg(feature = "sdk-facade-adapters")]
use agent_client_protocol::UntypedMessage;
use agent_client_protocol::schema::{ProtocolVersion, v1};
use agent_client_protocol::{BoxFuture, ByteStreams, Client, ConnectionTo, LineDirection};
#[cfg(feature = "sdk-facade-adapters")]
use base64::Engine as _;
use echo_sdk_protocol::capability::{
    EchoAgentCapability, EchoAgentClientHello, ExtensionCapability,
};
#[cfg(feature = "sdk-facade-adapters")]
use echo_sdk_protocol::error::ExtensionErrorCode;
use echo_sdk_protocol::event::{EventAck, EventAckNotification, EventNotification, ReplayRequest};
use echo_sdk_protocol::handle::HandleKind;
#[cfg(feature = "sdk-facade-adapters")]
use echo_sdk_protocol::handle::WireHandle;
use echo_sdk_protocol::methods::{
    AgentCloseRequest, AgentConfigWire, AgentCreateRequest, AgentDescribeRequest, RunGetRequest,
    RunInput, RunStartRequest, RunStatus, RunWaitRequest, SessionCloseRequest,
    SessionCreateRequest, SessionLoadRequest,
};
#[cfg(feature = "sdk-facade-adapters")]
use echo_sdk_protocol::methods::{
    RunCancelRequest, TaskCreateRequest, TaskListRequest, TaskUpdateRequest,
};
#[cfg(feature = "sdk-facade-adapters")]
use echo_sdk_protocol::scalar::{
    WireBytes, WireDuration, WireField, WireMapEntry, WirePath, WireValue,
};
use echo_sdk_protocol::scalar::{WireNonZeroU64, WireU64};
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::AsyncWriteExt as _;
use tokio::net::TcpListener;

mod support;

const SENTINEL_SECRET: &str = "sdk-core-sentinel-secret";

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_echo-agent-sdk-host"))
}

/// The embedded source contract is the same generated artifact the Host
/// embeds, so a same-revision Client hello always matches.
const SOURCE_CONTRACT_JSON: &str = include_str!("../../contracts/sdk/source-contract.json");

fn source_contract_digest() -> String {
    let document: serde_json::Value =
        serde_json::from_str(SOURCE_CONTRACT_JSON).expect("embedded source contract parses");
    document
        .get("aggregate_digest")
        .and_then(serde_json::Value::as_str)
        .expect("aggregate_digest present")
        .to_string()
}

fn client_hello() -> EchoAgentClientHello {
    EchoAgentClientHello {
        extension_protocol_version: echo_sdk_protocol::EXTENSION_PROTOCOL_VERSION,
        contract_digest: echo_sdk_protocol::schema::extension_contract_digest(),
        source_contract_digest: source_contract_digest(),
        required_features: Vec::new(),
        required_capabilities: vec![
            ExtensionCapability::AgentLifecycle,
            ExtensionCapability::SessionHandles,
            ExtensionCapability::Runs,
            ExtensionCapability::EventReplay,
        ],
    }
}

fn initialize_request(hello: Option<EchoAgentClientHello>) -> v1::InitializeRequest {
    let mut request = v1::InitializeRequest::new(ProtocolVersion::V1);
    if let Some(hello) = hello {
        let value = serde_json::to_value(&hello).expect("hello JSON");
        let mut meta = v1::Meta::new();
        meta.insert("echo_agent".to_string(), value);
        request.client_capabilities.meta = Some(meta);
    }
    request
}

fn write_config(
    directory: &Path,
    endpoint: &str,
    state_root: &Path,
    limits: Option<serde_json::Value>,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let document = serde_json::json!({
        "schema_version": 1,
        "default_agent": {
            "model": {
                "provider": "fixture",
                "name": "fixture-model",
                "base_url": endpoint,
                "api_protocol": "chat_completions",
                "auth_token": SENTINEL_SECRET
            },
            "agent": {
                "name": "fixture-agent",
                "system_prompt": "Answer the user directly.",
                "max_iterations": 4,
                "enable_tools": true,
                // The memory family e2e needs a live store behind the
                // session agent; the path stays inside the temp work dir.
                "enable_memory": true,
                "memory_path": directory.join("memstore.json").display().to_string()
            }
        },
        "sdk_profile": {
            "state_root": state_root.display().to_string(),
            "limits": limits.unwrap_or_else(|| serde_json::json!({}))
        }
    });
    let path = directory.join("host.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&document)?)?;
    Ok(path)
}

/// Loopback model server answering one chat completion, then closing.
async fn start_model_server(
    answer: &'static str,
) -> Result<(String, Arc<tokio::sync::Notify>), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let request_seen = Arc::new(tokio::sync::Notify::new());
    let notify = request_seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let notify = notify.clone();
            tokio::spawn(async move {
                let _ = support::read_http_request(&mut socket).await;
                notify.notify_one();
                let body = format!(
                    "data: {{\"id\":\"fixture\",\"choices\":[{{\"index\":0,\"delta\":{{\"role\":\"assistant\",\"content\":\"{answer}\"}},\"finish_reason\":\"stop\"}}]}}\n\ndata: [DONE]\n\n"
                );
                let headers = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = socket.write_all(headers.as_bytes()).await;
                let _ = socket.write_all(body.as_bytes()).await;
                let _ = socket.flush().await;
            });
        }
    });
    Ok((
        format!("http://{address}/v1/chat/completions"),
        request_seen,
    ))
}

/// Model server that sends one chunk and parks, keeping the run active.
async fn start_parking_model_server()
-> Result<(String, Arc<tokio::sync::Notify>), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let request_seen = Arc::new(tokio::sync::Notify::new());
    let notify = request_seen.clone();
    tokio::spawn(async move {
        let Ok((mut socket, _)) = listener.accept().await else {
            return;
        };
        let mut request = vec![0_u8; 64 * 1024];
        let _ = tokio::io::AsyncReadExt::read(&mut socket, &mut request).await;
        notify.notify_one();
        let payload = b"data: {\"id\":\"fixture\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"partial\"},\"finish_reason\":null}]}\n\n";
        let _ = socket
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: keep-alive\r\n\r\n",
            )
            .await;
        let _ = socket
            .write_all(format!("{:X}\r\n", payload.len()).as_bytes())
            .await;
        let _ = socket.write_all(payload).await;
        let _ = socket.write_all(b"\r\n").await;
        let _ = socket.flush().await;
        std::future::pending::<()>().await;
    });
    Ok((
        format!("http://{address}/v1/chat/completions"),
        request_seen,
    ))
}

type SharedVec<T> = Arc<Mutex<Vec<T>>>;

struct E2eProcessLock {
    path: PathBuf,
}

impl Drop for E2eProcessLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn acquire_e2e_process_lock() -> E2eProcessLock {
    let path = PathBuf::from("/tmp/echo-agent-sdk-host-e2e.lock");
    loop {
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(_) => return E2eProcessLock { path },
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if std::fs::metadata(&path)
                    .and_then(|metadata| metadata.modified())
                    .ok()
                    .and_then(|modified| modified.elapsed().ok())
                    .is_some_and(|age| age > Duration::from_secs(300))
                {
                    let _ = std::fs::remove_file(&path);
                } else {
                    std::thread::sleep(Duration::from_millis(25));
                }
            }
            Err(error) => panic!("failed to acquire E2E process lock: {error}"),
        }
    }
}

struct HostProcess {
    child: tokio::process::Child,
    stderr: SharedVec<u8>,
}

async fn spawn_host(config: &Path) -> Result<HostProcess, Box<dyn std::error::Error>> {
    let mut child = tokio::process::Command::new(binary())
        .arg("--config")
        .arg(config)
        // The fixture model servers are loopback by design; reqwest follows
        // the developer's system proxy otherwise and the model request never
        // reaches the fixture. Loopback is always excluded from proxies.
        .env("NO_PROXY", "127.0.0.1,localhost")
        .env("no_proxy", "127.0.0.1,localhost")
        // Keep the core-profile matrix deterministic even when the parent
        // test process exports RUST_LOG=warn; the official ACP runtime's
        // nested handler warning is intentionally expensive to format.
        .env("RUST_LOG", "error")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stderr_handle = child.stderr.take().ok_or("host stderr not piped")?;
    let stderr: SharedVec<u8> = Arc::new(Mutex::new(Vec::new()));
    let sink = stderr.clone();
    tokio::spawn(async move {
        use tokio::io::AsyncReadExt as _;
        let mut stderr_handle = stderr_handle;
        let mut buffer = [0_u8; 4096];
        loop {
            match stderr_handle.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(read) => sink
                    .lock()
                    .expect("stderr sink")
                    .extend_from_slice(&buffer[..read]),
            }
        }
    });
    Ok(HostProcess { child, stderr })
}

fn host_transport(
    child: &mut tokio::process::Child,
) -> ByteStreams<
    tokio_util::compat::Compat<tokio::process::ChildStdin>,
    tokio_util::compat::Compat<tokio::process::ChildStdout>,
> {
    let stdin = child.stdin.take().expect("host stdin piped");
    let stdout = child.stdout.take().expect("host stdout piped");
    ByteStreams::new(
        tokio_util::compat::TokioAsyncWriteCompatExt::compat_write(stdin),
        tokio_util::compat::TokioAsyncReadCompatExt::compat(stdout),
    )
}

fn stderr_text(host: &HostProcess) -> String {
    String::from_utf8_lossy(&host.stderr.lock().expect("stderr lock")).to_string()
}

/// Connect a Client to the host, collecting `_echo_agent/event` and
/// `session/update` notifications, and run the scenario to completion.
async fn drive<T, F>(
    host: &mut HostProcess,
    events: SharedVec<EventNotification>,
    updates: SharedVec<v1::SessionNotification>,
    gaps: SharedVec<echo_sdk_protocol::event::GapNotification>,
    scenario: F,
) -> Result<T, Box<dyn std::error::Error>>
where
    T: Send + 'static,
    F: FnOnce(
            ConnectionTo<agent_client_protocol::Agent>,
        ) -> BoxFuture<'static, agent_client_protocol::Result<T>>
        + Send
        + 'static,
{
    let _process_lock = acquire_e2e_process_lock();
    let transport = host_transport(&mut host.child);
    let scenario_done = Arc::new(tokio::sync::Notify::new());
    let scenario_done_for_client = scenario_done.clone();
    let connect = Client
        .builder()
        .on_receive_notification(
            async move |notification: EventNotification,
                        _connection: ConnectionTo<agent_client_protocol::Agent>| {
                events.lock().expect("events lock").push(notification);
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_notification(
            async move |notification: v1::SessionNotification,
                        _connection: ConnectionTo<agent_client_protocol::Agent>| {
                updates.lock().expect("updates lock").push(notification);
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_notification(
            async move |notification: echo_sdk_protocol::event::GapNotification,
                        _connection: ConnectionTo<agent_client_protocol::Agent>| {
                gaps.lock().expect("gaps lock").push(notification);
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .connect_with(transport, async move |connection| {
            let result = scenario(connection).await;
            scenario_done_for_client.notify_one();
            result
        });
    tokio::pin!(connect);
    let outcome = tokio::time::timeout(Duration::from_secs(60), async {
        tokio::select! {
            result = &mut connect => result,
            _ = scenario_done.notified() => {
                // The scenario has completed its assertions. Close the
                // source-built Host so the official Client connection can
                // finish instead of waiting for an unrelated EOF timeout.
                let _ = host.child.kill().await;
                (&mut connect).await
            }
        }
    })
    .await
    .map_err(|_| "client scenario timed out")??;
    Ok(outcome)
}

fn empty_collectors<T>() -> (
    SharedVec<T>,
    SharedVec<v1::SessionNotification>,
    SharedVec<echo_sdk_protocol::event::GapNotification>,
) {
    (
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(Vec::new())),
    )
}

fn nonzero(value: u64) -> WireNonZeroU64 {
    assert!(value >= 1);
    WireNonZeroU64::try_from(value.to_string()).expect("non-zero decimal parses")
}

async fn wait_for_model_request(
    notify: &Arc<tokio::sync::Notify>,
) -> Result<(), Box<dyn std::error::Error>> {
    tokio::time::timeout(Duration::from_secs(30), notify.notified())
        .await
        .map_err(|_| "timed out waiting for the model request".into())
}

async fn wait_until(predicate: impl Fn() -> bool) -> Result<(), Box<dyn std::error::Error>> {
    tokio::time::timeout(Duration::from_secs(20), async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .map_err(|_| "condition never became true".into())
}

// ── Scenario A: full lifecycle ──────────────────────────────────────────────

#[tokio::test]
async fn valid_hello_completes_full_core_lifecycle() -> Result<(), Box<dyn std::error::Error>> {
    let (endpoint, request_seen) = start_model_server("core-ok").await?;
    let work = tempfile::tempdir()?;
    let state_root = tempfile::tempdir()?;
    let config = write_config(work.path(), &endpoint, state_root.path(), None)?;
    let mut host = spawn_host(&config).await?;
    let (events, updates, gaps) = empty_collectors::<EventNotification>();

    let events_for_scenario = events.clone();
    let updates_for_scenario = updates.clone();
    let gaps_for_scenario = gaps.clone();
    let scenario = move |connection: ConnectionTo<agent_client_protocol::Agent>|
          -> BoxFuture<'static, agent_client_protocol::Result<()>> {
        let events = events_for_scenario.clone();
        let updates = updates_for_scenario.clone();
        let gaps = gaps_for_scenario.clone();
        Box::pin(async move {
            let initialized = connection
                .send_request(initialize_request(Some(client_hello())))
                .block_task()
                .await?;
            let advertised = initialized
                .agent_capabilities
                .meta
                .as_ref()
                .and_then(|meta| meta.get("echo_agent"))
                .ok_or_else(|| agent_client_protocol::Error::internal_error().data("no echo_agent advertisement"))?;
            let advertisement: EchoAgentCapability = serde_json::from_value(advertised.clone())
                .map_err(|error| {
                    agent_client_protocol::Error::invalid_params().data(error.to_string())
                })?;
            assert!(advertisement.validate_shape().is_empty());
            assert!(advertisement.declares(ExtensionCapability::Runs));

            let agent = connection
                .send_request(AgentCreateRequest {
                    config: AgentConfigWire::HostDefault,
                    idempotency_id: Some("e2e-agent".to_string()),
                })
                .block_task()
                .await?
                .agent;
            // Idempotent create returns the same handle.
            let again = connection
                .send_request(AgentCreateRequest {
                    config: AgentConfigWire::HostDefault,
                    idempotency_id: Some("e2e-agent".to_string()),
                })
                .block_task()
                .await?
                .agent;
            assert_eq!(agent, again);

            let describe = connection
                .send_request(AgentDescribeRequest { agent: agent.clone() })
                .block_task()
                .await?;
            assert_eq!(describe.snapshot.model_name, "fixture-model");

            let session = connection
                .send_request(SessionCreateRequest {
                    agent: agent.clone(),
                    working_dir: None,
                    session_id: None,
                    idempotency_id: None,
                })
                .block_task()
                .await?;
            assert!(!session.acp_session_id.is_empty());

            #[cfg(feature = "sdk-facade-adapters")]
            {
            let permission_mode = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/permission/op",
                    serde_json::json!({
                        "operation": "permission.mode",
                        "signature_digest": family_op_digest(
                            "permission",
                            "permission.mode"
                        ),
                        "handle": session.session,
                        "arguments": []
                    }),
                )?)
                .block_task()
                .await?;
            let permission_mode = decoded_facade_response(permission_mode).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            assert_eq!(permission_mode, serde_json::json!("default"));

            let permission_update = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/permission/op",
                    serde_json::json!({
                        "operation": "permission.apply_update",
                        "signature_digest": family_op_digest("permission", "permission.apply_update"),
                        "handle": session.session.clone(),
                        "arguments": [{
                            "kind": "map",
                            "value": [
                                {"key": {"kind": "string", "value": "type"}, "value": {"kind": "string", "value": "add_rule"}},
                                {"key": {"kind": "string", "value": "matcher"}, "value": {"kind": "string", "value": "fixture-tool"}},
                                {"key": {"kind": "string", "value": "behavior"}, "value": {"kind": "string", "value": "deny"}},
                                {"key": {"kind": "string", "value": "source"}, "value": {"kind": "string", "value": "session"}}
                            ]
                        }]
                    }),
                )?)
                .block_task()
                .await?;
            assert_eq!(decoded_facade_response(permission_update).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?, serde_json::Value::Null);
            let permission_check = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/permission/op",
                    serde_json::json!({
                        "operation": "permission.check",
                        "signature_digest": family_op_digest("permission", "permission.check"),
                        "handle": session.session.clone(),
                        "arguments": [
                            {"kind": "string", "value": "fixture-tool"},
                            {"kind": "map", "value": []}
                        ]
                    }),
                )?)
                .block_task()
                .await?;
            let permission_check = decoded_facade_response(permission_check).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            assert_eq!(permission_check.get("decision"), Some(&serde_json::json!("deny")));

            let record_approval = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/permission/op",
                    serde_json::json!({
                        "operation": "permission.record_approval",
                        "signature_digest": family_op_digest("permission", "permission.record_approval"),
                        "handle": session.session.clone(),
                        "arguments": [
                            {"kind": "string", "value": "scope-a"},
                            {"kind": "string", "value": "fixture-tool"},
                            {"kind": "map", "value": []},
                            {"kind": "string", "value": "session"}
                        ]
                    }),
                )?)
                .block_task()
                .await?;
            assert_eq!(decoded_facade_response(record_approval).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?, serde_json::Value::Null);
            let is_approved = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/permission/op",
                    serde_json::json!({
                        "operation": "permission.is_approved",
                        "signature_digest": family_op_digest("permission", "permission.is_approved"),
                        "handle": session.session.clone(),
                        "arguments": [
                            {"kind": "string", "value": "scope-a"},
                            {"kind": "string", "value": "fixture-tool"},
                            {"kind": "map", "value": []}
                        ]
                    }),
                )?)
                .block_task()
                .await?;
            assert_eq!(decoded_facade_response(is_approved).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?, serde_json::json!(true));
            }

            let started = connection
                .send_request(RunStartRequest {
                    session: session.session.clone(),
                    input: RunInput::Chat {
                        text: "hello core".to_string(),
                    },
                    idempotency_id: None,
                })
                .block_task()
                .await?;
            assert_eq!(started.run.kind, HandleKind::Run);
            assert_eq!(started.stream.kind, HandleKind::Stream);

            wait_for_model_request(&request_seen)
                .await
                .map_err(|error| agent_client_protocol::Error::internal_error().data(error.to_string()))?;

            let events_snapshot: SharedVec<EventNotification> = events.clone();
            wait_until(move || {
                events_snapshot
                    .lock()
                    .expect("events lock")
                    .iter()
                    .any(|notification: &EventNotification| {
                        matches!(
                            notification.envelope.payload.event_type.as_str(),
                            "final_answer" | "cancelled" | "error"
                        )
                    })
            })
            .await
            .map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;

            let all_events = events.lock().expect("events lock").clone();
            let terminals = all_events
                .iter()
                .filter(|notification| {
                    matches!(
                        notification.envelope.payload.event_type.as_str(),
                        "final_answer" | "cancelled" | "error"
                    )
                })
                .count();
            assert_eq!(terminals, 1, "exactly one terminal event");
            // Every event notification must bind to the announced stream.
            assert!(all_events
                .iter()
                .all(|notification| notification.stream.id == started.stream.id));

            let last_sequence = all_events
                .last()
                .and_then(|notification| notification.envelope.sequence.to_u64())
                .unwrap_or(1);
            connection.send_notification(EventAckNotification {
                ack: EventAck {
                    stream: started.stream.clone(),
                    last_processed_sequence: nonzero(last_sequence.saturating_add(1)),
                },
            })?;
            connection.send_notification(EventAckNotification {
                ack: EventAck {
                    stream: started.stream.clone(),
                    last_processed_sequence: nonzero(last_sequence.max(1)),
                },
            })?;

            let wait = connection
                .send_request(RunWaitRequest {
                    run: started.run.clone(),
                    timeout: None,
                })
                .block_task()
                .await?;
            assert!(wait.settled, "run must settle");
            let terminal = wait
                .terminal
                .ok_or_else(|| agent_client_protocol::Error::internal_error().data("no terminal"))?;
            let terminal_status = serde_json::to_value(&terminal)?
                .get("status")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            assert_eq!(terminal_status, "completed");
            assert!(wait.receipt.is_some());

            let get = connection
                .send_request(RunGetRequest { run: started.run.clone() })
                .block_task()
                .await?;
            assert_eq!(get.status, RunStatus::Completed);
            assert_eq!(get.stream.as_ref().map(|s| s.id.clone()), Some(started.stream.id.clone()));

            let replay = connection
                .send_request(ReplayRequest {
                    stream: started.stream.clone(),
                    after_sequence: WireU64::from_u64(0),
                    max_events: Some(nonzero(64)),
                })
                .block_task()
                .await?;
            assert!(!replay.events.is_empty(), "journal replay returns events");
            replay
                .validate()
                .map_err(|error| agent_client_protocol::Error::invalid_params().data(error.to_string()))?;

            // The extended standard view also projected session/update.
            assert!(
                updates
                    .lock()
                    .expect("updates lock")
                    .iter()
                    .any(|notification| matches!(
                        &notification.update,
                        v1::SessionUpdate::AgentMessageChunk(_)
                    )),
                "standard projection must accompany the extension stream"
            );
            assert!(gaps.lock().expect("gaps lock").is_empty());

            let closed_session = connection
                .send_request(SessionCloseRequest {
                    session: session.session.clone(),
                })
                .block_task()
                .await?;
            assert!(closed_session.released);
            let agent_for_close = agent.clone();
            let closed_agent = connection
                .send_request(AgentCloseRequest {
                    agent: agent_for_close.clone(),
                })
                .block_task()
                .await?;
            assert!(closed_agent.released);
            let closed_again = connection
                .send_request(AgentCloseRequest {
                    agent: agent_for_close,
                })
                .block_task()
                .await?;
            assert!(!closed_again.released);
            Ok(())
        })
    };

    let result = drive(&mut host, events, updates, gaps, scenario).await;
    let exit = tokio::time::timeout(Duration::from_secs(5), host.child.wait()).await;
    result?;
    let _ = exit;
    let stderr = stderr_text(&host);
    assert!(!stderr.contains(SENTINEL_SECRET), "secret leaked to stderr");
    Ok(())
}

#[tokio::test]
async fn tiny_live_window_emits_a_valid_gap_until_acknowledged()
-> Result<(), Box<dyn std::error::Error>> {
    let (endpoint, request_seen) = start_model_server("backpressure").await?;
    let work = tempfile::tempdir()?;
    let state_root = tempfile::tempdir()?;
    let config = write_config(
        work.path(),
        &endpoint,
        state_root.path(),
        Some(serde_json::json!({
            "max_outstanding_live_events": 1,
            "max_event_bytes": 1
        })),
    )?;
    let mut host = spawn_host(&config).await?;
    let (events, updates, gaps) = empty_collectors::<EventNotification>();
    let events_for_scenario = events.clone();
    let gaps_for_scenario = gaps.clone();
    let scenario = move |connection: ConnectionTo<agent_client_protocol::Agent>|
          -> BoxFuture<'static, agent_client_protocol::Result<()>> {
        let events = events_for_scenario.clone();
        let gaps = gaps_for_scenario.clone();
        Box::pin(async move {
            connection
                .send_request(initialize_request(Some(client_hello())))
                .block_task()
                .await?;
            let agent = connection
                .send_request(AgentCreateRequest {
                    config: AgentConfigWire::HostDefault,
                    idempotency_id: None,
                })
                .block_task()
                .await?
                .agent;
            let session = connection
                .send_request(SessionCreateRequest {
                    agent: agent.clone(),
                    working_dir: None,
                    session_id: None,
                    idempotency_id: None,
                })
                .block_task()
                .await?;
            let started = connection
                .send_request(RunStartRequest {
                    session: session.session,
                    input: RunInput::Chat {
                        text: "window test".to_string(),
                    },
                    idempotency_id: None,
                })
                .block_task()
                .await?;
            wait_for_model_request(&request_seen)
                .await
                .map_err(|error| agent_client_protocol::Error::internal_error().data(error.to_string()))?;
            wait_until({
                let gaps = gaps.clone();
                let events = events.clone();
                move || {
                    gaps.lock().map(|items| !items.is_empty()).unwrap_or(false)
                        || events.lock().map(|items| !items.is_empty()).unwrap_or(false)
                }
            })
            .await
            .map_err(|error| agent_client_protocol::Error::internal_error().data(error.to_string()))?;
            let gap = gaps
                .lock()
                .map_err(|_| agent_client_protocol::Error::internal_error().data("gap lock poisoned"))?
                .first()
                .cloned()
                .ok_or_else(|| agent_client_protocol::Error::internal_error().data("missing gap"))?;
            gap.validate()
                .map_err(|error| agent_client_protocol::Error::invalid_params().data(error.to_string()))?;
            let initial_gap_count = gaps
                .lock()
                .map_err(|_| agent_client_protocol::Error::internal_error().data("gap lock poisoned"))?
                .len();
            assert_eq!(initial_gap_count, 1, "live oversized events must coalesce behind one gap");
            connection.send_notification(EventAckNotification {
                ack: EventAck {
                    stream: started.stream.clone(),
                    last_processed_sequence: gap.gap.snapshot_watermark.clone(),
                },
            })?;
            if let Some(first) = events
                .lock()
                .map_err(|_| agent_client_protocol::Error::internal_error().data("event lock poisoned"))?
                .first()
            {
                connection.send_notification(EventAckNotification {
                    ack: EventAck {
                        stream: started.stream,
                        last_processed_sequence: first.envelope.sequence.clone(),
                    },
                })?;
            }
            Ok(())
        })
    };
    let result = drive(&mut host, events, updates, gaps, scenario).await;
    result?;
    let _ = tokio::time::timeout(Duration::from_secs(5), host.child.wait()).await;
    Ok(())
}

#[tokio::test]
async fn extended_standard_prompt_bridges_shared_core_handles()
-> Result<(), Box<dyn std::error::Error>> {
    let (endpoint, request_seen) = start_model_server("standard-bridge").await?;
    let work = tempfile::tempdir()?;
    let state_root = tempfile::tempdir()?;
    let config = write_config(work.path(), &endpoint, state_root.path(), None)?;
    let mut host = spawn_host(&config).await?;
    let (events, updates, gaps) = empty_collectors::<EventNotification>();
    let scenario = move |connection: ConnectionTo<agent_client_protocol::Agent>|
          -> BoxFuture<'static, agent_client_protocol::Result<()>> {
        Box::pin(async move {
            connection
                .send_request(initialize_request(Some(client_hello())))
                .block_task()
                .await?;
            let probe_agent = connection
                .send_request(AgentCreateRequest {
                    config: AgentConfigWire::HostDefault,
                    idempotency_id: None,
                })
                .block_task()
                .await?
                .agent;
            let cwd = std::env::current_dir()
                .map_err(|error| agent_client_protocol::Error::internal_error().data(error.to_string()))?;
            let session = connection
                .send_request(v1::NewSessionRequest::new(cwd))
                .block_task()
                .await?;
            let session_meta = session
                .meta
                .as_ref()
                .and_then(|meta| meta.get("echo_agent"))
                .ok_or_else(|| agent_client_protocol::Error::internal_error().data("missing session bridge"))?;
            let session_handle: echo_sdk_protocol::handle::WireHandle = serde_json::from_value(
                session_meta.get("session").cloned().ok_or_else(|| {
                    agent_client_protocol::Error::internal_error().data("missing session handle")
                })?,
            )
            .map_err(|error| agent_client_protocol::Error::invalid_params().data(error.to_string()))?;
            let prompt = connection
                .send_request(v1::PromptRequest::new(
                    session.session_id.clone(),
                    vec![v1::ContentBlock::Text(v1::TextContent::new("bridge me"))],
                ))
                .block_task()
                .await?;
            let prompt_meta = prompt
                .meta
                .as_ref()
                .and_then(|meta| meta.get("echo_agent"))
                .ok_or_else(|| agent_client_protocol::Error::internal_error().data("missing prompt bridge"))?;
            let run: echo_sdk_protocol::handle::WireHandle = serde_json::from_value(
                prompt_meta.get("run").cloned().ok_or_else(|| {
                    agent_client_protocol::Error::internal_error().data("missing run handle")
                })?,
            )
            .map_err(|error| agent_client_protocol::Error::invalid_params().data(error.to_string()))?;
            let stream: echo_sdk_protocol::handle::WireHandle = serde_json::from_value(
                prompt_meta.get("stream").cloned().ok_or_else(|| {
                    agent_client_protocol::Error::internal_error().data("missing stream handle")
                })?,
            )
            .map_err(|error| agent_client_protocol::Error::invalid_params().data(error.to_string()))?;
            assert_eq!(session_handle.kind, HandleKind::Session);
            assert_eq!(run.kind, HandleKind::Run);
            assert_eq!(stream.kind, HandleKind::Stream);
            let get = connection
                .send_request(RunGetRequest { run })
                .block_task()
                .await?;
            assert_eq!(get.status, RunStatus::Completed);
            wait_for_model_request(&request_seen)
                .await
                .map_err(|error| agent_client_protocol::Error::internal_error().data(error.to_string()))?;
            let replay = connection
                .send_request(ReplayRequest {
                    stream,
                    after_sequence: WireU64::from_u64(0),
                    max_events: Some(nonzero(32)),
                })
                .block_task()
                .await?;
            assert!(!replay.events.is_empty());
            connection
                .send_request(SessionCloseRequest {
                    session: session_handle,
                })
                .block_task()
                .await?;
            connection
                .send_request(AgentCloseRequest { agent: probe_agent })
                .block_task()
                .await?;
            Ok(())
        })
    };
    let result = drive(&mut host, events, updates, gaps, scenario).await;
    result?;
    let _ = tokio::time::timeout(Duration::from_secs(5), host.child.wait()).await;
    Ok(())
}

// ── Scenario B: fail-closed matrix ──────────────────────────────────────────

#[tokio::test]
async fn plain_client_and_mismatched_hello_fail_closed() -> Result<(), Box<dyn std::error::Error>> {
    let (endpoint, _request_seen) = start_model_server("unused").await?;
    let work = tempfile::tempdir()?;
    let state_root = tempfile::tempdir()?;
    let config = write_config(work.path(), &endpoint, state_root.path(), None)?;
    let mut host = spawn_host(&config).await?;
    let (events, updates, gaps) = empty_collectors();

    let scenario = move |connection: ConnectionTo<agent_client_protocol::Agent>|
          -> BoxFuture<'static, agent_client_protocol::Result<()>> {
        Box::pin(async move {
            // Plain Client: initialize carries no hello at all.
            let initialized = connection
                .send_request(initialize_request(None))
                .block_task()
                .await?;
            // The advertisement is still published; the plain Client ignores
            // it and the standard flow keeps working.
            assert!(
                initialized
                    .agent_capabilities
                    .meta
                    .as_ref()
                    .and_then(|meta| meta.get("echo_agent"))
                    .is_some()
            );
            let session = connection
                .send_request(v1::NewSessionRequest::new(
                    std::env::current_dir().expect("cwd"),
                ))
                .block_task()
                .await?;
            let _ = session.session_id;

            // Forced extension calls answer with official method-not-found
            // and no handle is ever created.
            let forced = connection
                .send_request(AgentCreateRequest {
                    config: AgentConfigWire::HostDefault,
                    idempotency_id: None,
                })
                .block_task()
                .await
                .expect_err("extension call must fail on a plain connection");
            assert!(matches!(
                forced.code,
                agent_client_protocol::ErrorCode::MethodNotFound
            ));

            // Mismatched hello: wrong extension version degrades to Standard
            // without failing initialize.
            let mut wrong = client_hello();
            wrong.extension_protocol_version = 99;
            let initialized = connection
                .send_request(initialize_request(Some(wrong)))
                .block_task()
                .await?;
            assert_eq!(initialized.protocol_version, ProtocolVersion::V1);
            let forced = connection
                .send_request(AgentCreateRequest {
                    config: AgentConfigWire::HostDefault,
                    idempotency_id: None,
                })
                .block_task()
                .await
                .expect_err("mismatched hello must stay Standard");
            assert!(matches!(
                forced.code,
                agent_client_protocol::ErrorCode::MethodNotFound
            ));
            Ok(())
        })
    };

    drive(&mut host, events, updates, gaps, scenario).await?;
    Ok(())
}

// ── Scenario C: restart recovery + crash interruption ───────────────────────

#[tokio::test]
async fn restart_recovers_history_and_marks_killed_runs_interrupted()
-> Result<(), Box<dyn std::error::Error>> {
    let work = tempfile::tempdir()?;
    let state_root = tempfile::tempdir()?;

    // Host 1: complete one settled run, then crash mid-run on a parked one.
    let (endpoint, request_seen) = start_model_server("settled-answer").await?;
    let config = write_config(work.path(), &endpoint, state_root.path(), None)?;
    let mut host1 = spawn_host(&config).await?;
    let (events, updates, gaps) = empty_collectors();

    struct FirstRun {
        session_id: String,
        settled_run: echo_sdk_protocol::handle::WireHandle,
        settled_stream: echo_sdk_protocol::handle::WireHandle,
    }
    let scenario = move |connection: ConnectionTo<agent_client_protocol::Agent>|
          -> BoxFuture<'static, agent_client_protocol::Result<FirstRun>> {
        Box::pin(async move {
            let initialized = connection
                .send_request(initialize_request(Some(client_hello())))
                .block_task()
                .await?;
            assert!(
                initialized
                    .agent_capabilities
                    .meta
                    .as_ref()
                    .and_then(|meta| meta.get("echo_agent"))
                    .is_some()
            );
            let agent = connection
                .send_request(AgentCreateRequest {
                    config: AgentConfigWire::HostDefault,
                    idempotency_id: None,
                })
                .block_task()
                .await?
                .agent;
            let session = connection
                .send_request(SessionCreateRequest {
                    agent: agent.clone(),
                    working_dir: None,
                    session_id: Some("sess_e2e_recovery".to_string()),
                    idempotency_id: None,
                })
                .block_task()
                .await?;

            let settled = connection
                .send_request(RunStartRequest {
                    session: session.session.clone(),
                    input: RunInput::Chat {
                        text: "settle me".to_string(),
                    },
                    idempotency_id: None,
                })
                .block_task()
                .await?;
            wait_for_model_request(&request_seen)
                .await
                .map_err(|error| agent_client_protocol::Error::internal_error().data(error.to_string()))?;
            let wait = connection
                .send_request(RunWaitRequest {
                    run: settled.run.clone(),
                    timeout: None,
                })
                .block_task()
                .await?;
            assert!(wait.settled);

            Ok(FirstRun {
                session_id: session.acp_session_id.clone(),
                settled_run: settled.run.clone(),
                settled_stream: settled.stream.clone(),
            })
        })
    };
    let first = drive(&mut host1, events, updates, gaps, scenario).await?;
    // Hard-kill: process exits without a close chain; state on disk stays.
    let _ = host1.child.start_kill();
    let _ = tokio::time::timeout(Duration::from_secs(5), host1.child.wait()).await;

    // Host 2 on the same state root: new generation, stale old handles,
    // session load, settled history, replay from the journal.
    let mut host2 = spawn_host(&config).await?;
    let (events2, updates2, gaps2) = empty_collectors();
    let settled_run_first = first.settled_run.clone();
    let settled_stream_first = first.settled_stream.clone();
    let session_id_first = first.session_id.clone();
    let scenario2 = move |connection: ConnectionTo<agent_client_protocol::Agent>|
          -> BoxFuture<'static, agent_client_protocol::Result<()>> {
        let settled_run_first = settled_run_first.clone();
        let settled_stream_first = settled_stream_first.clone();
        Box::pin(async move {
            let initialized = connection
                .send_request(initialize_request(Some(client_hello())))
                .block_task()
                .await?;
            assert!(
                initialized
                    .agent_capabilities
                    .meta
                    .as_ref()
                    .and_then(|meta| meta.get("echo_agent"))
                    .is_some()
            );
            let agent = connection
                .send_request(AgentCreateRequest {
                    config: AgentConfigWire::HostDefault,
                    idempotency_id: None,
                })
                .block_task()
                .await?
                .agent;

            // Pre-restart handles are stale at the new generation.
            let stale = connection
                .send_request(RunGetRequest {
                    run: settled_run_first.clone(),
                })
                .block_task()
                .await
                .expect_err("pre-restart run handle must be stale");
            let stale_data =
                echo_sdk_protocol::error::EchoSdkError::from_jsonrpc_data(stale.data.as_ref());
            assert_eq!(
                stale_data.map(|error| error.code),
                Ok(echo_sdk_protocol::error::ExtensionErrorCode::StaleHandle)
            );

            let loaded = connection
                .send_request(SessionLoadRequest {
                    agent,
                    session_id: "sess_e2e_recovery".to_string(),
                    working_dir: None,
                })
                .block_task()
                .await?;
            assert!(!loaded.runs.is_empty(), "history must be recovered");
            assert_eq!(loaded.acp_session_id, session_id_first);

            // The settled run recovered with its terminal and a fresh
            // generation; its journal replays.
            let recovered = loaded
                .runs
                .iter()
                .find(|run| run.status == RunStatus::Completed)
                .ok_or_else(|| {
                    agent_client_protocol::Error::internal_error().data("no recovered settled run")
                })?;
            assert!(recovered.terminal.is_some());
            assert!(recovered.last_sequence.to_u64().unwrap_or(0) >= 1);

            let replay = connection
                .send_request(ReplayRequest {
                    stream: recovered.stream.clone(),
                    after_sequence: WireU64::from_u64(0),
                    max_events: Some(nonzero(64)),
                })
                .block_task()
                .await?;
            assert!(!replay.events.is_empty(), "recovered journal replays");
            replay
                .validate()
                .map_err(|error| agent_client_protocol::Error::invalid_params().data(error.to_string()))?;

            // Old-generation stream handles stay fenced out of replay.
            let stale_replay = connection
                .send_request(ReplayRequest {
                    stream: settled_stream_first.clone(),
                    after_sequence: WireU64::from_u64(0),
                    max_events: Some(nonzero(8)),
                })
                .block_task()
                .await
                .expect_err("stale stream handle must fail replay");
            assert_eq!(stale_replay.code, agent_client_protocol::ErrorCode::Other(-32050));

            // `run/wait` on the settled recovered run answers immediately.
            let wait = connection
                .send_request(RunWaitRequest {
                    run: recovered.run.clone(),
                    timeout: None,
                })
                .block_task()
                .await?;
            assert!(wait.settled);
            Ok(())
        })
    };
    drive(&mut host2, events2, updates2, gaps2, scenario2).await?;
    let _ = host2.child.start_kill();
    Ok(())
}

#[tokio::test]
async fn killed_active_run_is_interrupted_never_completed() -> Result<(), Box<dyn std::error::Error>>
{
    let work = tempfile::tempdir()?;
    let state_root = tempfile::tempdir()?;
    let (endpoint, request_seen) = start_parking_model_server().await?;
    let config = write_config(work.path(), &endpoint, state_root.path(), None)?;
    let mut host1 = spawn_host(&config).await?;
    let (events, updates, gaps) = empty_collectors();

    let (started_tx, started_rx) =
        tokio::sync::oneshot::channel::<echo_sdk_protocol::handle::WireHandle>();
    let events_for_client = events.clone();
    let updates_for_client = updates.clone();
    let gaps_for_client = gaps.clone();
    let connect = {
        let request_seen = request_seen.clone();
        let transport = host_transport(&mut host1.child);
        Client
            .builder()
            .on_receive_notification(
                async move |notification: EventNotification,
                            _connection: ConnectionTo<agent_client_protocol::Agent>| {
                    events_for_client.lock().expect("events lock").push(notification);
                    Ok(())
                },
                agent_client_protocol::on_receive_notification!(),
            )
            .on_receive_notification(
                async move |notification: v1::SessionNotification,
                            _connection: ConnectionTo<agent_client_protocol::Agent>| {
                    updates_for_client.lock().expect("updates lock").push(notification);
                    Ok(())
                },
                agent_client_protocol::on_receive_notification!(),
            )
            .on_receive_notification(
                async move |notification: echo_sdk_protocol::event::GapNotification,
                            _connection: ConnectionTo<agent_client_protocol::Agent>| {
                    gaps_for_client.lock().expect("gaps lock").push(notification);
                    Ok(())
                },
                agent_client_protocol::on_receive_notification!(),
            )
            .connect_with(transport, async move |connection| {
                let _ = connection
                    .send_request(initialize_request(Some(client_hello())))
                    .block_task()
                    .await?;
                let agent = connection
                    .send_request(AgentCreateRequest {
                        config: AgentConfigWire::HostDefault,
                        idempotency_id: None,
                    })
                    .block_task()
                    .await?
                    .agent;
                let session = connection
                    .send_request(SessionCreateRequest {
                        agent,
                        working_dir: None,
                        session_id: Some("sess_e2e_crash".to_string()),
                        idempotency_id: None,
                    })
                    .block_task()
                    .await?;
                let started = connection
                    .send_request(RunStartRequest {
                        session: session.session.clone(),
                        input: RunInput::Chat {
                            text: "park forever".to_string(),
                        },
                        idempotency_id: None,
                    })
                    .block_task()
                    .await?;
                let _ = started_tx.send(started.run);
                // Wait for the model request while the connection stays open,
                // then park until the host is killed (transport EOF).
                let _ = tokio::time::timeout(Duration::from_secs(5), request_seen.notified()).await;
                std::future::pending::<agent_client_protocol::Result<()>>().await
            })
    };
    let client_task = tokio::spawn(connect);
    let _run_handle = tokio::time::timeout(Duration::from_secs(30), started_rx)
        .await
        .map_err(|_| "run never started")?
        .map_err(|_| "run start channel closed")?;
    // Kill -9 mid-run with the connection still open: no close chain, no terminal.
    host1.child.kill().await?;
    let _ = tokio::time::timeout(Duration::from_secs(5), host1.child.wait()).await;
    let _ = tokio::time::timeout(Duration::from_secs(5), client_task).await;

    let mut host2 = spawn_host(&config).await?;
    let (events2, updates2, gaps2) = empty_collectors();
    let scenario2 = move |connection: ConnectionTo<agent_client_protocol::Agent>|
          -> BoxFuture<'static, agent_client_protocol::Result<()>> {
        Box::pin(async move {
            let _ = connection
                .send_request(initialize_request(Some(client_hello())))
                .block_task()
                .await?;
            let agent = connection
                .send_request(AgentCreateRequest {
                    config: AgentConfigWire::HostDefault,
                    idempotency_id: None,
                })
                .block_task()
                .await?
                .agent;
            let loaded = connection
                .send_request(SessionLoadRequest {
                    agent,
                    session_id: "sess_e2e_crash".to_string(),
                    working_dir: None,
                })
                .block_task()
                .await?;
            let interrupted = loaded
                .runs
                .iter()
                .find(|run| run.status == RunStatus::Interrupted)
                .ok_or_else(|| {
                    agent_client_protocol::Error::internal_error()
                        .data("killed run must be recovered as interrupted")
                })?;
            assert!(interrupted.terminal.is_none());

            let get = connection
                .send_request(RunGetRequest {
                    run: interrupted.run.clone(),
                })
                .block_task()
                .await?;
            assert_eq!(get.status, RunStatus::Interrupted);
            assert!(get.terminal.is_none());
            assert!(get.receipt.is_none());

            // Waiting on an interrupted run answers typed host_exited.
            let wait = connection
                .send_request(RunWaitRequest {
                    run: interrupted.run.clone(),
                    timeout: None,
                })
                .block_task()
                .await
                .expect_err("interrupted run must not wait into success");
            let decoded =
                echo_sdk_protocol::error::EchoSdkError::from_jsonrpc_data(wait.data.as_ref());
            assert_eq!(
                decoded.map(|error| error.code),
                Ok(echo_sdk_protocol::error::ExtensionErrorCode::HostExited)
            );
            Ok(())
        })
    };
    drive(&mut host2, events2, updates2, gaps2, scenario2).await?;
    let _ = host2.child.start_kill();
    Ok(())
}

// ── Scenario D: bounded stdin frames ────────────────────────────────────────

#[tokio::test]
async fn oversized_input_frame_fails_without_side_effects() -> Result<(), Box<dyn std::error::Error>>
{
    let (endpoint, _request_seen) = start_model_server("unused").await?;
    let work = tempfile::tempdir()?;
    let state_root = tempfile::tempdir()?;
    let config = write_config(
        work.path(),
        &endpoint,
        state_root.path(),
        Some(serde_json::json!({ "max_frame_bytes": 64 })),
    )?;
    let mut host = spawn_host(&config).await?;
    {
        let mut stdin = host.child.stdin.take().expect("stdin piped");
        let oversized = format!(
            "{}{}\n",
            "{\"jsonrpc\":\"2.0\",\"method\":\"initialize\",\"params\":{\"x\":\"",
            "z".repeat(200)
        );
        stdin.write_all(oversized.as_bytes()).await?;
        stdin.flush().await?;
    }
    // The Host must fail the connection without emitting a response and exit
    // non-zero (bounded diagnostic on stderr).
    // The spawn helper drains stderr into the shared buffer; wait briefly
    // for the reader task to observe EOF after process exit.
    let host_for_stderr = {
        // Copy out the shared handle before partially moving the child.
        Arc::clone(&host.stderr)
    };
    let child = host.child;
    let output = tokio::time::timeout(Duration::from_secs(30), child.wait_with_output())
        .await
        .map_err(|_| "host did not exit after an oversized frame")??;
    assert!(
        !output.status.success(),
        "oversized frame must fail the connection"
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let stderr = loop {
        let snapshot =
            String::from_utf8_lossy(&host_for_stderr.lock().expect("stderr lock")).to_string();
        if snapshot.contains("byte limit") || std::time::Instant::now() > deadline {
            break snapshot;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    assert!(
        stderr.contains("byte limit"),
        "bounded diagnostic expected, got: {stderr}"
    );
    assert!(!stderr.contains(SENTINEL_SECRET));
    Ok(())
}

#[allow(dead_code)]
fn direction_marker(_: LineDirection) {}

// ── Facade admission ladder (plan 07 todo 2) ────────────────────────────────

#[cfg(feature = "sdk-facade-adapters")]
fn first_catalog_invoke_operation() -> Result<(String, String), Box<dyn std::error::Error>> {
    let catalog_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../contracts/sdk/facade-operation-catalog.json");
    let catalog: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(catalog_path)?)?;
    let mut operations: Vec<(String, String)> = catalog
        .get("routes")
        .and_then(|routes| routes.as_array())
        .into_iter()
        .flatten()
        .filter(|route| route.get("surface").and_then(|v| v.as_str()) == Some("invoke"))
        .filter_map(|route| {
            let operation = route.get("operation")?.as_str()?.to_string();
            let digest = route
                .get("signature_digests")?
                .as_array()?
                .first()?
                .as_str()?
                .to_string();
            Some((operation, digest))
        })
        .collect();
    operations.sort();
    operations
        .first()
        .cloned()
        .ok_or_else(|| "catalog carries no invoke identities".into())
}

#[cfg(feature = "sdk-facade-adapters")]
fn catalog_source_operations() -> Result<Vec<(String, String)>, Box<dyn std::error::Error>> {
    let catalog_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../contracts/sdk/facade-operation-catalog.json");
    let catalog: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(catalog_path)?)?;
    let mut operations: Vec<(String, String)> = catalog
        .get("routes")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter(|route| route.get("surface").and_then(serde_json::Value::as_str) == Some("invoke"))
        .filter(|route| {
            route.get("family").and_then(serde_json::Value::as_str) == Some("source_operation")
        })
        .filter_map(|route| {
            let operation = route.get("operation")?.as_str()?.to_string();
            let digest = route
                .get("signature_digests")?
                .as_array()?
                .first()?
                .as_str()?
                .to_string();
            Some((operation, digest))
        })
        .collect();
    operations.sort();
    Ok(operations)
}

#[cfg(feature = "sdk-facade-adapters")]
fn catalog_invoke_digest(operation: &str) -> Result<String, Box<dyn std::error::Error>> {
    let catalog_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../contracts/sdk/facade-operation-catalog.json");
    let catalog: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(catalog_path)?)?;
    let routes = catalog
        .get("routes")
        .and_then(|routes| routes.as_array())
        .into_iter()
        .flatten();
    let direct = routes
        .clone()
        .find(|route| route.get("operation").and_then(|value| value.as_str()) == Some(operation))
        .and_then(|route| route.get("signature_digests"))
        .and_then(|digests| digests.as_array())
        .and_then(|digests| digests.first())
        .and_then(|digest| digest.as_str())
        .map(str::to_string);
    direct
        .or_else(|| {
            routes
                .flat_map(|route| {
                    route
                        .get("operation_signatures")
                        .and_then(serde_json::Value::as_array)
                        .into_iter()
                        .flatten()
                })
                .find(|entry| {
                    entry.get("operation").and_then(serde_json::Value::as_str) == Some(operation)
                })
                .and_then(|entry| entry.get("signature_digests"))
                .and_then(serde_json::Value::as_array)
                .and_then(|digests| digests.first())
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .ok_or_else(|| format!("catalog has no digest for {operation}").into())
}

#[cfg(feature = "sdk-facade-adapters")]
fn family_op_digest(family: &str, operation: &str) -> String {
    echo_sdk_protocol::facade::family_operation_signature_digest(family, operation)
}

#[cfg(feature = "sdk-facade-adapters")]
fn decoded_facade_response(
    value: serde_json::Value,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let response: echo_sdk_protocol::methods::FeatureOperationResponse =
        serde_json::from_value(value)?;
    response
        .value
        .into_json()
        .map_err(|error| -> Box<dyn std::error::Error> { error.to_string().into() })
}

#[cfg(feature = "sdk-facade-adapters")]
fn typed_facade_error(
    error: &agent_client_protocol::Error,
) -> Result<echo_sdk_protocol::error::EchoSdkError, Box<dyn std::error::Error>> {
    echo_sdk_protocol::error::EchoSdkError::from_jsonrpc_data(error.data.as_ref())
        .map_err(|message| -> Box<dyn std::error::Error> { message.into() })
}

#[cfg(feature = "sdk-facade-adapters")]
async fn invoke_facade(
    connection: &ConnectionTo<agent_client_protocol::Agent>,
    operation: &str,
    handle: &WireHandle,
    arguments: Vec<serde_json::Value>,
) -> agent_client_protocol::Result<serde_json::Value> {
    let signature_digest = catalog_invoke_digest(operation)
        .map_err(|error| agent_client_protocol::Error::internal_error().data(error.to_string()))?;
    let response = connection
        .send_request(UntypedMessage::new(
            "_echo_agent/facade/invoke",
            serde_json::json!({
                "operation": operation,
                "signature_digest": signature_digest,
                "handle": handle,
                "arguments": arguments,
            }),
        )?)
        .block_task()
        .await?;
    decoded_facade_response(response)
        .map_err(|error| agent_client_protocol::Error::internal_error().data(error.to_string()))
}

#[cfg(feature = "sdk-facade-adapters")]
async fn invoke_facade_wire(
    connection: &ConnectionTo<agent_client_protocol::Agent>,
    operation: &str,
    handle: &WireHandle,
    arguments: Vec<WireValue>,
) -> agent_client_protocol::Result<WireValue> {
    let signature_digest = catalog_invoke_digest(operation)
        .map_err(|error| agent_client_protocol::Error::internal_error().data(error.to_string()))?;
    let request = echo_sdk_protocol::methods::FeatureOperationRequest {
        operation: operation.to_string(),
        signature_digest,
        handle: Some(handle.clone()),
        arguments,
    };
    let response = connection
        .send_request(UntypedMessage::new(
            "_echo_agent/facade/invoke",
            serde_json::to_value(request).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?,
        )?)
        .block_task()
        .await?;
    let response: echo_sdk_protocol::methods::FeatureOperationResponse =
        serde_json::from_value(response).map_err(|error| {
            agent_client_protocol::Error::internal_error().data(error.to_string())
        })?;
    Ok(response.value)
}

#[cfg(feature = "sdk-facade-adapters")]
async fn invoke_family_wire(
    connection: &ConnectionTo<agent_client_protocol::Agent>,
    method: &str,
    family: &str,
    operation: &str,
    handle: &WireHandle,
    arguments: Vec<WireValue>,
) -> agent_client_protocol::Result<WireValue> {
    let request = echo_sdk_protocol::methods::FeatureOperationRequest {
        operation: operation.to_string(),
        signature_digest: family_op_digest(family, operation),
        handle: Some(handle.clone()),
        arguments,
    };
    let response = connection
        .send_request(UntypedMessage::new(
            method,
            serde_json::to_value(request).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?,
        )?)
        .block_task()
        .await?;
    let response: echo_sdk_protocol::methods::FeatureOperationResponse =
        serde_json::from_value(response).map_err(|error| {
            agent_client_protocol::Error::internal_error().data(error.to_string())
        })?;
    Ok(response.value)
}

#[cfg(feature = "sdk-facade-adapters")]
fn test_wire_path(path: &Path) -> Result<WireValue, Box<dyn std::error::Error>> {
    let path = path
        .to_str()
        .ok_or_else(|| "test fixture path is not UTF-8".to_string())?;
    Ok(WireValue::Path(WirePath::Utf8 {
        path: path.to_string(),
    }))
}

#[cfg(feature = "sdk-facade-adapters")]
fn test_wire_bytes(bytes: &[u8]) -> WireValue {
    WireValue::Bytes(WireBytes {
        base64: base64::engine::general_purpose::STANDARD_NO_PAD.encode(bytes),
    })
}

#[cfg(feature = "sdk-facade-adapters")]
fn decode_wire_bytes(value: WireValue) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let WireValue::Bytes(bytes) = value else {
        return Err("facade result is not Bytes".into());
    };
    base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(bytes.base64)
        .map_err(|error| error.into())
}

#[cfg(feature = "sdk-facade-adapters")]
async fn invoke_facade_without_handle_wire(
    connection: &ConnectionTo<agent_client_protocol::Agent>,
    operation: &str,
    arguments: Vec<serde_json::Value>,
) -> agent_client_protocol::Result<echo_sdk_protocol::scalar::WireValue> {
    let signature_digest = catalog_invoke_digest(operation)
        .map_err(|error| agent_client_protocol::Error::internal_error().data(error.to_string()))?;
    let response = connection
        .send_request(UntypedMessage::new(
            "_echo_agent/facade/invoke",
            serde_json::json!({
                "operation": operation,
                "signature_digest": signature_digest,
                "arguments": arguments,
            }),
        )?)
        .block_task()
        .await?;
    let response: echo_sdk_protocol::methods::FeatureOperationResponse =
        serde_json::from_value(response).map_err(|error| {
            agent_client_protocol::Error::internal_error().data(error.to_string())
        })?;
    Ok(response.value)
}

#[cfg(feature = "sdk-facade-adapters")]
fn session_argument(handle: &WireHandle) -> serde_json::Value {
    serde_json::json!({"kind": "handle", "value": handle})
}

#[cfg(feature = "sdk-facade-adapters")]
async fn assert_facade_invalid(
    connection: &ConnectionTo<agent_client_protocol::Agent>,
    operation: &str,
    handle: &WireHandle,
    arguments: Vec<serde_json::Value>,
) -> agent_client_protocol::Result<()> {
    let result = invoke_facade(connection, operation, handle, arguments).await;
    let error = result.err().ok_or_else(|| {
        agent_client_protocol::Error::internal_error().data(format!(
            "{operation} unexpectedly accepted invalid arguments"
        ))
    })?;
    let typed = typed_facade_error(&error)
        .map_err(|error| agent_client_protocol::Error::internal_error().data(error.to_string()))?;
    assert_eq!(typed.code, ExtensionErrorCode::InvalidValue);
    Ok(())
}

#[cfg(feature = "sdk-facade-adapters")]
#[tokio::test]
async fn plain_clients_get_method_not_found_for_facade_methods()
-> Result<(), Box<dyn std::error::Error>> {
    let (endpoint, _request_seen) = start_model_server("unused").await?;
    let work = tempfile::tempdir()?;
    let state_root = tempfile::tempdir()?;
    let config = write_config(work.path(), &endpoint, state_root.path(), None)?;
    let mut host = spawn_host(&config).await?;
    let (events, updates, gaps) = empty_collectors::<EventNotification>();

    drive(&mut host, events, updates, gaps, move |connection| {
        Box::pin(async move {
            use agent_client_protocol::UntypedMessage;
            connection
                .send_request(initialize_request(None))
                .block_task()
                .await?;
            let invoke = serde_json::json!({
                "operation": "echo_agent::evolution::review::ReviewEngine",
                "signature_digest":
                    "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                "arguments": [],
            });
            let forced = connection
                .send_request(UntypedMessage::new("_echo_agent/facade/invoke", &invoke)?)
                .block_task()
                .await
                .expect_err("facade invoke must fail on a plain connection");
            assert!(matches!(
                forced.code,
                agent_client_protocol::ErrorCode::MethodNotFound
            ));
            let family = connection
                .send_request(UntypedMessage::new("_echo_agent/memory/op", &invoke)?)
                .block_task()
                .await
                .expect_err("family method must fail on a plain connection");
            assert!(matches!(
                family.code,
                agent_client_protocol::ErrorCode::MethodNotFound
            ));
            Ok(())
        })
    })
    .await?;
    let _ = host.child.kill().await;
    Ok(())
}

#[cfg(feature = "sdk-facade-adapters")]
#[tokio::test]
async fn negotiated_facade_admission_fails_closed() -> Result<(), Box<dyn std::error::Error>> {
    let (endpoint, _request_seen) = start_model_server("unused").await?;
    let work = tempfile::tempdir()?;
    let state_root = tempfile::tempdir()?;
    let skills_dir = work.path().join("skill-fixtures");
    let skill_dir = skills_dir.join("projection-skill");
    std::fs::create_dir_all(&skill_dir)?;
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: projection-skill\ndescription: A skill used by facade projection tests.\n---\n\nProjection fixture instructions.\n",
    )?;
    std::fs::write(work.path().join("AGENTS.md"), "project injected prompt")?;
    let config = write_config(work.path(), &endpoint, state_root.path(), None)?;
    let mut host = spawn_host(&config).await?;
    let (events, updates, gaps) = empty_collectors::<EventNotification>();
    let (known_operation, known_digest) = first_catalog_invoke_operation()?;

    drive(&mut host, events, updates, gaps, move |connection| {
        let known_operation = known_operation.clone();
        let known_digest = known_digest.clone();
        let skills_dir = skills_dir.clone();
        Box::pin(async move {
            use agent_client_protocol::UntypedMessage;
            use echo_sdk_protocol::error::ExtensionErrorCode;
            let initialized = connection
                .send_request(initialize_request(Some(client_hello())))
                .block_task()
                .await?;
            // The facade runtime advertises the feature-surfaces capability.
            let advertisement = initialized
                .agent_capabilities
                .meta
                .as_ref()
                .and_then(|meta| meta.get("echo_agent"))
                .ok_or_else(|| {
                    agent_client_protocol::Error::internal_error().data("no advertisement")
                })?;
            let advertisement: EchoAgentCapability = serde_json::from_value(advertisement.clone())
                .map_err(|error| {
                    agent_client_protocol::Error::invalid_params().data(error.to_string())
                })?;
            assert!(advertisement.declares(ExtensionCapability::FeatureSurfaces));

            let agent = connection
                .send_request(AgentCreateRequest {
                    config: AgentConfigWire::HostDefault,
                    idempotency_id: Some("source-operation-agent".to_string()),
                })
                .block_task()
                .await?
                .agent;
            let source_operation = "echo_core::agent::Agent::current_run_id";
            let source = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/facade/invoke",
                    serde_json::json!({
                        "operation": source_operation,
                        "signature_digest": catalog_invoke_digest(source_operation)
                            .map_err(|error| {
                                agent_client_protocol::Error::internal_error()
                                    .data(error.to_string())
                            })?,
                        "handle": agent.clone(),
                        "arguments": [],
                    }),
                )?)
                .block_task()
                .await?;
            let source_value = decoded_facade_response(source).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            assert_eq!(source_value, serde_json::Value::Null);

            let name_operation = "echo_core::agent::Agent::name";
            let name = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/facade/invoke",
                    serde_json::json!({
                        "operation": name_operation,
                        "signature_digest": catalog_invoke_digest(name_operation)
                            .map_err(|error| {
                                agent_client_protocol::Error::internal_error()
                                    .data(error.to_string())
                            })?,
                        "handle": agent.clone(),
                        "arguments": [],
                    }),
                )?)
                .block_task()
                .await?;
            let name = decoded_facade_response(name).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            assert!(name.as_str().is_some_and(|value| !value.is_empty()));

            let react_prompt_operation = "echo_agent::agent::react::ReactAgent::current_system_prompt";
            let react_prompt = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/facade/invoke",
                    serde_json::json!({
                        "operation": react_prompt_operation,
                        "signature_digest": catalog_invoke_digest(react_prompt_operation)
                            .map_err(|error| {
                                agent_client_protocol::Error::internal_error()
                                    .data(error.to_string())
                            })?,
                        "handle": agent.clone(),
                        "arguments": [],
                    }),
                )?)
                .block_task()
                .await?;
            let react_prompt = decoded_facade_response(react_prompt).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            assert!(react_prompt.as_str().is_some_and(|value| !value.is_empty()));

            let session = connection
                .send_request(SessionCreateRequest {
                    agent: agent.clone(),
                    working_dir: None,
                    session_id: None,
                    idempotency_id: Some("source-operation-session".to_string()),
                })
                .block_task()
                .await?;

            let set_prompt = "echo_core::agent::Agent::set_system_prompt";
            invoke_facade(
                &connection,
                set_prompt,
                &agent,
                vec![
                    session_argument(&session.session),
                    serde_json::json!({"kind": "string", "value": "runtime override"}),
                ],
            )
            .await?;
            let set_working_dir = "echo_core::agent::Agent::set_working_dir";
            invoke_facade(
                &connection,
                set_working_dir,
                &agent,
                vec![
                    session_argument(&session.session),
                    serde_json::json!({
                        "kind": "path",
                        "value": {
                            "encoding": "utf8",
                            "path": work.path().display().to_string(),
                        }
                    }),
                ],
            )
            .await?;
            let current_prompt = invoke_facade(
                &connection,
                react_prompt_operation,
                &agent,
                vec![session_argument(&session.session)],
            )
            .await?;
            assert_eq!(current_prompt, serde_json::json!("runtime override"));

            let load_skills_operation =
                "echo_agent::agent::react::ReactAgent::load_skills_from_dir";
            let load_skills = invoke_facade(
                &connection,
                load_skills_operation,
                &agent,
                vec![
                    session_argument(&session.session),
                    serde_json::json!({
                        "kind": "path",
                        "value": {
                            "encoding": "utf8",
                            "path": skills_dir.display().to_string(),
                        },
                    }),
                ],
            )
            .await?;
            assert_eq!(load_skills, serde_json::json!(["projection-skill"]));

            // ReactAgent::list_skills returns the typed SkillInfo projection,
            // not the string-only skill_names accessor. This exact source
            // identity must retain description/tool metadata across the wire.
            let list_skills_operation =
                "echo_agent::agent::react::ReactAgent::list_skills";
            let listed_skills = invoke_facade(
                &connection,
                list_skills_operation,
                &agent,
                vec![session_argument(&session.session)],
            )
            .await?;
            let listed_skills = listed_skills.as_array().ok_or_else(|| {
                agent_client_protocol::Error::internal_error()
                    .data("ReactAgent::list_skills did not return an array")
            })?;
            // The fixture above is file-based, while this concrete API lists
            // code-based skills only. The important contract is the typed
            // array projection; file-based descriptors are covered below by
            // the registry descriptor route.
            assert!(listed_skills.iter().all(|skill| {
                skill.get("name").and_then(serde_json::Value::as_str).is_some()
                    && skill
                        .get("tool_names")
                        .and_then(serde_json::Value::as_array)
                        .is_some()
            }));

            // SkillRegistry read projections use the same Session Agent
            // authority as the normal Agent accessors; there is no second
            // registry handle or process-local resource map.
            let registry_count_operation =
                "echo_execution::skills::registry::SkillRegistry::count";
            let registry_count = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/facade/invoke",
                    serde_json::json!({
                        "operation": registry_count_operation,
                        "signature_digest": catalog_invoke_digest(registry_count_operation)
                            .map_err(|error| {
                                agent_client_protocol::Error::internal_error()
                                    .data(error.to_string())
                            })?,
                        "handle": agent.clone(),
                        "arguments": [
                            {"kind": "handle", "value": session.session.clone()}
                        ],
                    }),
                )?)
                .block_task()
                .await?;
            let registry_count = decoded_facade_response(registry_count).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            assert!(registry_count
                .as_str()
                .and_then(|value| value.parse::<u64>().ok())
                .is_some());

            let registry_descriptors_operation =
                "echo_execution::skills::registry::SkillRegistry::list_descriptors";
            let registry_descriptors = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/facade/invoke",
                    serde_json::json!({
                        "operation": registry_descriptors_operation,
                        "signature_digest": catalog_invoke_digest(registry_descriptors_operation)
                            .map_err(|error| {
                                agent_client_protocol::Error::internal_error()
                                    .data(error.to_string())
                            })?,
                        "handle": agent.clone(),
                        "arguments": [
                            {"kind": "handle", "value": session.session.clone()}
                        ],
                    }),
                )?)
                .block_task()
                .await?;
            let registry_descriptors =
                decoded_facade_response(registry_descriptors).map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })?;
            assert!(registry_descriptors.is_array());

            let registry_installed_operation =
                "echo_execution::skills::registry::SkillRegistry::is_installed";
            let registry_installed = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/facade/invoke",
                    serde_json::json!({
                        "operation": registry_installed_operation,
                        "signature_digest": catalog_invoke_digest(registry_installed_operation)
                            .map_err(|error| {
                                agent_client_protocol::Error::internal_error()
                                    .data(error.to_string())
                            })?,
                        "handle": agent.clone(),
                        "arguments": [
                            {"kind": "handle", "value": session.session.clone()},
                            {"kind": "string", "value": "__missing_skill__"}
                        ],
                    }),
                )?)
                .block_task()
                .await?;
            assert_eq!(decoded_facade_response(registry_installed).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?, serde_json::Value::Bool(false));

            let registry_code_skills_operation =
                "echo_execution::skills::registry::SkillRegistry::list_code_skills";
            let registry_code_skills = invoke_facade(
                &connection,
                registry_code_skills_operation,
                &agent,
                vec![session_argument(&session.session)],
            )
            .await?;
            assert_eq!(registry_code_skills, serde_json::json!([]));

            let registry_descriptor_operation =
                "echo_execution::skills::registry::SkillRegistry::get_descriptor";
            let registry_descriptor = invoke_facade(
                &connection,
                registry_descriptor_operation,
                &agent,
                vec![
                    session_argument(&session.session),
                    serde_json::json!({"kind": "string", "value": "projection-skill"}),
                ],
            )
            .await?;
            assert_eq!(
                registry_descriptor.get("name"),
                Some(&serde_json::json!("projection-skill"))
            );
            assert_eq!(
                registry_descriptor.get("description"),
                Some(&serde_json::json!(
                    "A skill used by facade projection tests."
                ))
            );
            assert!(registry_descriptor.get("location").is_none());

            let registry_code_skill_operation =
                "echo_execution::skills::registry::SkillRegistry::get_code_skill";
            let registry_code_skill = invoke_facade(
                &connection,
                registry_code_skill_operation,
                &agent,
                vec![
                    session_argument(&session.session),
                    serde_json::json!({"kind": "string", "value": "__missing_skill__"}),
                ],
            )
            .await?;
            assert_eq!(registry_code_skill, serde_json::Value::Null);

            let registry_allowed_tools_operation =
                "echo_execution::skills::registry::SkillRegistry::active_skill_allowed_tools";
            let registry_allowed_tools = invoke_facade(
                &connection,
                registry_allowed_tools_operation,
                &agent,
                vec![session_argument(&session.session)],
            )
            .await?;
            assert_eq!(registry_allowed_tools, serde_json::Value::Null);

            let registry_catalog_operation =
                "echo_execution::skills::registry::SkillRegistry::catalog_prompt";
            let registry_catalog = invoke_facade(
                &connection,
                registry_catalog_operation,
                &agent,
                vec![session_argument(&session.session)],
            )
            .await?;
            let registry_catalog = registry_catalog
                .as_str()
                .ok_or_else(|| {
                    agent_client_protocol::Error::internal_error()
                        .data("SkillRegistry::catalog_prompt did not return a string")
                })?;
            assert!(registry_catalog.contains("projection-skill"));

            let registry_sandbox_operation =
                "echo_execution::skills::registry::SkillRegistry::get_active_sandbox_policy";
            let registry_sandbox = invoke_facade(
                &connection,
                registry_sandbox_operation,
                &agent,
                vec![
                    session_argument(&session.session),
                    serde_json::json!({"kind": "string", "value": "projection-skill"}),
                ],
            )
            .await?;
            assert_eq!(registry_sandbox, serde_json::Value::Null);

            let registry_dependencies_operation =
                "echo_execution::skills::registry::SkillRegistry::get_dependency_tree";
            let registry_dependencies = invoke_facade(
                &connection,
                registry_dependencies_operation,
                &agent,
                vec![
                    session_argument(&session.session),
                    serde_json::json!({"kind": "string", "value": "projection-skill"}),
                ],
            )
            .await?;
            assert_eq!(registry_dependencies, serde_json::json!([]));

            // SkillRegistry source mutations run against the same Session
            // Agent authority as the read projections above. Exercise each
            // safe mutation and verify the descriptor projection changes.
            let registry_tag_operation =
                "echo_execution::skills::registry::SkillRegistry::tag_source";
            let registry_unregister_operation =
                "echo_execution::skills::registry::SkillRegistry::unregister_by_source";
            let registry_unregister_names_operation =
                "echo_execution::skills::registry::SkillRegistry::unregister_names_by_source";
            let registry_remove_operation =
                "echo_execution::skills::registry::SkillRegistry::remove_descriptor";
            let skill_names = serde_json::json!({
                "kind": "list",
                "value": [{"kind": "string", "value": "projection-skill"}]
            });
            let tag_result = invoke_facade(
                &connection,
                registry_tag_operation,
                &agent,
                vec![
                    session_argument(&session.session),
                    skill_names.clone(),
                    serde_json::json!({"kind": "string", "value": "fixture:source-a"}),
                ],
            )
            .await?;
            assert_eq!(tag_result, serde_json::Value::Null);
            let unregister_count = invoke_facade(
                &connection,
                registry_unregister_operation,
                &agent,
                vec![
                    session_argument(&session.session),
                    serde_json::json!({"kind": "string", "value": "fixture:source-a"}),
                ],
            )
            .await?;
            assert_eq!(unregister_count, serde_json::json!("1"));
            let descriptors_after_source_unload = invoke_facade(
                &connection,
                registry_descriptors_operation,
                &agent,
                vec![session_argument(&session.session)],
            )
            .await?;
            assert!(descriptors_after_source_unload
                .as_array()
                .is_some_and(|descriptors| descriptors.is_empty()));

            // Reload the same fixture to cover the Vec<String> source unload
            // result independently from the usize count operation.
            let load_skills = invoke_facade(
                &connection,
                load_skills_operation,
                &agent,
                vec![
                    session_argument(&session.session),
                    serde_json::json!({
                        "kind": "path",
                        "value": {
                            "encoding": "utf8",
                            "path": skills_dir.display().to_string(),
                        },
                    }),
                ],
            )
            .await?;
            assert_eq!(load_skills, serde_json::json!(["projection-skill"]));
            invoke_facade(
                &connection,
                registry_tag_operation,
                &agent,
                vec![
                    session_argument(&session.session),
                    skill_names,
                    serde_json::json!({"kind": "string", "value": "fixture:source-b"}),
                ],
            )
            .await?;
            let unregister_names = invoke_facade(
                &connection,
                registry_unregister_names_operation,
                &agent,
                vec![
                    session_argument(&session.session),
                    serde_json::json!({"kind": "string", "value": "fixture:source-b"}),
                ],
            )
            .await?;
            assert_eq!(unregister_names, serde_json::json!(["projection-skill"]));

            // A final reload leaves an untagged descriptor for the direct
            // remove_descriptor bool result.
            invoke_facade(
                &connection,
                load_skills_operation,
                &agent,
                vec![
                    session_argument(&session.session),
                    serde_json::json!({
                        "kind": "path",
                        "value": {
                            "encoding": "utf8",
                            "path": skills_dir.display().to_string(),
                        },
                    }),
                ],
            )
            .await?;
            let removed_descriptor = invoke_facade(
                &connection,
                registry_remove_operation,
                &agent,
                vec![
                    session_argument(&session.session),
                    serde_json::json!({"kind": "string", "value": "projection-skill"}),
                ],
            )
            .await?;
            assert_eq!(removed_descriptor, serde_json::Value::Bool(true));
            let descriptors_after_direct_remove = invoke_facade(
                &connection,
                registry_descriptors_operation,
                &agent,
                vec![session_argument(&session.session)],
            )
            .await?;
            assert!(descriptors_after_direct_remove
                .as_array()
                .is_some_and(|descriptors| descriptors.is_empty()));

            for (operation, arguments) in [
                (
                    registry_tag_operation,
                    vec![
                        session_argument(&session.session),
                        serde_json::json!({"kind": "string", "value": "projection-skill"}),
                        serde_json::json!({"kind": "string", "value": "fixture:bad"}),
                    ],
                ),
                (
                    registry_unregister_operation,
                    vec![
                        session_argument(&session.session),
                        serde_json::json!({"kind": "list", "value": []}),
                    ],
                ),
                (
                    registry_unregister_names_operation,
                    vec![session_argument(&session.session)],
                ),
                (
                    registry_remove_operation,
                    vec![
                        session_argument(&session.session),
                        serde_json::json!({"kind": "list", "value": []}),
                    ],
                ),
            ] {
                assert_facade_invalid(&connection, operation, &agent, arguments)
                    .await
                    .map_err(|error| {
                        agent_client_protocol::Error::internal_error().data(error.to_string())
                    })?;
            }

            for operation in [
                registry_code_skills_operation,
                registry_allowed_tools_operation,
                registry_catalog_operation,
            ] {
                assert_facade_invalid(
                    &connection,
                    operation,
                    &agent,
                    vec![
                        session_argument(&session.session),
                        serde_json::json!({"kind": "string", "value": "unexpected"}),
                    ],
                )
                .await
                .map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })?;
            }
            for operation in [
                registry_descriptor_operation,
                registry_code_skill_operation,
                registry_sandbox_operation,
                registry_dependencies_operation,
            ] {
                assert_facade_invalid(
                    &connection,
                    operation,
                    &agent,
                    vec![session_argument(&session.session)],
                )
                .await
                .map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })?;
            }

            let wrong_registry_args = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/facade/invoke",
                    serde_json::json!({
                        "operation": registry_count_operation,
                        "signature_digest": catalog_invoke_digest(registry_count_operation)
                            .map_err(|error| {
                                agent_client_protocol::Error::internal_error()
                                    .data(error.to_string())
                            })?,
                        "handle": agent.clone(),
                        "arguments": [],
                    }),
                )?)
                .block_task()
                .await
                .expect_err("SkillRegistry count requires a Session handle");
            let typed = typed_facade_error(&wrong_registry_args).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            assert_eq!(typed.code, ExtensionErrorCode::InvalidValue);

            let wrong_registry_handle = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/facade/invoke",
                    serde_json::json!({
                        "operation": registry_count_operation,
                        "signature_digest": catalog_invoke_digest(registry_count_operation)
                            .map_err(|error| {
                                agent_client_protocol::Error::internal_error()
                                    .data(error.to_string())
                            })?,
                        "handle": session.session.clone(),
                        "arguments": [
                            {"kind": "handle", "value": session.session.clone()}
                        ],
                    }),
                )?)
                .block_task()
                .await
                .expect_err("SkillRegistry source operation requires an Agent handle");
            let typed = typed_facade_error(&wrong_registry_handle).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            assert_eq!(typed.code, ExtensionErrorCode::InvalidValue);

            let set_plan_mode_operation = "echo_agent::agent::react::ReactAgent::set_plan_mode";
            let set_plan_mode = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/facade/invoke",
                    serde_json::json!({
                        "operation": set_plan_mode_operation,
                        "signature_digest": catalog_invoke_digest(set_plan_mode_operation)
                            .map_err(|error| {
                                agent_client_protocol::Error::internal_error()
                                    .data(error.to_string())
                            })?,
                        "handle": agent.clone(),
                        "arguments": [
                            {"kind": "handle", "value": session.session.clone()},
                            {"kind": "bool", "value": true}
                        ],
                    }),
                )?)
                .block_task()
                .await?;
            assert_eq!(decoded_facade_response(set_plan_mode).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?, serde_json::Value::Null);
            let is_plan_mode_operation = "echo_agent::agent::react::ReactAgent::is_plan_mode";
            let is_plan_mode = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/facade/invoke",
                    serde_json::json!({
                        "operation": is_plan_mode_operation,
                        "signature_digest": catalog_invoke_digest(is_plan_mode_operation)
                            .map_err(|error| {
                                agent_client_protocol::Error::internal_error()
                                    .data(error.to_string())
                            })?,
                        "handle": agent.clone(),
                        "arguments": [
                            {"kind": "handle", "value": session.session.clone()}
                        ],
                    }),
                )?)
                .block_task()
                .await?;
            assert_eq!(decoded_facade_response(is_plan_mode).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?, serde_json::Value::Bool(true));
            let set_max_iterations_operation =
                "echo_agent::agent::react::ReactAgent::set_max_iterations";
            let set_max_iterations = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/facade/invoke",
                    serde_json::json!({
                        "operation": set_max_iterations_operation,
                        "signature_digest": catalog_invoke_digest(set_max_iterations_operation)
                            .map_err(|error| {
                                agent_client_protocol::Error::internal_error()
                                    .data(error.to_string())
                            })?,
                        "handle": agent.clone(),
                        "arguments": [
                            {"kind": "handle", "value": session.session.clone()},
                            {"kind": "u64", "value": "7"}
                        ],
                    }),
                )?)
                .block_task()
                .await?;
            assert_eq!(decoded_facade_response(set_max_iterations).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?, serde_json::Value::Null);
            let max_iterations_operation = "echo_agent::agent::react::ReactAgent::max_iterations";
            let max_iterations = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/facade/invoke",
                    serde_json::json!({
                        "operation": max_iterations_operation,
                        "signature_digest": catalog_invoke_digest(max_iterations_operation)
                            .map_err(|error| {
                                agent_client_protocol::Error::internal_error()
                                    .data(error.to_string())
                            })?,
                        "handle": agent.clone(),
                        "arguments": [
                            {"kind": "handle", "value": session.session.clone()}
                        ],
                    }),
                )?)
                .block_task()
                .await?;
            assert_eq!(decoded_facade_response(max_iterations).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?, serde_json::json!("7"));
            let set_permission_mode_operation =
                "echo_agent::agent::react::ReactAgent::set_permission_mode";
            let set_permission_mode = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/facade/invoke",
                    serde_json::json!({
                        "operation": set_permission_mode_operation,
                        "signature_digest": catalog_invoke_digest(set_permission_mode_operation)
                            .map_err(|error| {
                                agent_client_protocol::Error::internal_error()
                                    .data(error.to_string())
                            })?,
                        "handle": agent.clone(),
                        "arguments": [
                            {"kind": "handle", "value": session.session.clone()},
                            {"kind": "string", "value": "strict"}
                        ],
                    }),
                )?)
                .block_task()
                .await?;
            assert_eq!(decoded_facade_response(set_permission_mode).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?, serde_json::Value::Null);
            let get_permission_mode_operation =
                "echo_agent::agent::react::ReactAgent::get_permission_mode";
            let get_permission_mode = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/facade/invoke",
                    serde_json::json!({
                        "operation": get_permission_mode_operation,
                        "signature_digest": catalog_invoke_digest(get_permission_mode_operation)
                            .map_err(|error| {
                                agent_client_protocol::Error::internal_error()
                                    .data(error.to_string())
                            })?,
                        "handle": agent.clone(),
                        "arguments": [
                            {"kind": "handle", "value": session.session.clone()}
                        ],
                    }),
                )?)
                .block_task()
                .await?;
            assert_eq!(decoded_facade_response(get_permission_mode).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?, serde_json::json!("strict"));
            let context_stats_operation = "echo_agent::agent::react::ReactAgent::context_stats";
            let context_stats = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/facade/invoke",
                    serde_json::json!({
                        "operation": context_stats_operation,
                        "signature_digest": catalog_invoke_digest(context_stats_operation)
                            .map_err(|error| {
                                agent_client_protocol::Error::internal_error()
                                    .data(error.to_string())
                            })?,
                        "handle": agent.clone(),
                        "arguments": [
                            {"kind": "handle", "value": session.session.clone()}
                        ],
                    }),
                )?)
                .block_task()
                .await?;
            let context_stats = decoded_facade_response(context_stats).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            assert!(context_stats.is_array());
            assert_eq!(context_stats.as_array().map(Vec::len), Some(2));
            let snapshot_operation = "echo_agent::agent::react::ReactAgent::snapshot";
            let snapshot = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/facade/invoke",
                    serde_json::json!({
                        "operation": snapshot_operation,
                        "signature_digest": catalog_invoke_digest(snapshot_operation)
                            .map_err(|error| {
                                agent_client_protocol::Error::internal_error()
                                    .data(error.to_string())
                            })?,
                        "handle": agent.clone(),
                        "arguments": [
                            {"kind": "handle", "value": session.session.clone()}
                        ],
                    }),
                )?)
                .block_task()
                .await?;
            let snapshot = decoded_facade_response(snapshot).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            assert!(snapshot.is_null() || snapshot.is_string());
            let snapshots_operation = "echo_agent::agent::react::ReactAgent::snapshots";
            let snapshots = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/facade/invoke",
                    serde_json::json!({
                        "operation": snapshots_operation,
                        "signature_digest": catalog_invoke_digest(snapshots_operation)
                            .map_err(|error| {
                                agent_client_protocol::Error::internal_error()
                                    .data(error.to_string())
                            })?,
                        "handle": agent.clone(),
                        "arguments": [
                            {"kind": "handle", "value": session.session.clone()}
                        ],
                    }),
                )?)
                .block_task()
                .await?;
            assert!(decoded_facade_response(snapshots)
                .map_err(|error| agent_client_protocol::Error::internal_error().data(error.to_string()))?
                .is_array());
            let latest_snapshot_operation =
                "echo_agent::agent::react::ReactAgent::latest_snapshot";
            let latest_snapshot = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/facade/invoke",
                    serde_json::json!({
                        "operation": latest_snapshot_operation,
                        "signature_digest": catalog_invoke_digest(latest_snapshot_operation)
                            .map_err(|error| {
                                agent_client_protocol::Error::internal_error()
                                    .data(error.to_string())
                            })?,
                        "handle": agent.clone(),
                        "arguments": [
                            {"kind": "handle", "value": session.session.clone()}
                        ],
                    }),
                )?)
                .block_task()
                .await?;
            let latest_snapshot = decoded_facade_response(latest_snapshot).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            assert!(latest_snapshot.is_null() || latest_snapshot.is_object());
            let disconnect_mcp_operation =
                "echo_agent::agent::react::ReactAgent::disconnect_mcp";
            let disconnected = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/facade/invoke",
                    serde_json::json!({
                        "operation": disconnect_mcp_operation,
                        "signature_digest": catalog_invoke_digest(disconnect_mcp_operation)
                            .map_err(|error| {
                                agent_client_protocol::Error::internal_error()
                                    .data(error.to_string())
                            })?,
                        "handle": agent.clone(),
                        "arguments": [
                            {"kind": "handle", "value": session.session.clone()},
                            {"kind": "string", "value": "missing-mcp"}
                        ],
                    }),
                )?)
                .block_task()
                .await?;
            assert_eq!(decoded_facade_response(disconnected).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?, serde_json::Value::Bool(false));
            let cancelled_delegate_operation =
                "echo_agent::agent::react::ReactAgent::delegate_to_agent_with_cancel";
            let cancelled_delegate = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/facade/invoke",
                    serde_json::json!({
                        "operation": cancelled_delegate_operation,
                        "signature_digest": catalog_invoke_digest(cancelled_delegate_operation)
                            .map_err(|error| {
                                agent_client_protocol::Error::internal_error()
                                    .data(error.to_string())
                            })?,
                        "handle": agent.clone(),
                        "arguments": [
                            {"kind": "handle", "value": session.session.clone()},
                            {"kind": "string", "value": "target"},
                            {"kind": "string", "value": "task"}
                        ],
                    }),
                )?)
                .block_task()
                .await
                .expect_err("cancel-aware delegation requires an active run");
            let cancelled_delegate_error = typed_facade_error(&cancelled_delegate).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            assert_eq!(
                cancelled_delegate_error.code,
                ExtensionErrorCode::FrameworkError
            );
            let messages_operation = "echo_core::agent::Agent::messages";
            let messages = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/facade/invoke",
                    serde_json::json!({
                        "operation": messages_operation,
                        "signature_digest": catalog_invoke_digest(messages_operation)
                            .map_err(|error| {
                                agent_client_protocol::Error::internal_error()
                                    .data(error.to_string())
                            })?,
                        "handle": agent.clone(),
                        "arguments": [{"kind": "handle", "value": session.session.clone()}],
                    }),
                )?)
                .block_task()
                .await?;
            let messages = decoded_facade_response(messages).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            assert!(messages.is_array());
            let loaded_message = echo_sdk_protocol::scalar::WireValue::from_json(
                serde_json::json!({
                    "role": "user",
                    "content": {"kind": "string", "value": "loaded replacement"}
                }),
            )
            .map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            let loaded_messages = echo_sdk_protocol::scalar::WireValue::List(vec![loaded_message]);
            let load_messages_operation =
                "echo_agent::agent::react::ReactAgent::load_messages";
            let loaded = invoke_facade(
                &connection,
                load_messages_operation,
                &agent,
                vec![
                    session_argument(&session.session),
                    serde_json::to_value(loaded_messages).map_err(|error| {
                        agent_client_protocol::Error::internal_error().data(error.to_string())
                    })?,
                ],
            )
            .await?;
            assert_eq!(loaded, serde_json::Value::Null);
            let replaced_messages = invoke_facade(
                &connection,
                messages_operation,
                &agent,
                vec![session_argument(&session.session)],
            )
            .await?;
            assert_eq!(
                replaced_messages
                    .as_array()
                    .and_then(|messages| messages.first())
                    .and_then(|message| message.get("content"))
                    .and_then(serde_json::Value::as_str),
                Some("loaded replacement")
            );
            let tool_names_operation = "echo_core::agent::Agent::tool_names";
            let tool_names = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/facade/invoke",
                    serde_json::json!({
                        "operation": tool_names_operation,
                        "signature_digest": catalog_invoke_digest(tool_names_operation)
                            .map_err(|error| {
                                agent_client_protocol::Error::internal_error()
                                    .data(error.to_string())
                            })?,
                        "handle": agent.clone(),
                        "arguments": [{"kind": "handle", "value": session.session.clone()}],
                    }),
                )?)
                .block_task()
                .await?;
            let tool_names = decoded_facade_response(tool_names).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            assert!(tool_names.is_array());
            let set_prompt_operation = "echo_core::agent::Agent::set_system_prompt";
            let set_prompt = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/facade/invoke",
                    serde_json::json!({
                        "operation": set_prompt_operation,
                        "signature_digest": catalog_invoke_digest(set_prompt_operation)
                            .map_err(|error| {
                                agent_client_protocol::Error::internal_error()
                                    .data(error.to_string())
                            })?,
                        "handle": agent.clone(),
                        "arguments": [
                            {"kind": "handle", "value": session.session.clone()},
                            {"kind": "string", "value": "updated system prompt"}
                        ],
                    }),
                )?)
                .block_task()
                .await?;
            assert_eq!(decoded_facade_response(set_prompt).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?, serde_json::Value::Null);
            let live_messages_operation = "echo_core::agent::Agent::messages";
            let live_messages = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/facade/invoke",
                    serde_json::json!({
                        "operation": live_messages_operation,
                        "signature_digest": catalog_invoke_digest(live_messages_operation)
                            .map_err(|error| {
                                agent_client_protocol::Error::internal_error()
                                    .data(error.to_string())
                            })?,
                        "handle": agent.clone(),
                        "arguments": [{"kind": "handle", "value": session.session.clone()}],
                    }),
                )?)
                .block_task()
                .await?;
            let live_messages = decoded_facade_response(live_messages).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            assert!(live_messages.to_string().contains("updated system prompt"));
            let usage_operation = "echo_core::agent::Agent::token_usage_summary";
            let usage = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/facade/invoke",
                    serde_json::json!({
                        "operation": usage_operation,
                        "signature_digest": catalog_invoke_digest(usage_operation)
                            .map_err(|error| {
                                agent_client_protocol::Error::internal_error()
                                    .data(error.to_string())
                            })?,
                        "handle": agent.clone(),
                        "arguments": [{"kind": "handle", "value": session.session.clone()}],
                    }),
                )?)
                .block_task()
                .await?;
            let usage = decoded_facade_response(usage).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            assert_eq!(usage.get("model_name"), Some(&serde_json::json!("fixture-model")));
            let checkpoint_operation = "echo_agent::agent::react::ReactAgent::force_checkpoint";
            let checkpoint = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/facade/invoke",
                    serde_json::json!({
                        "operation": checkpoint_operation,
                        "signature_digest": catalog_invoke_digest(checkpoint_operation)
                            .map_err(|error| {
                                agent_client_protocol::Error::internal_error()
                                    .data(error.to_string())
                            })?,
                        "handle": agent.clone(),
                        "arguments": [{"kind": "handle", "value": session.session.clone()}],
                    }),
                )?)
                .block_task()
                .await?;
            assert_eq!(decoded_facade_response(checkpoint).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?, serde_json::Value::Null);
            let compress_operation =
                "echo_agent::agent::react::ReactAgent::force_compress_context";
            let compressed = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/facade/invoke",
                    serde_json::json!({
                        "operation": compress_operation,
                        "signature_digest": catalog_invoke_digest(compress_operation)
                            .map_err(|error| {
                                agent_client_protocol::Error::internal_error()
                                    .data(error.to_string())
                            })?,
                        "handle": agent.clone(),
                        "arguments": [{"kind": "handle", "value": session.session.clone()}],
                    }),
                )?)
                .block_task()
                .await?;
            let compressed = decoded_facade_response(compressed).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            assert!(compressed.get("stats").is_some());
            let reset_operation = "echo_core::agent::Agent::reset";
            let reset = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/facade/invoke",
                    serde_json::json!({
                        "operation": reset_operation,
                        "signature_digest": catalog_invoke_digest(reset_operation)
                            .map_err(|error| {
                                agent_client_protocol::Error::internal_error()
                                    .data(error.to_string())
                            })?,
                        "handle": agent.clone(),
                        "arguments": [{"kind": "handle", "value": session.session.clone()}],
                    }),
                )?)
                .block_task()
                .await?;
            assert_eq!(decoded_facade_response(reset).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?, serde_json::Value::Null);
            let chat_operation = "echo_core::agent::Agent::chat";
            let chat = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/facade/invoke",
                    serde_json::json!({
                        "operation": chat_operation,
                        "signature_digest": catalog_invoke_digest(chat_operation)
                            .map_err(|error| {
                                agent_client_protocol::Error::internal_error()
                                    .data(error.to_string())
                            })?,
                        "handle": agent.clone(),
                        "arguments": [
                            {"kind": "handle", "value": session.session.clone()},
                            {"kind": "string", "value": "source chat"}
                        ],
                    }),
                )?)
                .block_task()
                .await?;
            let chat = decoded_facade_response(chat).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            let chat: echo_sdk_protocol::methods::RunStartResponse =
                serde_json::from_value(chat).map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })?;
            assert_eq!(chat.run.kind, HandleKind::Run);
            assert_eq!(chat.stream.kind, HandleKind::Stream);
            let extra_chat_arg = assert_facade_invalid(
                &connection,
                chat_operation,
                &agent,
                vec![
                    session_argument(&session.session),
                    serde_json::json!({"kind": "string", "value": "source chat"}),
                    serde_json::json!({"kind": "string", "value": "unexpected"}),
                ],
            )
            .await;
            extra_chat_arg.map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            let steer_operation = "echo_core::agent::Agent::steer_input";
            let steer = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/facade/invoke",
                    serde_json::json!({
                        "operation": steer_operation,
                        "signature_digest": catalog_invoke_digest(steer_operation)
                            .map_err(|error| {
                                agent_client_protocol::Error::internal_error()
                                    .data(error.to_string())
                            })?,
                        "handle": agent.clone(),
                        "arguments": [
                            {"kind": "handle", "value": chat.run.clone()},
                            {"kind": "string", "value": "source steer"}
                        ],
                    }),
                )?)
                .block_task()
                .await?;
            let steer = decoded_facade_response(steer).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            let steer: echo_sdk_protocol::methods::RunSteerResponse =
                serde_json::from_value(steer).map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })?;
            assert!(steer.accepted || steer.steer_id.is_none());
            let extra_steer_arg = assert_facade_invalid(
                &connection,
                steer_operation,
                &agent,
                vec![
                    serde_json::json!({"kind": "handle", "value": chat.run.clone()}),
                    serde_json::json!({"kind": "string", "value": "source steer"}),
                    serde_json::json!({"kind": "string", "value": "unexpected"}),
                ],
            )
            .await;
            extra_steer_arg.map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            let chat_run = chat.run;

            // ReactAgent exposes the same live steering safe point through its
            // concrete method identity. The Host routes it to the typed Run
            // authority, so the result retains the normal accepted/steer_id
            // lifecycle rather than simulating a local mailbox.
            let react_steer_operation =
                "echo_agent::agent::react::ReactAgent::steer_input";
            let react_steer = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/facade/invoke",
                    serde_json::json!({
                        "operation": react_steer_operation,
                        "signature_digest": catalog_invoke_digest(react_steer_operation)
                            .map_err(|error| {
                                agent_client_protocol::Error::internal_error()
                                    .data(error.to_string())
                            })?,
                        "handle": agent.clone(),
                        "arguments": [
                            {"kind": "handle", "value": chat_run.clone()},
                            {"kind": "string", "value": "react source steer"}
                        ],
                    }),
                )?)
                .block_task()
                .await?;
            let react_steer = decoded_facade_response(react_steer).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            let react_steer: echo_sdk_protocol::methods::RunSteerResponse =
                serde_json::from_value(react_steer).map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })?;
            assert!(react_steer.accepted || react_steer.steer_id.is_none());

            // Both concrete ReactAgent steering identities share the same
            // strict wire shape. A missing Run handle is rejected before any
            // framework authority is touched and remains a typed invalid
            // value rather than a generic transport failure.
            let tracked_operation =
                "echo_agent::agent::react::ReactAgent::steer_input_tracked";
            let tracked_error = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/facade/invoke",
                    serde_json::json!({
                        "operation": tracked_operation,
                        "signature_digest": catalog_invoke_digest(tracked_operation)
                            .map_err(|error| {
                                agent_client_protocol::Error::internal_error()
                                    .data(error.to_string())
                            })?,
                        "handle": agent.clone(),
                        "arguments": [],
                    }),
                )?)
                .block_task()
                .await
                .expect_err("ReactAgent steering requires a Run handle");
            let typed = typed_facade_error(&tracked_error).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            assert_eq!(typed.code, ExtensionErrorCode::InvalidValue);

            let _ = connection
                .send_request(RunCancelRequest {
                    run: chat_run.clone(),
                })
                .block_task()
                .await?;
            let _ = connection
                .send_request(RunWaitRequest {
                    run: chat_run,
                    timeout: None,
                })
                .block_task()
                .await?;

            let execute_operation = "echo_core::agent::Agent::execute";
            let execute = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/facade/invoke",
                    serde_json::json!({
                        "operation": execute_operation,
                        "signature_digest": catalog_invoke_digest(execute_operation)
                            .map_err(|error| {
                                agent_client_protocol::Error::internal_error()
                                    .data(error.to_string())
                            })?,
                        "handle": agent.clone(),
                        "arguments": [
                            {"kind": "handle", "value": session.session.clone()},
                            {"kind": "string", "value": "source execute"}
                        ],
                    }),
                )?)
                .block_task()
                .await?;
            let execute = decoded_facade_response(execute).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            let execute: echo_sdk_protocol::methods::RunStartResponse =
                serde_json::from_value(execute).map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })?;
            let execute_run = execute.run;
            let _ = connection
                .send_request(RunCancelRequest {
                    run: execute_run.clone(),
                })
                .block_task()
                .await?;
            let _ = connection
                .send_request(RunWaitRequest {
                    run: execute_run,
                    timeout: None,
                })
                .block_task()
                .await?;

            let stream_operation = "echo_core::agent::Agent::chat_stream";
            let streamed = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/facade/invoke",
                    serde_json::json!({
                        "operation": stream_operation,
                        "signature_digest": catalog_invoke_digest(stream_operation)
                            .map_err(|error| {
                                agent_client_protocol::Error::internal_error()
                                    .data(error.to_string())
                            })?,
                        "handle": agent.clone(),
                        "arguments": [
                            {"kind": "handle", "value": session.session.clone()},
                            {"kind": "string", "value": "source stream"}
                        ],
                    }),
                )?)
                .block_task()
                .await?;
            let streamed = decoded_facade_response(streamed).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            let streamed: echo_sdk_protocol::methods::RunStartResponse =
                serde_json::from_value(streamed).map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })?;
            let streamed_run = streamed.run;
            let _ = connection
                .send_request(RunCancelRequest {
                    run: streamed_run.clone(),
                })
                .block_task()
                .await?;
            let _ = connection
                .send_request(RunWaitRequest {
                    run: streamed_run,
                    timeout: None,
                })
                .block_task()
                .await?;

            let message_stream_operation =
                "echo_agent::agent::react::ReactAgent::chat_stream_message";
            let message_value = echo_sdk_protocol::scalar::WireValue::from_json(
                serde_json::json!({
                    "role": "user",
                    "content": {"kind": "string", "value": "structured source message"}
                }),
            )
            .map_err(|error| agent_client_protocol::Error::internal_error().data(error.to_string()))?;
            let message_stream = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/facade/invoke",
                    serde_json::json!({
                        "operation": message_stream_operation,
                        "signature_digest": catalog_invoke_digest(message_stream_operation)
                            .map_err(|error| {
                                agent_client_protocol::Error::internal_error()
                                    .data(error.to_string())
                            })?,
                        "handle": agent.clone(),
                        "arguments": [
                            {"kind": "handle", "value": session.session.clone()},
                            serde_json::to_value(message_value.clone()).map_err(|error| {
                                agent_client_protocol::Error::internal_error()
                                    .data(error.to_string())
                            })?
                        ],
                    }),
                )?)
                .block_task()
                .await?;
            let message_stream = decoded_facade_response(message_stream).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            let message_stream: echo_sdk_protocol::methods::RunStartResponse =
                serde_json::from_value(message_stream).map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })?;
            let message_run = message_stream.run;
            let _ = connection
                .send_request(RunCancelRequest {
                    run: message_run.clone(),
                })
                .block_task()
                .await?;
            let _ = connection
                .send_request(RunWaitRequest {
                    run: message_run,
                    timeout: None,
                })
                .block_task()
                .await?;

            let execute_message_operation =
                "echo_agent::agent::react::ReactAgent::execute_stream_message";
            let execute_message = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/facade/invoke",
                    serde_json::json!({
                        "operation": execute_message_operation,
                        "signature_digest": catalog_invoke_digest(execute_message_operation)
                            .map_err(|error| {
                                agent_client_protocol::Error::internal_error()
                                    .data(error.to_string())
                            })?,
                        "handle": agent.clone(),
                        "arguments": [
                            {"kind": "handle", "value": session.session.clone()},
                            serde_json::to_value(message_value).map_err(|error| {
                                agent_client_protocol::Error::internal_error()
                                    .data(error.to_string())
                            })?
                        ],
                    }),
                )?)
                .block_task()
                .await?;
            let execute_message = decoded_facade_response(execute_message).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            let execute_message: echo_sdk_protocol::methods::RunStartResponse =
                serde_json::from_value(execute_message).map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })?;
            let _ = connection
                .send_request(RunCancelRequest {
                    run: execute_message.run.clone(),
                })
                .block_task()
                .await?;
            let _ = connection
                .send_request(RunWaitRequest {
                    run: execute_message.run,
                    timeout: None,
                })
                .block_task()
                .await?;

            let digest =
                |family: &str, operation: &str| family_op_digest(family, operation);
            // Unknown operation identities fail closed as invalid_value with
            // the typed facade detail carrying the rejected identity.
            let unknown = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/facade/invoke",
                    serde_json::json!({
                        "operation": "totally::unknown::operation",
                        "signature_digest": known_digest,
                        "arguments": [],
                    }),
                )?)
                .block_task()
                .await
                .expect_err("unknown operation must fail");
            let typed = typed_facade_error(&unknown).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            assert_eq!(typed.code, ExtensionErrorCode::InvalidValue);
            let detail = typed
                .details
                .and_then(|details| details.facade)
                .ok_or_else(|| {
                    agent_client_protocol::Error::internal_error().data("no facade detail")
                })?;
            assert_eq!(
                detail.operation.as_deref(),
                Some("totally::unknown::operation")
            );

            // A canonical source operation resolves through the embedded
            // catalog and reaches its concrete adapter's typed input checks;
            // Plan 08 no longer permits an unbound generic source fallback.
            let known = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/facade/invoke",
                    serde_json::json!({
                        "operation": known_operation.clone(),
                        "signature_digest": known_digest,
                        "arguments": [],
                    }),
                )?)
                .block_task()
                .await
                .expect_err("source operation with invalid arguments must fail explicitly");
            let typed = typed_facade_error(&known).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            assert_eq!(typed.code, ExtensionErrorCode::InvalidValue);
            assert_ne!(
                typed.message,
                "source operation is canonical but has no Host authority adapter"
            );

            let wrong_digest = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/facade/invoke",
                    serde_json::json!({
                        "operation": known_operation,
                        "signature_digest":
                            "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                        "arguments": [],
                    }),
                )?)
                .block_task()
                .await
                .expect_err("wrong canonical signature must fail closed");
            let typed = typed_facade_error(&wrong_digest).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            assert_eq!(typed.code, ExtensionErrorCode::InvalidValue);

            // The structured-output contract validation ships with the
            // facade runtime: a broken schema is a typed invalid value and
            // a valid schema with a conforming sample validates.
            let arguments =
                |schema: serde_json::Value,
                 instance: Option<serde_json::Value>|
                 -> Result<serde_json::Value, agent_client_protocol::Error> {
                    let mut arguments = vec![
                        echo_sdk_protocol::scalar::WireValue::from_json(schema).map_err(
                            |error| {
                                agent_client_protocol::Error::invalid_params()
                                    .data(error.to_string())
                            },
                        )?,
                    ];
                    if let Some(instance) = instance {
                        arguments.push(
                            echo_sdk_protocol::scalar::WireValue::from_json(instance).map_err(
                                |error| {
                                    agent_client_protocol::Error::invalid_params()
                                        .data(error.to_string())
                                },
                            )?,
                        );
                    }
                    let request = echo_sdk_protocol::methods::FeatureOperationRequest {
                        operation: "structured_output.validate".to_string(),
                        signature_digest:
                            "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                                .to_string(),
                        handle: None,
                        arguments,
                    };
                    serde_json::to_value(&request).map_err(|error| {
                        agent_client_protocol::Error::internal_error().data(error.to_string())
                    })
                };
            let valid = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/structured_output/validate",
                    arguments(
                        serde_json::json!({"type": "object"}),
                        Some(serde_json::json!({})),
                    )?,
                )?)
                .block_task()
                .await?;
            let decoded: echo_sdk_protocol::methods::FeatureOperationResponse =
                serde_json::from_value(valid).map_err(|error| {
                    agent_client_protocol::Error::invalid_params().data(error.to_string())
                })?;
            let decoded = decoded.value.into_json().map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            assert_eq!(decoded.get("valid"), Some(&serde_json::Value::Bool(true)));
            let broken = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/structured_output/validate",
                    arguments(serde_json::json!("not-a-schema"), None)?,
                )?)
                .block_task()
                .await
                .expect_err("a non-object schema must fail");
            let typed = typed_facade_error(&broken).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            assert_eq!(typed.code, ExtensionErrorCode::InvalidValue);

            // The memory family routes the closed store-operation set onto
            // the session's own store authority (todo 4).
            let memory_request =
                |operation: &str,
                 handle: echo_sdk_protocol::handle::WireHandle,
                 arguments: Vec<serde_json::Value>|
                 -> Result<serde_json::Value, agent_client_protocol::Error> {
                    let arguments = arguments
                        .into_iter()
                        .map(|value| {
                            echo_sdk_protocol::scalar::WireValue::from_json(value).map_err(
                                |error| {
                                    agent_client_protocol::Error::invalid_params()
                                        .data(error.to_string())
                                },
                            )
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    let request = echo_sdk_protocol::methods::FeatureOperationRequest {
                        operation: operation.to_string(),
                        signature_digest: digest("memory", operation),
                        handle: Some(handle),
                        arguments,
                    };
                    serde_json::to_value(&request).map_err(|error| {
                        agent_client_protocol::Error::internal_error().data(error.to_string())
                    })
                };
            // Create a session whose store backs the family operations.
            let agent = connection
                .send_request(AgentCreateRequest {
                    config: AgentConfigWire::HostDefault,
                    idempotency_id: None,
                })
                .block_task()
                .await?
                .agent;
            let memory_session = connection
                .send_request(SessionCreateRequest {
                    agent: agent.clone(),
                    working_dir: None,
                    session_id: None,
                    idempotency_id: None,
                })
                .block_task()
                .await?;
            let put = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/memory/op",
                    memory_request(
                        "memory.store.put",
                        memory_session.session.clone(),
                        vec![
                            serde_json::json!(["memories"]),
                            serde_json::json!("m-1"),
                            serde_json::json!({"text": "hello memory"}),
                        ],
                    )?,
                )?)
                .block_task()
                .await?;
            let put = decoded_facade_response(put).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            assert_eq!(put.get("ok"), Some(&serde_json::Value::Bool(true)));
            let got = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/memory/op",
                    memory_request(
                        "memory.store.get",
                        memory_session.session.clone(),
                        vec![serde_json::json!(["memories"]), serde_json::json!("m-1")],
                    )?,
                )?)
                .block_task()
                .await?;
            let got = decoded_facade_response(got).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            assert_eq!(
                got.get("value").and_then(|value| value.get("text")),
                Some(&serde_json::json!("hello memory"))
            );
            let deleted = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/memory/op",
                    memory_request(
                        "memory.store.delete",
                        memory_session.session.clone(),
                        vec![serde_json::json!(["memories"]), serde_json::json!("m-1")],
                    )?,
                )?)
                .block_task()
                .await?;
            let deleted = decoded_facade_response(deleted).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            assert_eq!(deleted.get("deleted"), Some(&serde_json::Value::Bool(true)));
            let unknown_op = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/memory/op",
                    memory_request(
                        "memory.store.vacuum",
                        memory_session.session.clone(),
                        vec![serde_json::json!(["memories"])],
                    )?,
                )?)
                .block_task()
                .await
                .expect_err("closed family surface rejects unknown operations");
            let typed = typed_facade_error(&unknown_op).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            assert_eq!(typed.code, ExtensionErrorCode::InvalidValue);

            #[cfg(all(feature = "framework-channels", feature = "sdk-extension-bridge"))]
            {
                // The full facade build binds channels through the reverse
                // extension bridge, so the family is executable rather than
                // method-not-found.
                let family = connection
                    .send_request(UntypedMessage::new(
                        "_echo_agent/channels/op",
                        serde_json::json!({
                            "operation": "channels.manager.open",
                            "signature_digest": family_op_digest(
                                "channels",
                                "channels.manager.open"
                            ),
                            "handle": memory_session.session,
                            "arguments": [],
                        }),
                    )?)
                    .block_task()
                    .await?;
                let family = decoded_facade_response(family).map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })?;
                assert!(family.get("resource").is_some());
            }
            #[cfg(not(all(feature = "framework-channels", feature = "sdk-extension-bridge")))]
            {
                // A build without the typed channel adapter must fail closed
                // through the official method-not-found path.
                let family = connection
                    .send_request(UntypedMessage::new(
                        "_echo_agent/channels/op",
                        serde_json::json!({
                            "operation": "channels.manager.open",
                            "signature_digest":
                                "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                            "arguments": [],
                        }),
                    )?)
                    .block_task()
                    .await
                    .expect_err("unbound family surface must fail closed");
                assert_eq!(family.code, agent_client_protocol::ErrorCode::MethodNotFound);
            }
            let telemetry = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/telemetry/op",
                    serde_json::json!({
                        "operation": "telemetry.status",
                        "signature_digest": family_op_digest("telemetry", "telemetry.status"),
                        "arguments": [],
                    }),
                )?)
                .block_task()
                .await?;
            let telemetry = decoded_facade_response(telemetry).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            assert_eq!(
                telemetry.get("initialized"),
                Some(&serde_json::Value::Bool(false))
            );
            let close_operation = "echo_core::agent::Agent::close";
            let closed = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/facade/invoke",
                    serde_json::json!({
                        "operation": close_operation,
                        "signature_digest": catalog_invoke_digest(close_operation)
                            .map_err(|error| {
                                agent_client_protocol::Error::internal_error()
                                    .data(error.to_string())
                            })?,
                        "handle": agent,
                        "arguments": [],
                    }),
                )?)
                .block_task()
                .await?;
            let closed = decoded_facade_response(closed).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            assert_eq!(closed.get("released"), Some(&serde_json::Value::Bool(true)));
            Ok(())
        })
    })
    .await?;
    let _ = host.child.kill().await;
    Ok(())
}

#[cfg(feature = "sdk-facade-adapters")]
#[tokio::test]
async fn every_canonical_source_operation_reaches_a_host_adapter()
-> Result<(), Box<dyn std::error::Error>> {
    let (endpoint, _request_seen) = start_model_server("unused").await?;
    let work = tempfile::tempdir()?;
    let state_root = tempfile::tempdir()?;
    let config = write_config(work.path(), &endpoint, state_root.path(), None)?;
    let mut host = spawn_host(&config).await?;
    let (events, updates, gaps) = empty_collectors::<EventNotification>();
    let operations = catalog_source_operations()?;

    drive(&mut host, events, updates, gaps, move |connection| {
        let operations = operations.clone();
        Box::pin(async move {
            connection
                .send_request(initialize_request(Some(client_hello())))
                .block_task()
                .await?;
            let mut missing = Vec::new();
            for (operation, signature_digest) in operations {
                let response = connection
                    .send_request(UntypedMessage::new(
                        "_echo_agent/facade/invoke",
                        serde_json::json!({
                            "operation": operation,
                            "signature_digest": signature_digest,
                            "arguments": [{"kind": "null"}],
                        }),
                    )?)
                    .block_task()
                    .await;
                if let Err(error) = response {
                    let typed = typed_facade_error(&error).map_err(|decode_error| {
                        agent_client_protocol::Error::internal_error()
                            .data(decode_error.to_string())
                    })?;
                    if typed.message
                        == "source operation is canonical but has no Host authority adapter"
                    {
                        missing.push(operation);
                    }
                }
            }
            assert!(
                missing.is_empty(),
                "canonical source operations without Host adapters: {missing:#?}"
            );
            Ok(())
        })
    })
    .await?;
    let _ = host.child.kill().await;
    Ok(())
}

#[cfg(feature = "sdk-facade-adapters")]
#[tokio::test]
async fn stateful_source_resources_preserve_owner_state_and_close()
-> Result<(), Box<dyn std::error::Error>> {
    let (endpoint, _request_seen) = start_model_server("unused").await?;
    let work = tempfile::tempdir()?;
    let state_root = tempfile::tempdir()?;
    let plugin_root = work.path().join("plugins-state");
    let config = write_config(work.path(), &endpoint, state_root.path(), None)?;
    let mut host = spawn_host(&config).await?;
    let (events, updates, gaps) = empty_collectors::<EventNotification>();

    drive(&mut host, events, updates, gaps, move |connection| {
        let plugin_root = plugin_root.clone();
        Box::pin(async move {
            connection
                .send_request(initialize_request(Some(client_hello())))
                .block_task()
                .await?;
            let agent = connection
                .send_request(AgentCreateRequest {
                    config: AgentConfigWire::HostDefault,
                    idempotency_id: Some("stateful-resource-agent".to_string()),
                })
                .block_task()
                .await?
                .agent;
            let first = connection
                .send_request(SessionCreateRequest {
                    agent: agent.clone(),
                    working_dir: None,
                    session_id: None,
                    idempotency_id: Some("stateful-resource-first".to_string()),
                })
                .block_task()
                .await?;
            let second = connection
                .send_request(SessionCreateRequest {
                    agent: agent.clone(),
                    working_dir: None,
                    session_id: None,
                    idempotency_id: Some("stateful-resource-second".to_string()),
                })
                .block_task()
                .await?;

            let store = invoke_facade_wire(
                &connection,
                "echo_state::memory::store::InMemoryStore::new",
                &agent,
                vec![WireValue::Handle(first.session.clone())],
            )
            .await?;
            let WireValue::Handle(store) = store else {
                return Err(agent_client_protocol::Error::internal_error()
                    .data("InMemoryStore::new did not return a resource"));
            };
            let namespace = WireValue::List(vec![WireValue::String("sdk".to_string())]);
            invoke_family_wire(
                &connection,
                "_echo_agent/memory/op",
                "memory",
                "memory.resource.put",
                &first.session,
                vec![
                    WireValue::Handle(store.clone()),
                    namespace.clone(),
                    WireValue::String("answer".to_string()),
                    WireValue::String("42".to_string()),
                ],
            )
            .await?;
            let found = invoke_family_wire(
                &connection,
                "_echo_agent/memory/op",
                "memory",
                "memory.resource.get",
                &first.session,
                vec![
                    WireValue::Handle(store.clone()),
                    namespace.clone(),
                    WireValue::String("answer".to_string()),
                ],
            )
            .await?;
            let found = found.into_json().map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            assert_eq!(found.get("key"), Some(&serde_json::json!("answer")));
            assert_eq!(found.get("value"), Some(&serde_json::json!("42")));

            let foreign = invoke_family_wire(
                &connection,
                "_echo_agent/memory/op",
                "memory",
                "memory.resource.get",
                &second.session,
                vec![
                    WireValue::Handle(store.clone()),
                    namespace,
                    WireValue::String("answer".to_string()),
                ],
            )
            .await
            .expect_err("a foreign Session must not resolve a Store resource");
            assert_eq!(
                typed_facade_error(&foreign)
                    .map_err(|error| agent_client_protocol::Error::internal_error()
                        .data(error.to_string()))?
                    .code,
                ExtensionErrorCode::InvalidValue
            );

            let registry = invoke_facade_wire(
                &connection,
                "echo_core::plugin::registry::PluginRegistry::new",
                &agent,
                vec![
                    WireValue::Handle(first.session.clone()),
                    test_wire_path(&plugin_root).map_err(|error| {
                        agent_client_protocol::Error::internal_error().data(error.to_string())
                    })?,
                    WireValue::Null,
                ],
            )
            .await?;
            let WireValue::Handle(registry) = registry else {
                return Err(agent_client_protocol::Error::internal_error()
                    .data("PluginRegistry::new did not return a resource"));
            };
            let count = invoke_facade_wire(
                &connection,
                "echo_core::plugin::registry::PluginRegistry::count",
                &agent,
                vec![
                    WireValue::Handle(first.session.clone()),
                    WireValue::Handle(registry.clone()),
                ],
            )
            .await?;
            assert_eq!(count, WireValue::U64(WireU64::from_u64(0)));

            let no_secret = invoke_facade_wire(
                &connection,
                "echo_agent::security::contains_secrets",
                &agent,
                vec![
                    WireValue::Handle(first.session.clone()),
                    WireValue::String(String::new()),
                ],
            )
            .await?;
            assert_eq!(no_secret, WireValue::Bool(false));

            let prompt_context = invoke_facade_wire(
                &connection,
                "echo_execution::skills::external::prompt_exec::PromptContext",
                &agent,
                vec![
                    WireValue::Handle(first.session.clone()),
                    WireValue::String(String::new()),
                    WireValue::String(String::new()),
                    WireValue::List(Vec::new()),
                    WireValue::Null,
                    WireValue::Duration(WireDuration::from_nanos(1_000_000)),
                    WireValue::String("local".to_string()),
                    WireValue::Null,
                ],
            )
            .await?;
            let WireValue::Handle(prompt_context) = prompt_context else {
                return Err(agent_client_protocol::Error::internal_error()
                    .data("PromptContext did not return a resource"));
            };
            let rendered = invoke_facade_wire(
                &connection,
                "echo_execution::skills::external::prompt_exec::process_skill_content",
                &agent,
                vec![
                    WireValue::Handle(first.session.clone()),
                    WireValue::String(String::new()),
                    WireValue::Handle(prompt_context.clone()),
                ],
            )
            .await?;
            assert_eq!(rendered, WireValue::String(String::new()));

            for resource in [store.clone(), registry, prompt_context] {
                assert_eq!(
                    invoke_family_wire(
                        &connection,
                        "_echo_agent/facade/invoke",
                        "invoke",
                        "facade.resource.close",
                        &first.session,
                        vec![WireValue::Handle(resource)],
                    )
                    .await?,
                    WireValue::Bool(true)
                );
            }
            let closed = invoke_family_wire(
                &connection,
                "_echo_agent/memory/op",
                "memory",
                "memory.resource.get",
                &first.session,
                vec![
                    WireValue::Handle(store),
                    WireValue::List(vec![WireValue::String("sdk".to_string())]),
                    WireValue::String("answer".to_string()),
                ],
            )
            .await
            .expect_err("a closed Store resource must stay closed");
            assert_eq!(
                typed_facade_error(&closed)
                    .map_err(|error| agent_client_protocol::Error::internal_error()
                        .data(error.to_string()))?
                    .code,
                ExtensionErrorCode::ClosedHandle
            );
            Ok(())
        })
    })
    .await?;
    let _ = host.child.kill().await;
    Ok(())
}

#[cfg(feature = "sdk-facade-adapters")]
#[tokio::test]
async fn source_files_and_skill_registry_preserve_rust_authority()
-> Result<(), Box<dyn std::error::Error>> {
    let (endpoint, _request_seen) = start_model_server("unused").await?;
    let work = tempfile::tempdir()?;
    let state_root = tempfile::tempdir()?;
    let file_path = work.path().join("facade-file.txt");
    let lease_path = work.path().join("facade-authority.json");
    let skill_path = work.path().join("facade-methodology").join("SKILL.md");
    let plugin_root = work.path().join("plugin-root");
    let plugin_data = work.path().join("plugin-data");
    let project_dir = work.path().to_path_buf();
    let config = write_config(work.path(), &endpoint, state_root.path(), None)?;
    let mut host = spawn_host(&config).await?;
    let (events, updates, gaps) = empty_collectors::<EventNotification>();

    let outcome = drive(&mut host, events, updates, gaps, move |connection| {
        let file_path = file_path.clone();
        let lease_path = lease_path.clone();
        let skill_path = skill_path.clone();
        let plugin_root = plugin_root.clone();
        let plugin_data = plugin_data.clone();
        let project_dir = project_dir.clone();
        Box::pin(async move {
            connection
                .send_request(initialize_request(Some(client_hello())))
                .block_task()
                .await?;
            let agent = connection
                .send_request(AgentCreateRequest {
                    config: AgentConfigWire::HostDefault,
                    idempotency_id: Some("facade-authority-agent".to_string()),
                })
                .block_task()
                .await?
                .agent;
            let first = connection
                .send_request(SessionCreateRequest {
                    agent: agent.clone(),
                    working_dir: None,
                    session_id: None,
                    idempotency_id: Some("facade-authority-first".to_string()),
                })
                .block_task()
                .await?;
            let second = connection
                .send_request(SessionCreateRequest {
                    agent: agent.clone(),
                    working_dir: None,
                    session_id: None,
                    idempotency_id: Some("facade-authority-second".to_string()),
                })
                .block_task()
                .await?;

            let session = WireValue::Handle(first.session.clone());
            let contains_secret = invoke_facade_wire(
                &connection,
                "echo_agent::security::contains_secrets",
                &agent,
                vec![
                    session.clone(),
                    WireValue::String("OPENAI_API_KEY=sk-facade-secret-fixture".to_string()),
                ],
            )
            .await?;
            assert_eq!(contains_secret, WireValue::Bool(true));

            let risk = invoke_facade_wire(
                &connection,
                "echo_execution::risk::ToolRiskClassifier::classify",
                &agent,
                vec![session.clone(), WireValue::String("shell".to_string())],
            )
            .await?;
            assert!(matches!(
                risk,
                WireValue::Variant { variant, .. } if variant == "shell_exec"
            ));

            let sandbox_command = WireValue::from_json(
                serde_json::to_value(echo_agent::sandbox::SandboxCommand::shell("echo ok"))
                    .map_err(|error| {
                        agent_client_protocol::Error::internal_error().data(error.to_string())
                    })?,
            )
            .map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            let isolation = invoke_facade_wire(
                &connection,
                "echo_execution::sandbox::policy::SandboxPolicy::evaluate",
                &agent,
                vec![
                    session.clone(),
                    WireValue::from_json(serde_json::json!({
                        "default_level": "strict",
                        "auto_escalate": true,
                        "max_isolation_level": null,
                        "container_required_languages": ["python"],
                        "trusted_commands": ["echo"]
                    }))
                    .map_err(|error| {
                        agent_client_protocol::Error::internal_error().data(error.to_string())
                    })?,
                    sandbox_command,
                ],
            )
            .await?;
            assert_eq!(isolation, WireValue::String("container".to_string()));

            let valid_task = WireValue::from_json(serde_json::json!({
                "id": "validate",
                "title": "Validate facade",
                "description": "Exercise the Rust PlanValidator authority",
                "depends_on": [],
                "max_retries": 1,
                "extension": {}
            }))
            .map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            let validated = invoke_facade_wire(
                &connection,
                "echo_orchestration::planning::validator::PlanValidator::validate_task_specs",
                &agent,
                vec![
                    session.clone(),
                    WireValue::U64(WireU64::from_u64(8)),
                    WireValue::U64(WireU64::from_u64(4)),
                    WireValue::U64(WireU64::from_u64(2)),
                    WireValue::List(vec![valid_task]),
                ],
            )
            .await?;
            let validated = validated.into_json().map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            assert_eq!(validated.get("valid"), Some(&serde_json::json!(true)));

            invoke_facade_wire(
                &connection,
                "echo_core::utils::fs::atomic_write",
                &agent,
                vec![
                    session.clone(),
                    test_wire_path(&file_path).map_err(|error| {
                        agent_client_protocol::Error::internal_error().data(error.to_string())
                    })?,
                    test_wire_bytes(b"alpha\nbeta\n"),
                ],
            )
            .await?;
            let read = invoke_facade_wire(
                &connection,
                "echo_core::utils::fs::read_existing",
                &agent,
                vec![
                    session.clone(),
                    test_wire_path(&file_path).map_err(|error| {
                        agent_client_protocol::Error::internal_error().data(error.to_string())
                    })?,
                ],
            )
            .await?;
            assert_eq!(
                decode_wire_bytes(read).map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })?,
                b"alpha\nbeta\n"
            );
            let guard = invoke_facade_wire(
                &connection,
                "echo_core::utils::fs::open_existing_regular_guard",
                &agent,
                vec![
                    session.clone(),
                    test_wire_path(&file_path).map_err(|error| {
                        agent_client_protocol::Error::internal_error().data(error.to_string())
                    })?,
                ],
            )
            .await?;
            let WireValue::Handle(guard) = guard else {
                return Err(agent_client_protocol::Error::internal_error()
                    .data("regular guard did not return a resource handle"));
            };
            assert_eq!(guard.kind, HandleKind::FacadeResource);
            let len = invoke_facade_wire(
                &connection,
                "echo_core::utils::fs::ExistingRegularFileGuard::len",
                &agent,
                vec![session.clone(), WireValue::Handle(guard.clone())],
            )
            .await?;
            assert!(matches!(len, WireValue::U64(value) if value.to_u64() == Some(11)));

            let foreign = invoke_facade_wire(
                &connection,
                "echo_core::utils::fs::ExistingRegularFileGuard::len",
                &agent,
                vec![
                    WireValue::Handle(second.session.clone()),
                    WireValue::Handle(guard.clone()),
                ],
            )
            .await
            .expect_err("a file guard cannot cross Session ownership");
            let typed = typed_facade_error(&foreign).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            assert_eq!(typed.code, ExtensionErrorCode::InvalidValue);

            invoke_facade_wire(
                &connection,
                "echo_core::utils::fs::append_existing_matching",
                &agent,
                vec![
                    session.clone(),
                    test_wire_path(&file_path).map_err(|error| {
                        agent_client_protocol::Error::internal_error().data(error.to_string())
                    })?,
                    WireValue::Handle(guard.clone()),
                    WireValue::U64(WireU64::from_u64(11)),
                    test_wire_bytes(b"gamma\n"),
                    WireValue::String("sync_data".to_string()),
                ],
            )
            .await?;
            assert_eq!(std::fs::read(&file_path).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?, b"alpha\nbeta\ngamma\n");

            let closed_guard = invoke_facade_wire(
                &connection,
                "facade.resource.close",
                &first.session,
                vec![WireValue::Handle(guard.clone())],
            )
            .await?;
            assert_eq!(closed_guard, WireValue::Bool(true));
            let closed_guard_access = invoke_facade_wire(
                &connection,
                "echo_core::utils::fs::ExistingRegularFileGuard::len",
                &agent,
                vec![session.clone(), WireValue::Handle(guard)],
            )
            .await
            .expect_err("a closed file guard must not remain resolvable");
            assert_eq!(
                typed_facade_error(&closed_guard_access)
                    .map_err(|error| agent_client_protocol::Error::internal_error()
                        .data(error.to_string()))?
                    .code,
                ExtensionErrorCode::ClosedHandle
            );

            let lease = invoke_facade_wire(
                &connection,
                "echo_core::utils::fs::try_exclusive_file_lease",
                &agent,
                vec![
                    session.clone(),
                    test_wire_path(&lease_path).map_err(|error| {
                        agent_client_protocol::Error::internal_error().data(error.to_string())
                    })?,
                ],
            )
            .await?;
            let WireValue::Handle(lease) = lease else {
                return Err(agent_client_protocol::Error::internal_error()
                    .data("file lease did not return a resource handle"));
            };
            assert_eq!(lease.kind, HandleKind::FacadeResource);
            let duplicate = invoke_facade_wire(
                &connection,
                "echo_core::utils::fs::try_exclusive_file_lease",
                &agent,
                vec![
                    WireValue::Handle(second.session.clone()),
                    test_wire_path(&lease_path).map_err(|error| {
                        agent_client_protocol::Error::internal_error().data(error.to_string())
                    })?,
                ],
            )
            .await
            .expect_err("the Rust lease authority must reject a duplicate holder");
            let typed = typed_facade_error(&duplicate).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            assert_eq!(typed.code, ExtensionErrorCode::FrameworkError);

            let foreign_close = invoke_facade_wire(
                &connection,
                "facade.resource.close",
                &second.session,
                vec![WireValue::Handle(lease.clone())],
            )
            .await
            .expect_err("a foreign Session must not close a file lease");
            assert_eq!(
                typed_facade_error(&foreign_close)
                    .map_err(|error| agent_client_protocol::Error::internal_error()
                        .data(error.to_string()))?
                    .code,
                ExtensionErrorCode::InvalidValue
            );
            let closed_lease = invoke_facade_wire(
                &connection,
                "facade.resource.close",
                &first.session,
                vec![WireValue::Handle(lease.clone())],
            )
            .await?;
            assert_eq!(closed_lease, WireValue::Bool(true));
            let repeated_close = invoke_facade_wire(
                &connection,
                "facade.resource.close",
                &first.session,
                vec![WireValue::Handle(lease)],
            )
            .await?;
            assert_eq!(repeated_close, WireValue::Bool(false));
            let reacquired = invoke_facade_wire(
                &connection,
                "echo_core::utils::fs::try_exclusive_file_lease",
                &agent,
                vec![
                    WireValue::Handle(second.session.clone()),
                    test_wire_path(&lease_path).map_err(|error| {
                        agent_client_protocol::Error::internal_error().data(error.to_string())
                    })?,
                ],
            )
            .await?;
            assert!(matches!(reacquired, WireValue::Handle(handle) if handle.kind == HandleKind::FacadeResource));

            let markdown = concat!(
                "---\n",
                "name: facade-methodology\n",
                "description: Facade methodology fixture.\n",
                "metadata:\n  category: methodology\n",
                "---\n\n",
                "Plugin root is ${ECHO_PLUGIN_ROOT}.\n"
            );
            invoke_facade_wire(
                &connection,
                "echo_execution::skills::registry::SkillRegistry::register_prepared",
                &agent,
                vec![
                    session.clone(),
                    WireValue::String(markdown.to_string()),
                    test_wire_path(&skill_path).map_err(|error| {
                        agent_client_protocol::Error::internal_error().data(error.to_string())
                    })?,
                    WireValue::Null,
                ],
            )
            .await?;
            let variables = WireValue::Record {
                type_id: "echo_core::plugin::PluginVariables".to_string(),
                fields: vec![
                    WireField {
                        name: "plugin_root".to_string(),
                        value: test_wire_path(&plugin_root).map_err(|error| {
                            agent_client_protocol::Error::internal_error().data(error.to_string())
                        })?,
                    },
                    WireField {
                        name: "plugin_data".to_string(),
                        value: test_wire_path(&plugin_data).map_err(|error| {
                            agent_client_protocol::Error::internal_error().data(error.to_string())
                        })?,
                    },
                    WireField {
                        name: "project_dir".to_string(),
                        value: test_wire_path(&project_dir).map_err(|error| {
                            agent_client_protocol::Error::internal_error().data(error.to_string())
                        })?,
                    },
                    WireField {
                        name: "user_config".to_string(),
                        value: WireValue::Map(vec![WireMapEntry {
                            key: WireValue::String("mode".to_string()),
                            value: WireValue::String("test".to_string()),
                        }]),
                    },
                ],
            };
            invoke_facade_wire(
                &connection,
                "echo_execution::skills::registry::SkillRegistry::tag_source_with_variables",
                &agent,
                vec![
                    session.clone(),
                    WireValue::List(vec![WireValue::String(
                        "facade-methodology".to_string(),
                    )]),
                    WireValue::String("plugin:facade".to_string()),
                    variables,
                ],
            )
            .await?;
            let content = invoke_facade_wire(
                &connection,
                "echo_execution::skills::registry::SkillRegistry::activate",
                &agent,
                vec![
                    session.clone(),
                    WireValue::String("facade-methodology".to_string()),
                ],
            )
            .await?
            .into_json()
            .map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            assert!(
                content
                    .get("instructions")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|text| text.contains(&plugin_root.display().to_string()))
            );
            let reset = invoke_facade_wire(
                &connection,
                "echo_execution::skills::registry::SkillRegistry::reset_activation_state",
                &agent,
                vec![session.clone()],
            )
            .await?;
            assert_eq!(reset, WireValue::Null);
            let activated = invoke_facade_wire(
                &connection,
                "echo_execution::skills::registry::SkillRegistry::mark_activated",
                &agent,
                vec![
                    session.clone(),
                    WireValue::String("facade-methodology".to_string()),
                ],
            )
            .await?;
            assert_eq!(activated, WireValue::Bool(true));
            invoke_facade_wire(
                &connection,
                "echo_execution::skills::registry::SkillRegistry::record_code_skill",
                &agent,
                vec![
                    session.clone(),
                    WireValue::String("facade-code".to_string()),
                    WireValue::String("Facade code skill".to_string()),
                    WireValue::List(vec![WireValue::String("facade_tool".to_string())]),
                    WireValue::Bool(true),
                ],
            )
            .await?;

            Ok(())
        })
    })
    .await;
    if outcome.is_err() {
        eprintln!("source authority Host stderr: {}", stderr_text(&host));
    }
    outcome?;
    let _ = host.child.kill().await;
    Ok(())
}

#[cfg(feature = "sdk-facade-adapters")]
#[tokio::test]
async fn turn_receipt_source_operations_project_live_and_recovered()
-> Result<(), Box<dyn std::error::Error>> {
    let (endpoint, request_seen) = start_model_server("receipt-answer").await?;
    let work = tempfile::tempdir()?;
    let state_root = tempfile::tempdir()?;
    let config = write_config(work.path(), &endpoint, state_root.path(), None)?;

    let mut host1 = spawn_host(&config).await?;
    let (events, updates, gaps) = empty_collectors::<EventNotification>();
    let session_id = drive(&mut host1, events, updates, gaps, move |connection| {
        Box::pin(async move {
            connection
                .send_request(initialize_request(Some(client_hello())))
                .block_task()
                .await?;
            let agent = connection
                .send_request(AgentCreateRequest {
                    config: AgentConfigWire::HostDefault,
                    idempotency_id: Some("turn-receipt-agent".to_string()),
                })
                .block_task()
                .await?
                .agent;
            let session = connection
                .send_request(SessionCreateRequest {
                    agent: agent.clone(),
                    working_dir: None,
                    session_id: Some("turn-receipt-session".to_string()),
                    idempotency_id: None,
                })
                .block_task()
                .await?;
            let classify_operation =
                "echo_orchestration::runtime::turn_driver::TurnOutcome::classify";
            let final_outcome = invoke_facade_without_handle_wire(
                &connection,
                classify_operation,
                vec![serde_json::json!({
                    "kind": "variant",
                    "value": {
                        "type_id": "echo_sdk_protocol::methods::AgentEventWire",
                        "variant": "final_answer",
                        "fields": [{
                            "name": "text",
                            "value": {"kind": "string", "value": "done"}
                        }]
                    }
                })],
            )
            .await?;
            assert!(matches!(
                final_outcome,
                echo_sdk_protocol::scalar::WireValue::Variant {
                    ref variant, ref fields, ..
                } if variant == "completed" && fields.is_empty()
            ));
            let pending_outcome = invoke_facade_without_handle_wire(
                &connection,
                classify_operation,
                vec![serde_json::json!({
                    "kind": "variant",
                    "value": {
                        "type_id": "echo_sdk_protocol::methods::AgentEventWire",
                        "variant": "token",
                        "fields": [{
                            "name": "text",
                            "value": {"kind": "string", "value": "partial"}
                        }]
                    }
                })],
            )
            .await?;
            assert_eq!(pending_outcome, echo_sdk_protocol::scalar::WireValue::Null);
            let started = connection
                .send_request(RunStartRequest {
                    session: session.session.clone(),
                    input: RunInput::Chat {
                        text: "receipt projection".to_string(),
                    },
                    idempotency_id: None,
                })
                .block_task()
                .await?;
            wait_for_model_request(&request_seen)
                .await
                .map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })?;
            let wait = connection
                .send_request(RunWaitRequest {
                    run: started.run.clone(),
                    timeout: None,
                })
                .block_task()
                .await?;
            assert!(wait.settled);

            let outcome_status_operation =
                "echo_orchestration::runtime::turn_driver::TurnOutcome::status";
            let status_operation = "echo_orchestration::runtime::turn_driver::TurnReceipt::status";
            let usage_operation = "echo_orchestration::runtime::turn_driver::TurnReceipt::usage";
            let outcome_status =
                invoke_facade(&connection, outcome_status_operation, &started.run, vec![]).await?;
            assert_eq!(outcome_status, serde_json::json!("completed"));
            let status = invoke_facade(&connection, status_operation, &started.run, vec![]).await?;
            assert_eq!(status, serde_json::json!("completed"));
            let usage = invoke_facade(&connection, usage_operation, &started.run, vec![]).await?;
            assert!(
                usage
                    .get("duration_ms")
                    .and_then(serde_json::Value::as_str)
                    .is_some()
            );
            assert!(
                usage
                    .get("iterations")
                    .is_some_and(serde_json::Value::is_null)
            );

            let invalid_arguments = invoke_facade(
                &connection,
                status_operation,
                &started.run,
                vec![serde_json::json!(null)],
            )
            .await
            .expect_err("TurnReceipt accessors must reject arguments");
            let invalid_arguments = typed_facade_error(&invalid_arguments).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            assert_eq!(invalid_arguments.code, ExtensionErrorCode::InvalidValue);

            let invalid_receiver = invoke_facade(&connection, status_operation, &agent, vec![])
                .await
                .expect_err("TurnReceipt receiver must be a Run handle");
            let invalid_receiver = typed_facade_error(&invalid_receiver).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            assert_eq!(invalid_receiver.code, ExtensionErrorCode::InvalidValue);
            Ok(session.acp_session_id)
        })
    })
    .await?;

    let _ = host1.child.start_kill();
    let _ = tokio::time::timeout(Duration::from_secs(5), host1.child.wait()).await;

    let mut host2 = spawn_host(&config).await?;
    let (events, updates, gaps) = empty_collectors::<EventNotification>();
    drive(&mut host2, events, updates, gaps, move |connection| {
        Box::pin(async move {
            connection
                .send_request(initialize_request(Some(client_hello())))
                .block_task()
                .await?;
            let agent = connection
                .send_request(AgentCreateRequest {
                    config: AgentConfigWire::HostDefault,
                    idempotency_id: Some("turn-receipt-recovered-agent".to_string()),
                })
                .block_task()
                .await?
                .agent;
            let loaded = connection
                .send_request(SessionLoadRequest {
                    agent: agent.clone(),
                    session_id,
                    working_dir: None,
                })
                .block_task()
                .await?;
            let recovered = loaded
                .runs
                .iter()
                .find(|run| run.status == RunStatus::Completed)
                .ok_or_else(|| {
                    agent_client_protocol::Error::internal_error()
                        .data("settled run was not recovered")
                })?;
            let outcome_status_operation =
                "echo_orchestration::runtime::turn_driver::TurnOutcome::status";
            let status_operation = "echo_orchestration::runtime::turn_driver::TurnReceipt::status";
            let usage_operation = "echo_orchestration::runtime::turn_driver::TurnReceipt::usage";
            let outcome_status = invoke_facade(
                &connection,
                outcome_status_operation,
                &recovered.run,
                vec![],
            )
            .await?;
            assert_eq!(outcome_status, serde_json::json!("completed"));
            let status =
                invoke_facade(&connection, status_operation, &recovered.run, vec![]).await?;
            assert_eq!(status, serde_json::json!("completed"));
            let usage = invoke_facade(&connection, usage_operation, &recovered.run, vec![]).await?;
            assert!(
                usage
                    .get("duration_ms")
                    .and_then(serde_json::Value::as_str)
                    .is_some()
            );
            assert!(
                usage
                    .get("iterations")
                    .is_some_and(serde_json::Value::is_null)
            );
            Ok(())
        })
    })
    .await?;
    let _ = host2.child.start_kill();
    Ok(())
}

// ── Workflow family over the framework graph engine (plan 07 todo 4) ───────

#[cfg(feature = "sdk-facade-adapters")]
#[tokio::test]
async fn workflow_family_runs_declarative_graphs_over_the_framework_engine()
-> Result<(), Box<dyn std::error::Error>> {
    let (endpoint, _request_seen) = start_model_server("agent-node-done").await?;
    let work = tempfile::tempdir()?;
    let state_root = tempfile::tempdir()?;
    let config = write_config(work.path(), &endpoint, state_root.path(), None)?;
    let mut host = spawn_host(&config).await?;
    let (events, updates, gaps) = empty_collectors::<EventNotification>();

    drive(&mut host, events, updates, gaps, move |connection| {
        Box::pin(async move {
            use agent_client_protocol::UntypedMessage;
            let digest =
                |family: &str, operation: &str| family_op_digest(family, operation);
            let decode = |value: serde_json::Value| -> Result<serde_json::Value, agent_client_protocol::Error> {
                decoded_facade_response(value).map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })
            };
            let typed_error = |error: &agent_client_protocol::Error| -> Result<echo_sdk_protocol::error::EchoSdkError, agent_client_protocol::Error> {
                typed_facade_error(error).map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })
            };
            connection
                .send_request(initialize_request(Some(client_hello())))
                .block_task()
                .await?;
            let agent = connection
                .send_request(AgentCreateRequest {
                    config: AgentConfigWire::HostDefault,
                    idempotency_id: None,
                })
                .block_task()
                .await?
                .agent;
            let session = connection
                .send_request(SessionCreateRequest {
                    agent: agent.clone(),
                    working_dir: None,
                    session_id: None,
                    idempotency_id: None,
                })
                .block_task()
                .await?;
            let workflow_request =
                |operation: &str,
                 handle: echo_sdk_protocol::handle::WireHandle,
                 arguments: Vec<serde_json::Value>|
                 -> Result<serde_json::Value, agent_client_protocol::Error> {
                    let arguments = arguments
                        .into_iter()
                        .map(|value| {
                            echo_sdk_protocol::scalar::WireValue::from_json(value).map_err(
                                |error| {
                                    agent_client_protocol::Error::invalid_params()
                                        .data(error.to_string())
                                },
                            )
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    let request = echo_sdk_protocol::methods::FeatureOperationRequest {
                        operation: operation.to_string(),
                        signature_digest: digest("workflow", operation),
                        handle: Some(handle),
                        arguments,
                    };
                    serde_json::to_value(&request).map_err(|error| {
                        agent_client_protocol::Error::internal_error().data(error.to_string())
                    })
                };
            let workflow_op =
                |operation: &str,
                 handle: echo_sdk_protocol::handle::WireHandle,
                 arguments: Vec<serde_json::Value>|
                 -> Result<UntypedMessage, agent_client_protocol::Error> {
                    UntypedMessage::new(
                        "_echo_agent/workflow/op",
                        workflow_request(operation, handle, arguments)?,
                    )
                };

            // Build a declarative graph with a real agent node (backed by
            // the Session's LLM configuration), a conditional edge and an
            // interrupt before the finish node.
            let definition = serde_json::json!({
                "name": "facade_flow",
                "nodes": [
                    {"name": "agent_step", "type": "agent", "system_prompt": "echo the task",
                     "input_key": "task", "output_key": "agent_out"},
                    {"name": "check", "type": "router"},
                    {"name": "yes", "type": "router"},
                    {"name": "no", "type": "router"},
                    {"name": "end", "type": "router"}
                ],
                "edges": [
                    {"from": "agent_step", "to": "check"},
                    {"from": "check", "condition":
                        {"key": "approved", "equals": true, "then": "yes", "else": "no"}},
                    {"from": "yes", "to": "end"},
                    {"from": "no", "to": "end"}
                ],
                "entry": "agent_step",
                "finish": ["end"],
                "interrupt_before": ["end"]
            })
            .to_string();
            let built = decode(
                connection
                    .send_request(workflow_op(
                        "workflow.graph.build",
                        session.session.clone(),
                        vec![serde_json::json!(definition)],
                    )?)
                    .block_task()
                    .await?,
            )?;
            let graph: echo_sdk_protocol::handle::WireHandle =
                serde_json::from_value(
                    built.get("resource").cloned().ok_or_else(|| {
                        agent_client_protocol::Error::internal_error().data("no graph resource")
                    })?,
                )
                .map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })?;
            assert_eq!(
                graph.kind,
                echo_sdk_protocol::handle::HandleKind::FacadeResource
            );
            let graph_id = graph;
            assert_eq!(built.get("nodes"), Some(&serde_json::json!(5)));
            assert_eq!(built.get("edges"), Some(&serde_json::json!(4)));

            // The first run suspends before `end`; the agent node already
            // executed against the fixture model server.
            let first = decode(
                connection
                    .send_request(workflow_op(
                        "workflow.graph.run_until_interrupt",
                        session.session.clone(),
                        vec![
                            serde_json::json!(graph_id),
                            serde_json::json!({"task": "summarize", "approved": true}),
                        ],
                    )?)
                    .block_task()
                    .await?,
            )?;
            assert_eq!(
                first.get("outcome"),
                Some(&serde_json::json!("interrupted"))
            );
            assert_eq!(first.get("pending_node"), Some(&serde_json::json!("end")));
            let checkpoint = first
                .get("checkpoint")
                .and_then(|value| value.get("id"))
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    agent_client_protocol::Error::internal_error().data("no checkpoint id")
                })?
                .to_string();
            let checkpoint_value = first
                .get("checkpoint")
                .cloned()
                .ok_or_else(|| {
                    agent_client_protocol::Error::internal_error().data("no checkpoint value")
                })?;

            let checkpoints = decode(
                connection
                    .send_request(workflow_op(
                        "workflow.graph.list_checkpoints",
                        session.session.clone(),
                        vec![serde_json::json!(graph_id)],
                    )?)
                    .block_task()
                    .await?,
            )?;
            assert_eq!(
                checkpoints
                    .get("checkpoints")
                    .and_then(|value| value.as_array())
                    .map(Vec::len),
                Some(1)
            );
            let graph_receiver = serde_json::json!({"kind": "handle", "value": graph_id.clone()});
            let by_graph = invoke_facade(
                &connection,
                "echo_orchestration::workflow::graph::Graph::list_checkpoints_by_graph",
                &agent,
                vec![session_argument(&session.session), graph_receiver.clone()],
            )
            .await?;
            assert_eq!(by_graph.as_array().map(Vec::len), Some(1));
            let loaded = invoke_facade(
                &connection,
                "echo_orchestration::workflow::graph::Graph::load_checkpoint",
                &agent,
                vec![
                    session_argument(&session.session),
                    graph_receiver.clone(),
                    serde_json::json!({"kind": "string", "value": checkpoint.clone()}),
                ],
            )
            .await?;
            assert_eq!(
                loaded.get("id").and_then(serde_json::Value::as_str),
                Some(checkpoint.as_str())
            );

            // Approving the checkpoint resumes to completion through the
            // `yes` branch; the agent node's mock answer landed in state.
            let resumed = invoke_facade(
                &connection,
                "echo_orchestration::workflow::graph::Graph::resume",
                &agent,
                vec![
                    session_argument(&session.session),
                    graph_receiver,
                    serde_json::to_value(WireValue::from_json(checkpoint_value).map_err(
                        |error| {
                            agent_client_protocol::Error::internal_error()
                                .data(error.to_string())
                        },
                    )?)
                    .map_err(|error| {
                        agent_client_protocol::Error::internal_error().data(error.to_string())
                    })?,
                    serde_json::json!({"kind": "string", "value": "approve"}),
                ],
            )
            .await?;
            assert_eq!(
                resumed.get("outcome"),
                Some(&serde_json::json!("completed"))
            );
            let path = resumed
                .get("path")
                .and_then(|value| value.as_array())
                .ok_or_else(|| {
                    agent_client_protocol::Error::internal_error().data("no path")
                })?;
            assert!(path.contains(&serde_json::json!("yes")));
            assert!(!path.contains(&serde_json::json!("no")));
            assert_eq!(
                resumed
                    .get("state")
                    .and_then(|value| value.get("values"))
                    .and_then(|value| value.get("agent_out")),
                Some(&serde_json::json!("agent-node-done"))
            );

            // A plain run with approved=false takes the `no` branch to the
            // finish node without interrupting again.
            let second = decode(
                connection
                    .send_request(workflow_op(
                        "workflow.graph.run",
                        session.session.clone(),
                        vec![
                            serde_json::json!(graph_id),
                            serde_json::json!({"task": "summarize", "approved": false}),
                        ],
                    )?)
                    .block_task()
                    .await?,
            )?;
            assert_eq!(
                second.get("outcome"),
                Some(&serde_json::json!("completed"))
            );
            let second_path = second
                .get("path")
                .and_then(|value| value.as_array())
                .ok_or_else(|| {
                    agent_client_protocol::Error::internal_error().data("no path")
                })?;
            assert!(second_path.contains(&serde_json::json!("no")));
            assert!(!second_path.contains(&serde_json::json!("yes")));

            // Standalone SharedState resources keep framework state
            // semantics: set/get/keys/snapshot round-trip.
            let state_new = decode(
                connection
                    .send_request(workflow_op(
                        "workflow.state.new",
                        session.session.clone(),
                        vec![serde_json::json!({"seed": 7})],
                    )?)
                    .block_task()
                    .await?,
            )?;
            let state_id: echo_sdk_protocol::handle::WireHandle =
                serde_json::from_value(
                    state_new.get("resource").cloned().ok_or_else(|| {
                        agent_client_protocol::Error::internal_error().data("no state resource")
                    })?,
                )
                .map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })?;
            assert_eq!(
                state_id.kind,
                echo_sdk_protocol::handle::HandleKind::FacadeResource
            );
            let got = decode(
                connection
                    .send_request(workflow_op(
                        "workflow.state.get",
                        session.session.clone(),
                        vec![serde_json::json!(state_id), serde_json::json!("seed")],
                    )?)
                    .block_task()
                    .await?,
            )?;
            assert_eq!(got.get("value"), Some(&serde_json::json!(7)));
            let set = decode(
                connection
                    .send_request(workflow_op(
                        "workflow.state.set",
                        session.session.clone(),
                        vec![
                            serde_json::json!(state_id),
                            serde_json::json!("extra"),
                            serde_json::json!("value-2"),
                        ],
                    )?)
                    .block_task()
                    .await?,
            )?;
            assert_eq!(set.get("ok"), Some(&serde_json::json!(true)));
            let keys = decode(
                connection
                    .send_request(workflow_op(
                        "workflow.state.keys",
                        session.session.clone(),
                        vec![serde_json::json!(state_id)],
                    )?)
                    .block_task()
                    .await?,
            )?;
            let keys = keys
                .get("keys")
                .and_then(|value| value.as_array())
                .ok_or_else(|| {
                    agent_client_protocol::Error::internal_error().data("no keys")
                })?;
            assert!(keys.contains(&serde_json::json!("seed")));
            assert!(keys.contains(&serde_json::json!("extra")));

            // Cancelling a graph resource makes the next run fail with the
            // framework's cancellation error — the token is real.
            let cancelled_graph = decode(
                connection
                    .send_request(workflow_op(
                        "workflow.graph.build",
                        session.session.clone(),
                        vec![serde_json::json!(
                            serde_json::json!({
                                "name": "cancel_me",
                                "nodes": [
                                    {"name": "a", "type": "router"},
                                    {"name": "b", "type": "router"}
                                ],
                                "edges": [{"from": "a", "to": "b"}],
                                "entry": "a",
                                "finish": ["b"]
                            })
                            .to_string()
                        )],
                    )?)
                    .block_task()
                    .await?,
            )?;
            let cancelled_id = cancelled_graph
                .get("resource")
                .and_then(|value| serde_json::from_value::<echo_sdk_protocol::handle::WireHandle>(value.clone()).ok())
                .ok_or_else(|| {
                    agent_client_protocol::Error::internal_error().data("no graph resource")
                })?;
            let cancelled = decode(
                connection
                    .send_request(workflow_op(
                        "workflow.graph.cancel",
                        session.session.clone(),
                        vec![serde_json::json!(cancelled_id)],
                    )?)
                    .block_task()
                    .await?,
            )?;
            assert_eq!(cancelled.get("cancelled"), Some(&serde_json::json!(true)));
            let refused = connection
                .send_request(workflow_op(
                    "workflow.graph.run",
                    session.session.clone(),
                    vec![serde_json::json!(cancelled_id), serde_json::json!({})],
                )?)
                .block_task()
                .await
                .expect_err("a cancelled graph must refuse further runs");
            let typed = typed_error(&refused)?;
            assert_eq!(typed.code, ExtensionErrorCode::FrameworkError);

            // The family surface is closed: unknown operations and unknown
            // resources fail with typed invalid-value errors.
            let unknown_op = connection
                .send_request(workflow_op(
                    "workflow.graph.teleport",
                    session.session.clone(),
                    vec![serde_json::json!(graph_id)],
                )?)
                .block_task()
                .await
                .expect_err("unknown workflow operation must fail");
            assert_eq!(
                typed_error(&unknown_op)?.code,
                ExtensionErrorCode::InvalidValue
            );
            let unknown_graph = connection
                .send_request(workflow_op(
                    "workflow.graph.run",
                    session.session.clone(),
                    vec![serde_json::json!("wfg-does-not-exist"), serde_json::json!({})],
                )?)
                .block_task()
                .await
                .expect_err("unknown graph resource must fail");
            assert_eq!(
                typed_error(&unknown_graph)?.code,
                ExtensionErrorCode::InvalidValue
            );

            // Graph resources are owner-bound: a second session of the same
            // connection cannot reach the first session's graph.
            let second_agent = connection
                .send_request(AgentCreateRequest {
                    config: AgentConfigWire::HostDefault,
                    idempotency_id: None,
                })
                .block_task()
                .await?
                .agent;
            let second_session = connection
                .send_request(SessionCreateRequest {
                    agent: second_agent,
                    working_dir: None,
                    session_id: None,
                    idempotency_id: None,
                })
                .block_task()
                .await?;
            let cross = connection
                .send_request(workflow_op(
                    "workflow.graph.run",
                    second_session.session.clone(),
                    vec![serde_json::json!(graph_id), serde_json::json!({})],
                )?)
                .block_task()
                .await
                .expect_err("cross-session graph access must fail");
            let cross = typed_error(&cross)?;
            assert_eq!(cross.code, ExtensionErrorCode::InvalidValue);
            Ok(())
        })
    })
    .await?;
    let _ = host.child.kill().await;
    Ok(())
}

// ── State, delivery and trace families (plan 07 todo 4 step 3) ──────────────

#[cfg(feature = "sdk-facade-adapters")]
#[tokio::test]
async fn state_delivery_and_trace_families_use_framework_services()
-> Result<(), Box<dyn std::error::Error>> {
    let (endpoint, _request_seen) = start_model_server("unused").await?;
    let work = tempfile::tempdir()?;
    let state_root = tempfile::tempdir()?;
    let config = write_config(work.path(), &endpoint, state_root.path(), None)?;
    let mut host = spawn_host(&config).await?;
    let (events, updates, gaps) = empty_collectors::<EventNotification>();

    drive(&mut host, events, updates, gaps, move |connection| {
        Box::pin(async move {
            use agent_client_protocol::UntypedMessage;
            let digest =
                |family: &str, operation: &str| family_op_digest(family, operation);
            let decode = |value: serde_json::Value| -> Result<serde_json::Value, agent_client_protocol::Error> {
                decoded_facade_response(value).map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })
            };
            let typed_error = |error: &agent_client_protocol::Error| -> Result<echo_sdk_protocol::error::EchoSdkError, agent_client_protocol::Error> {
                typed_facade_error(error).map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })
            };
            connection
                .send_request(initialize_request(Some(client_hello())))
                .block_task()
                .await?;
            let agent = connection
                .send_request(AgentCreateRequest {
                    config: AgentConfigWire::HostDefault,
                    idempotency_id: None,
                })
                .block_task()
                .await?
                .agent;
            let session = connection
                .send_request(SessionCreateRequest {
                    agent: agent.clone(),
                    working_dir: None,
                    session_id: None,
                    idempotency_id: None,
                })
                .block_task()
                .await?;
            let family_request =
                |method: &str,
                 operation: &str,
                 arguments: Vec<serde_json::Value>|
                 -> Result<UntypedMessage, agent_client_protocol::Error> {
                    let arguments = arguments
                        .into_iter()
                        .map(|value| {
                            echo_sdk_protocol::scalar::WireValue::from_json(value).map_err(
                                |error| {
                                    agent_client_protocol::Error::invalid_params()
                                        .data(error.to_string())
                                },
                            )
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    let family = method
                        .trim_start_matches("_echo_agent/")
                        .trim_end_matches("/op")
                        .replace('-', "_");
                    // Tool families accept the family-qualified spelling
                    // (`git.git_status`); the frozen digest is over the
                    // catalog's unqualified operation identity.
                    let canonical_operation = if [
                        "files", "web", "shell", "git", "database", "rag", "chart", "media",
                        "data", "statistics", "research",
                    ]
                    .contains(&family.as_str())
                    {
                        operation
                            .strip_prefix(&format!("{family}."))
                            .unwrap_or(operation)
                    } else {
                        operation
                    };
                    let request = echo_sdk_protocol::methods::FeatureOperationRequest {
                        operation: operation.to_string(),
                        signature_digest: digest(family.as_str(), canonical_operation),
                        handle: Some(session.session.clone()),
                        arguments,
                    };
                    serde_json::to_value(&request)
                        .map_err(|error| {
                            agent_client_protocol::Error::internal_error().data(error.to_string())
                        })
                        .and_then(|value| UntypedMessage::new(method, value))
                };

            // The state family shares the Host's runtime-state store: save
            // a checkpoint, observe the runtime id, read it back, clear it.
            let checkpoint = serde_json::json!({
                "conversation_id": "scope-e2e",
                "messages_json": "[]",
                "current_plan": null,
                "active_skills": [],
                "blocked_reason": null,
                "timestamp": "2026-09-08T00:00:00Z",
            });
            let saved = decode(
                connection
                    .send_request(family_request(
                        "_echo_agent/state/op",
                        "state.checkpoint.save",
                        vec![serde_json::json!("scope-e2e"), checkpoint],
                    )?)
                    .block_task()
                    .await?,
            )?;
            assert_eq!(saved.get("ok"), Some(&serde_json::json!(true)));
            let ids = decode(
                connection
                    .send_request(family_request(
                        "_echo_agent/state/op",
                        "state.runtime.list",
                        vec![serde_json::json!("scope-e2e")],
                    )?)
                    .block_task()
                    .await?,
            )?;
            let runtime_ids = ids
                .get("runtime_state_ids")
                .and_then(|value| value.as_array())
                .ok_or_else(|| {
                    agent_client_protocol::Error::internal_error().data("no runtime ids")
                })?;
            assert_eq!(runtime_ids.len(), 1);
            let read_back = decode(
                connection
                    .send_request(family_request(
                        "_echo_agent/state/op",
                        "state.checkpoint.get",
                        vec![serde_json::json!("scope-e2e")],
                    )?)
                    .block_task()
                    .await?,
            )?;
            assert_eq!(
                read_back
                    .get("checkpoint")
                    .and_then(|value| value.get("conversation_id")),
                Some(&serde_json::json!("scope-e2e"))
            );
            let cleared = decode(
                connection
                    .send_request(family_request(
                        "_echo_agent/state/op",
                        "state.runtime.clear",
                        vec![
                            serde_json::json!("scope-e2e"),
                            serde_json::json!(runtime_ids[0]),
                        ],
                    )?)
                    .block_task()
                    .await?,
            )?;
            assert_eq!(
                cleared.get("checkpoint_removed"),
                Some(&serde_json::json!(true))
            );

            // The delivery family drives the framework ledger through its
            // real lifecycle: enqueue, claim, effect, settle, recover.
            let ledger = decode(
                connection
                    .send_request(family_request(
                        "_echo_agent/delivery/op",
                        "delivery.ledger.open",
                        vec![],
                    )?)
                    .block_task()
                    .await?,
            )?;
            let ledger_id: echo_sdk_protocol::handle::WireHandle =
                serde_json::from_value(
                    ledger.get("resource").cloned().ok_or_else(|| {
                        agent_client_protocol::Error::internal_error().data("no ledger resource")
                    })?,
                )
                .map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })?;
            assert_eq!(
                ledger_id.kind,
                echo_sdk_protocol::handle::HandleKind::FacadeResource
            );
            let enqueued = decode(
                connection
                    .send_request(family_request(
                        "_echo_agent/delivery/op",
                        "delivery.enqueue",
                        vec![
                            serde_json::json!(ledger_id),
                            serde_json::json!("m-1"),
                            serde_json::json!("channel/primary"),
                            serde_json::json!({"text": "hello"}),
                        ],
                    )?)
                    .block_task()
                    .await?,
            )?;
            assert_eq!(enqueued.get("ok"), Some(&serde_json::json!(true)));
            let claim = decode(
                connection
                    .send_request(family_request(
                        "_echo_agent/delivery/op",
                        "delivery.claim_next",
                        vec![serde_json::json!(ledger_id)],
                    )?)
                    .block_task()
                    .await?,
            )?;
            assert_eq!(
                claim.get("claim").and_then(|value| value.get("message_id")),
                Some(&serde_json::json!("m-1"))
            );
            let transitioned = decode(
                connection
                    .send_request(family_request(
                        "_echo_agent/delivery/op",
                        "delivery.transition",
                        vec![
                            serde_json::json!(ledger_id),
                            serde_json::json!("m-1"),
                            serde_json::json!("effect_started"),
                            serde_json::json!("turn-1"),
                        ],
                    )?)
                    .block_task()
                    .await?,
            )?;
            assert_eq!(transitioned.get("ok"), Some(&serde_json::json!(true)));
            let settled = decode(
                connection
                    .send_request(family_request(
                        "_echo_agent/delivery/op",
                        "delivery.settle",
                        vec![
                            serde_json::json!(ledger_id),
                            serde_json::json!("m-1"),
                            serde_json::json!("completed"),
                        ],
                    )?)
                    .block_task()
                    .await?,
            )?;
            assert_eq!(settled.get("ok"), Some(&serde_json::json!(true)));
            let snapshot = decode(
                connection
                    .send_request(family_request(
                        "_echo_agent/delivery/op",
                        "delivery.snapshot",
                        vec![serde_json::json!(ledger_id)],
                    )?)
                    .block_task()
                    .await?,
            )?;
            let records = snapshot
                .get("records")
                .and_then(|value| value.as_array())
                .ok_or_else(|| {
                    agent_client_protocol::Error::internal_error().data("no records")
                })?;
            assert_eq!(records.len(), 1);
            assert_eq!(
                records[0].get("outcome"),
                Some(&serde_json::json!("completed"))
            );
            let recovered = decode(
                connection
                    .send_request(family_request(
                        "_echo_agent/delivery/op",
                        "delivery.recover",
                        vec![serde_json::json!(ledger_id)],
                    )?)
                    .block_task()
                    .await?,
            )?;
            assert!(
                recovered
                    .get("last_applied_sequence")
                    .and_then(serde_json::Value::as_u64)
                    .is_some_and(|sequence| sequence >= 3)
            );

            // The trace family resource-izes the framework RunStore.
            let store = decode(
                connection
                    .send_request(family_request(
                        "_echo_agent/trace/op",
                        "trace.store.open",
                        vec![serde_json::json!("memory")],
                    )?)
                    .block_task()
                    .await?,
            )?;
            let store_id: echo_sdk_protocol::handle::WireHandle =
                serde_json::from_value(
                    store.get("resource").cloned().ok_or_else(|| {
                        agent_client_protocol::Error::internal_error().data("no store resource")
                    })?,
                )
                .map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })?;
            assert_eq!(
                store_id.kind,
                echo_sdk_protocol::handle::HandleKind::FacadeResource
            );
            let run = serde_json::json!({
                "run_id": "run-e2e-1",
                "session_id": "session-e2e",
                "status": "completed",
                "input": "hello trace",
                "events": [],
                "token_usage": {"prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0},
                "timings": {"total_duration_ms": 0, "llm_duration_ms": 0, "tool_duration_ms": 0},
                "started_at": "2026-09-08T00:00:00Z",
                "finished_at": "2026-09-08T00:00:01Z",
            });
            let saved_run = decode(
                connection
                    .send_request(family_request(
                        "_echo_agent/trace/op",
                        "trace.run.save",
                        vec![serde_json::json!(store_id), run],
                    )?)
                    .block_task()
                    .await?,
            )?;
            assert_eq!(saved_run.get("ok"), Some(&serde_json::json!(true)));
            let loaded = decode(
                connection
                    .send_request(family_request(
                        "_echo_agent/trace/op",
                        "trace.run.load",
                        vec![serde_json::json!(store_id), serde_json::json!("run-e2e-1")],
                    )?)
                    .block_task()
                    .await?,
            )?;
            assert_eq!(
                loaded.get("run").and_then(|value| value.get("input")),
                Some(&serde_json::json!("hello trace"))
            );
            let recent = decode(
                connection
                    .send_request(family_request(
                        "_echo_agent/trace/op",
                        "trace.run.list_recent",
                        vec![serde_json::json!(store_id), serde_json::json!(10)],
                    )?)
                    .block_task()
                    .await?,
            )?;
            assert_eq!(
                recent
                    .get("summaries")
                    .and_then(|value| value.as_array())
                    .map(Vec::len),
                Some(1)
            );

            // Closed surfaces: unknown operations fail with typed
            // invalid-value errors.
            let unknown = connection
                .send_request(family_request(
                    "_echo_agent/state/op",
                    "state.checkpoint.vacuum",
                    vec![serde_json::json!("scope-e2e")],
                )?)
                .block_task()
                .await
                .expect_err("unknown state operation must fail");
            assert_eq!(typed_error(&unknown)?.code, ExtensionErrorCode::InvalidValue);
            Ok(())
        })
    })
    .await?;
    let _ = host.child.kill().await;
    Ok(())
}

// ── Eval and improve families (plan 07 todo 4 step 3, feature-gated) ────────

#[cfg(all(
    feature = "sdk-facade-adapters",
    feature = "framework-eval",
    feature = "framework-improve"
))]
#[tokio::test]
async fn eval_and_improve_families_use_the_framework_analyzers()
-> Result<(), Box<dyn std::error::Error>> {
    let (endpoint, _request_seen) = start_model_server("unused").await?;
    let work = tempfile::tempdir()?;
    let state_root = tempfile::tempdir()?;
    let config = write_config(work.path(), &endpoint, state_root.path(), None)?;
    let mut host = spawn_host(&config).await?;
    let (events, updates, gaps) = empty_collectors::<EventNotification>();

    drive(&mut host, events, updates, gaps, move |connection| {
        Box::pin(async move {
            use agent_client_protocol::UntypedMessage;
            let digest =
                |family: &str, operation: &str| family_op_digest(family, operation);
            let decode = |value: serde_json::Value| -> Result<serde_json::Value, agent_client_protocol::Error> {
                decoded_facade_response(value).map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })
            };
            let typed_error = |error: &agent_client_protocol::Error| -> Result<echo_sdk_protocol::error::EchoSdkError, agent_client_protocol::Error> {
                typed_facade_error(error).map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })
            };
            connection
                .send_request(initialize_request(Some(client_hello())))
                .block_task()
                .await?;
            let agent = connection
                .send_request(AgentCreateRequest {
                    config: AgentConfigWire::HostDefault,
                    idempotency_id: None,
                })
                .block_task()
                .await?
                .agent;
            let session = connection
                .send_request(SessionCreateRequest {
                    agent: agent.clone(),
                    working_dir: None,
                    session_id: None,
                    idempotency_id: None,
                })
                .block_task()
                .await?;
            let family_request =
                |method: &str,
                 operation: &str,
                 arguments: Vec<serde_json::Value>|
                 -> Result<UntypedMessage, agent_client_protocol::Error> {
                    let arguments = arguments
                        .into_iter()
                        .map(|value| {
                            echo_sdk_protocol::scalar::WireValue::from_json(value).map_err(
                                |error| {
                                    agent_client_protocol::Error::invalid_params()
                                        .data(error.to_string())
                                },
                            )
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    let family = method
                        .trim_start_matches("_echo_agent/")
                        .trim_end_matches("/op")
                        .replace('-', "_");
                    // Tool families accept the family-qualified spelling
                    // (`git.git_status`); the frozen digest is over the
                    // catalog's unqualified operation identity.
                    let canonical_operation = if [
                        "files", "web", "shell", "git", "database", "rag", "chart", "media",
                        "data", "statistics", "research",
                    ]
                    .contains(&family.as_str())
                    {
                        operation
                            .strip_prefix(&format!("{family}."))
                            .unwrap_or(operation)
                    } else {
                        operation
                    };
                    let request = echo_sdk_protocol::methods::FeatureOperationRequest {
                        operation: operation.to_string(),
                        signature_digest: digest(family.as_str(), canonical_operation),
                        handle: Some(session.session.clone()),
                        arguments,
                    };
                    serde_json::to_value(&request)
                        .map_err(|error| {
                            agent_client_protocol::Error::internal_error().data(error.to_string())
                        })
                        .and_then(|value| UntypedMessage::new(method, value))
                };

            let run = serde_json::json!({
                "run_id": "run-eval-1",
                "session_id": "session-eval",
                "status": "completed",
                "input": "analyze me",
                "events": [],
                "token_usage": {"prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0},
                "timings": {"total_duration_ms": 0, "llm_duration_ms": 0, "tool_duration_ms": 0},
                "started_at": "2026-09-08T00:00:00Z",
                "finished_at": "2026-09-08T00:00:01Z",
            });

            // Constraint evaluation is the framework's own runner over
            // the run trace; an empty constraint set yields no violations.
            let constraints = decode(
                connection
                    .send_request(family_request(
                        "_echo_agent/eval/op",
                        "eval.constraints.run",
                        vec![serde_json::json!({}), run.clone()],
                    )?)
                    .block_task()
                    .await?,
            )?;
            assert_eq!(
                constraints
                    .get("violations")
                    .and_then(|value| value.as_array())
                    .map(Vec::len),
                Some(0)
            );

            // Reports aggregate results with the framework's own shape.
            let report = decode(
                connection
                    .send_request(family_request(
                        "_echo_agent/eval/op",
                        "eval.report.build",
                        vec![serde_json::json!([
                            {"case_id": "c-1", "success": true, "score": 1.0,
                             "metrics": [], "violations": [], "duration_ms": 10},
                            {"case_id": "c-2", "success": false, "score": 0.0,
                             "metrics": [], "violations": [], "duration_ms": 5},
                        ])],
                    )?)
                    .block_task()
                    .await?,
            )?;
            assert_eq!(report.get("total"), Some(&serde_json::json!(2)));
            assert_eq!(report.get("passed"), Some(&serde_json::json!(1)));
            assert_eq!(report.get("failed"), Some(&serde_json::json!(1)));

            // The improve family exports ShareGPT trajectories and runs the
            // real analyzer over the trace.
            let trajectory = decode(
                connection
                    .send_request(family_request(
                        "_echo_agent/improve/op",
                        "improve.trajectory.sharegpt",
                        vec![run.clone()],
                    )?)
                    .block_task()
                    .await?,
            )?;
            assert!(
                trajectory
                    .get("messages")
                    .and_then(|value| value.as_array())
                    .is_some()
            );
            let critique = decode(
                connection
                    .send_request(family_request(
                        "_echo_agent/improve/op",
                        "improve.run.analyze",
                        vec![run],
                    )?)
                    .block_task()
                    .await?,
            )?;
            assert_eq!(critique.get("run_id"), Some(&serde_json::json!("run-eval-1")));
            assert_eq!(critique.get("success"), Some(&serde_json::json!(true)));

            // Unknown operations stay closed.
            let unknown = connection
                .send_request(family_request(
                    "_echo_agent/eval/op",
                    "eval.magic.optimize",
                    vec![],
                )?)
                .block_task()
                .await
                .expect_err("unknown eval operation must fail");
            assert_eq!(typed_error(&unknown)?.code, ExtensionErrorCode::InvalidValue);
            Ok(())
        })
    })
    .await?;
    let _ = host.child.kill().await;
    Ok(())
}

// ── Tool families over the framework tools (plan 07 todo 5) ─────────────────

#[cfg(all(
    feature = "sdk-facade-adapters",
    feature = "framework-files",
    feature = "framework-shell",
    feature = "framework-git",
    feature = "framework-data",
    feature = "framework-web",
    feature = "framework-content-guard",
    feature = "framework-project-rules"
))]
#[tokio::test]
async fn tool_families_execute_framework_tools_with_the_session_cwd()
-> Result<(), Box<dyn std::error::Error>> {
    let (endpoint, _request_seen) = start_model_server("unused").await?;
    let work = tempfile::tempdir()?;
    let state_root = tempfile::tempdir()?;
    let project_root = work.path().join("project-rules-root");
    let project_child = project_root.join("nested");
    std::fs::create_dir_all(&project_child)?;
    std::fs::write(project_root.join("AGENTS.md"), "root agents rule")?;
    std::fs::write(project_child.join("AGENTS.md"), "nested agents rule")?;
    std::fs::write(project_child.join("CLAUDE.md"), "must be excluded")?;
    let config = write_config(work.path(), &endpoint, state_root.path(), None)?;
    let mut host = spawn_host(&config).await?;
    let (events, updates, gaps) = empty_collectors::<EventNotification>();

    drive(&mut host, events, updates, gaps, move |connection| {
        let project_root = project_root.clone();
        let project_child = project_child.clone();
        Box::pin(async move {
            use agent_client_protocol::UntypedMessage;
            let digest =
                |family: &str, operation: &str| family_op_digest(family, operation);
            let decode = |value: serde_json::Value| -> Result<serde_json::Value, agent_client_protocol::Error> {
                decoded_facade_response(value).map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })
            };
            let typed_error = |error: &agent_client_protocol::Error| -> Result<echo_sdk_protocol::error::EchoSdkError, agent_client_protocol::Error> {
                typed_facade_error(error).map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })
            };
            connection
                .send_request(initialize_request(Some(client_hello())))
                .block_task()
                .await?;
            let agent = connection
                .send_request(AgentCreateRequest {
                    config: AgentConfigWire::HostDefault,
                    idempotency_id: None,
                })
                .block_task()
                .await?
                .agent;
            // The session's working directory is the tool workspace.
            let work_dir = work.path().display().to_string();
            let session = connection
                .send_request(SessionCreateRequest {
                    agent: agent.clone(),
                    working_dir: Some(echo_sdk_protocol::scalar::WirePath::Utf8 {
                        path: work_dir.clone(),
                    }),
                    session_id: None,
                    idempotency_id: None,
                })
                .block_task()
                .await?;
            let tool_request =
                |method: &str,
                 operation: &str,
                 arguments: Vec<serde_json::Value>|
                 -> Result<UntypedMessage, agent_client_protocol::Error> {
                    let arguments = arguments
                        .into_iter()
                        .map(|value| {
                            echo_sdk_protocol::scalar::WireValue::from_json(value).map_err(
                                |error| {
                                    agent_client_protocol::Error::invalid_params()
                                        .data(error.to_string())
                                },
                            )
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    let family = method
                        .trim_start_matches("_echo_agent/")
                        .trim_end_matches("/op")
                        .replace('-', "_");
                    // Tool families accept the family-qualified spelling
                    // (`git.git_status`); the frozen digest is over the
                    // catalog's unqualified operation identity.
                    let canonical_operation = if [
                        "files", "web", "shell", "git", "database", "rag", "chart", "media",
                        "data", "statistics", "research",
                    ]
                    .contains(&family.as_str())
                    {
                        operation
                            .strip_prefix(&format!("{family}."))
                            .unwrap_or(operation)
                    } else {
                        operation
                    };
                    let request = echo_sdk_protocol::methods::FeatureOperationRequest {
                        operation: operation.to_string(),
                        signature_digest: digest(family.as_str(), canonical_operation),
                        handle: Some(session.session.clone()),
                        arguments,
                    };
                    serde_json::to_value(&request)
                        .map_err(|error| {
                            agent_client_protocol::Error::internal_error().data(error.to_string())
                        })
                        .and_then(|value| UntypedMessage::new(method, value))
                };

            // files: write then read a file relative to the session cwd.
            let written = decode(
                connection
                    .send_request(tool_request(
                        "_echo_agent/files/op",
                        "files.write_file",
                        vec![
                            serde_json::json!("notes/tool-family.txt"),
                            serde_json::json!("written by the facade tool family"),
                        ],
                    )?)
                    .block_task()
                    .await?,
            )?;
            assert_eq!(written.get("success"), Some(&serde_json::json!(true)));
            let read = decode(
                connection
                    .send_request(tool_request(
                        "_echo_agent/files/op",
                        "files.read_file",
                        vec![serde_json::json!("notes/tool-family.txt")],
                    )?)
                    .block_task()
                    .await?,
            )?;
            assert_eq!(read.get("success"), Some(&serde_json::json!(true)));
            assert!(
                read.get("output")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|text| text.contains("written by the facade tool family"))
            );

            // shell: run echo through the framework ShellTool.
            let shell = decode(
                connection
                    .send_request(tool_request(
                        "_echo_agent/shell/op",
                        "shell.shell",
                        vec![serde_json::json!("printf facade-shell-ok")],
                    )?)
                    .block_task()
                    .await?,
            )?;
            assert_eq!(shell.get("success"), Some(&serde_json::json!(true)));
            assert!(
                shell
                    .get("output")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|text| text.contains("facade-shell-ok"))
            );

            // web: extract structured text from HTML without any network.
            let extracted = decode(
                connection
                    .send_request(tool_request(
                        "_echo_agent/web/op",
                        "web.web_extract",
                        vec![serde_json::json!("<html><body><h1>Facade Head</h1><p>Body text</p></body></html>")],
                    )?)
                    .block_task()
                    .await?,
            )?;
            assert_eq!(extracted.get("success"), Some(&serde_json::json!(true)));

            // content-guard: the framework PII detector finds an email.
            let pii = decode(
                connection
                    .send_request(tool_request(
                        "_echo_agent/content-guard/op",
                        "content-guard.detect",
                        vec![serde_json::json!("contact me at alice@example.com please")],
                    )?)
                    .block_task()
                    .await?,
            )?;
            assert!(
                pii.get("matches")
                    .and_then(|value| value.as_array())
                    .is_some_and(|matches| !matches.is_empty())
            );

            let guard = invoke_facade_wire(
                &connection,
                "echo_core::guard::content::ContentGuard::new",
                &agent,
                vec![WireValue::Handle(session.session.clone()), WireValue::String("reject".to_string())],
            )
            .await?;
            let WireValue::Handle(guard) = guard else {
                return Err(agent_client_protocol::Error::internal_error()
                    .data("ContentGuard::new did not return a handle"));
            };
            let clean = invoke_facade(
                &connection,
                "echo_core::guard::content::ContentGuard::is_clean",
                &agent,
                vec![
                    session_argument(&session.session),
                    serde_json::json!({"kind": "handle", "value": guard.clone()}),
                    serde_json::json!({"kind": "string", "value": "plain text"}),
                ],
            )
            .await?;
            assert_eq!(clean, serde_json::json!(true));
            let rejected = invoke_facade_wire(
                &connection,
                "echo_core::guard::content::ContentGuard::check",
                &agent,
                vec![
                    WireValue::Handle(session.session.clone()),
                    WireValue::Handle(guard),
                    WireValue::String("alice@example.com".to_string()),
                ],
            )
            .await?;
            assert!(matches!(
                rejected,
                WireValue::Variant { ref variant, .. } if variant == "rejected"
            ));

            // project-rules: an empty temp workspace resolves no instruction
            // sources — the framework resolver's own answer.
            let resolved = decode(
                connection
                    .send_request(tool_request(
                        "_echo_agent/project-rules/op",
                        "project-rules.resolve",
                        vec![],
                    )?)
                    .block_task()
                    .await?,
            )?;
            assert_eq!(
                resolved.get("is_empty"),
                Some(&serde_json::json!(true))
            );

            let path_argument = |path: &std::path::Path| {
                WireValue::Path(WirePath::Utf8 {
                    path: path.display().to_string(),
                })
            };
            let resolver = invoke_facade_wire(
                &connection,
                "echo_core::project_rules::InstructionResolver::new",
                &agent,
                vec![
                    WireValue::Handle(session.session.clone()),
                    path_argument(&project_child),
                ],
            )
            .await?;
            let WireValue::Handle(resolver) = resolver else {
                return Err(agent_client_protocol::Error::internal_error()
                    .data("InstructionResolver::new did not return a handle"));
            };
            for (operation, extra) in [
                (
                    "echo_core::project_rules::InstructionResolver::project_root",
                    Some(path_argument(&project_root)),
                ),
                (
                    "echo_core::project_rules::InstructionResolver::agents_files_only",
                    None,
                ),
            ] {
                let mut arguments = vec![
                    WireValue::Handle(session.session.clone()),
                    WireValue::Handle(resolver.clone()),
                ];
                if let Some(extra) = extra {
                    arguments.push(extra);
                }
                invoke_facade_wire(&connection, operation, &agent, arguments).await?;
            }
            let resolved = invoke_facade_wire(
                &connection,
                "echo_core::project_rules::InstructionResolver::resolve",
                &agent,
                vec![
                    WireValue::Handle(session.session.clone()),
                    WireValue::Handle(resolver),
                ],
            )
            .await?;
            let WireValue::Record { fields, .. } = resolved else {
                return Err(agent_client_protocol::Error::internal_error()
                    .data("InstructionResolver::resolve did not return a record"));
            };
            let content = fields
                .iter()
                .find(|field| field.name == "content")
                .and_then(|field| match &field.value {
                    WireValue::String(value) => Some(value.as_str()),
                    _ => None,
                })
                .unwrap_or_default();
            assert!(content.contains("root agents rule"));
            assert!(content.contains("nested agents rule"));
            assert!(!content.contains("must be excluded"));

            // Unknown tools stay closed with typed invalid-value errors.
            let unknown = connection
                .send_request(tool_request(
                    "_echo_agent/files/op",
                    "files.magic_teleport",
                    vec![],
                )?)
                .block_task()
                .await
                .expect_err("unknown tool must fail");
            assert_eq!(typed_error(&unknown)?.code, ExtensionErrorCode::InvalidValue);
            Ok(())
        })
    })
    .await?;
    let _ = host.child.kill().await;
    Ok(())
}

// ── Integration families (plan 07 todo 5) ───────────────────────────────────

#[cfg(all(
    feature = "sdk-facade-adapters",
    feature = "framework-a2a",
    feature = "framework-lsp",
    feature = "framework-topology"
))]
#[tokio::test]
async fn integration_families_use_framework_managers_and_clients()
-> Result<(), Box<dyn std::error::Error>> {
    let (endpoint, _request_seen) = start_model_server("unused").await?;
    let work = tempfile::tempdir()?;
    let state_root = tempfile::tempdir()?;
    let config = write_config(work.path(), &endpoint, state_root.path(), None)?;
    let mut host = spawn_host(&config).await?;
    let (events, updates, gaps) = empty_collectors::<EventNotification>();

    drive(&mut host, events, updates, gaps, move |connection| {
        Box::pin(async move {
            use agent_client_protocol::UntypedMessage;
            let digest =
                |family: &str, operation: &str| family_op_digest(family, operation);
            let decode = |value: serde_json::Value| -> Result<serde_json::Value, agent_client_protocol::Error> {
                decoded_facade_response(value).map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })
            };
            let typed_error = |error: &agent_client_protocol::Error| -> Result<echo_sdk_protocol::error::EchoSdkError, agent_client_protocol::Error> {
                typed_facade_error(error).map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })
            };
            connection
                .send_request(initialize_request(Some(client_hello())))
                .block_task()
                .await?;
            let agent = connection
                .send_request(AgentCreateRequest {
                    config: AgentConfigWire::HostDefault,
                    idempotency_id: None,
                })
                .block_task()
                .await?
                .agent;
            let session = connection
                .send_request(SessionCreateRequest {
                    agent: agent.clone(),
                    working_dir: None,
                    session_id: None,
                    idempotency_id: None,
                })
                .block_task()
                .await?;
            let family_request =
                |method: &str,
                 operation: &str,
                 arguments: Vec<serde_json::Value>|
                 -> Result<UntypedMessage, agent_client_protocol::Error> {
                    let arguments = arguments
                        .into_iter()
                        .map(|value| {
                            echo_sdk_protocol::scalar::WireValue::from_json(value).map_err(
                                |error| {
                                    agent_client_protocol::Error::invalid_params()
                                        .data(error.to_string())
                                },
                            )
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    let family = method
                        .trim_start_matches("_echo_agent/")
                        .trim_end_matches("/op")
                        .replace('-', "_");
                    // Tool families accept the family-qualified spelling
                    // (`git.git_status`); the frozen digest is over the
                    // catalog's unqualified operation identity.
                    let canonical_operation = if [
                        "files", "web", "shell", "git", "database", "rag", "chart", "media",
                        "data", "statistics", "research",
                    ]
                    .contains(&family.as_str())
                    {
                        operation
                            .strip_prefix(&format!("{family}."))
                            .unwrap_or(operation)
                    } else {
                        operation
                    };
                    let request = echo_sdk_protocol::methods::FeatureOperationRequest {
                        operation: operation.to_string(),
                        signature_digest: digest(family.as_str(), canonical_operation),
                        handle: Some(session.session.clone()),
                        arguments,
                    };
                    serde_json::to_value(&request)
                        .map_err(|error| {
                            agent_client_protocol::Error::internal_error().data(error.to_string())
                        })
                        .and_then(|value| UntypedMessage::new(method, value))
                };

            // MCP: a fresh manager reports no servers; connecting to a
            // command that cannot start is the framework's own failure.
            let manager = decode(
                connection
                    .send_request(family_request(
                        "_echo_agent/mcp/op",
                        "mcp.manager.open",
                        vec![],
                    )?)
                    .block_task()
                    .await?,
            )?;
            let manager_id: echo_sdk_protocol::handle::WireHandle =
                serde_json::from_value(
                    manager.get("resource").cloned().ok_or_else(|| {
                        agent_client_protocol::Error::internal_error().data("no manager resource")
                    })?,
                )
                .map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })?;
            assert_eq!(
                manager_id.kind,
                echo_sdk_protocol::handle::HandleKind::FacadeResource
            );
            let servers = decode(
                connection
                    .send_request(family_request(
                        "_echo_agent/mcp/op",
                        "mcp.server.list",
                        vec![serde_json::json!(manager_id)],
                    )?)
                    .block_task()
                    .await?,
            )?;
            assert_eq!(
                servers
                    .get("servers")
                    .and_then(|value| value.as_array())
                    .map(Vec::len),
                Some(0)
            );
            let refused = connection
                .send_request(family_request(
                    "_echo_agent/mcp/op",
                    "mcp.server.connect",
                    vec![
                        serde_json::json!(manager_id),
                        serde_json::json!("broken"),
                        serde_json::json!("stdio"),
                        serde_json::json!("/nonexistent/definitely-not-a-binary"),
                        serde_json::json!([]),
                    ],
                )?)
                .block_task()
                .await
                .expect_err("a broken MCP server must fail with the framework error");
            assert_eq!(
                typed_error(&refused)?.code,
                ExtensionErrorCode::FrameworkError
            );

            // A2A: discovery against a closed loopback port surfaces the
            // framework client's real connection failure.
            let client = decode(
                connection
                    .send_request(family_request(
                        "_echo_agent/a2a/op",
                        "a2a.client.open",
                        vec![],
                    )?)
                    .block_task()
                    .await?,
            )?;
            let client_id: echo_sdk_protocol::handle::WireHandle =
                serde_json::from_value(
                    client.get("resource").cloned().ok_or_else(|| {
                        agent_client_protocol::Error::internal_error().data("no client resource")
                    })?,
                )
                .map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })?;
            assert_eq!(
                client_id.kind,
                echo_sdk_protocol::handle::HandleKind::FacadeResource
            );
            let discovery = connection
                .send_request(family_request(
                    "_echo_agent/a2a/op",
                    "a2a.discover",
                    vec![
                        serde_json::json!(client_id),
                        serde_json::json!("http://127.0.0.1:9/.well-known/agent.json"),
                    ],
                )?)
                .block_task()
                .await
                .expect_err("a closed port must fail discovery");
            assert_eq!(
                typed_error(&discovery)?.code,
                ExtensionErrorCode::FrameworkError
            );

            // LSP: a fresh manager reports no running servers.
            let lsp = decode(
                connection
                    .send_request(family_request(
                        "_echo_agent/lsp/op",
                        "lsp.manager.open",
                        vec![],
                    )?)
                    .block_task()
                    .await?,
            )?;
            let lsp_id: echo_sdk_protocol::handle::WireHandle =
                serde_json::from_value(
                    lsp.get("resource").cloned().ok_or_else(|| {
                        agent_client_protocol::Error::internal_error()
                            .data("no lsp manager resource")
                    })?,
                )
                .map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })?;
            assert_eq!(
                lsp_id.kind,
                echo_sdk_protocol::handle::HandleKind::FacadeResource
            );
            let statuses = decode(
                connection
                    .send_request(family_request(
                        "_echo_agent/lsp/op",
                        "lsp.server.status",
                        vec![serde_json::json!(lsp_id)],
                    )?)
                    .block_task()
                    .await?,
            )?;
            assert_eq!(
                statuses
                    .get("servers")
                    .and_then(|value| value.as_array())
                    .map(Vec::len),
                Some(0)
            );
            let lsp_receiver = serde_json::json!({"kind": "handle", "value": lsp_id});
            for operation in [
                "echo_integration::lsp::manager::LspManager::configured_languages",
                "echo_integration::lsp::manager::LspManager::running_servers",
                "echo_integration::lsp::manager::LspManager::status_all",
            ] {
                let value = invoke_facade(
                    &connection,
                    operation,
                    &agent,
                    vec![session_argument(&session.session), lsp_receiver.clone()],
                )
                .await?;
                assert!(
                    value.is_array(),
                    "{operation} must preserve its Rust vector result"
                );
            }

            // Topology: full round trip through the framework tracker.
            let tracker = decode(
                connection
                    .send_request(family_request(
                        "_echo_agent/topology/op",
                        "topology.tracker.open",
                        vec![],
                    )?)
                    .block_task()
                    .await?,
            )?;
            let tracker_id: echo_sdk_protocol::handle::WireHandle =
                serde_json::from_value(
                    tracker.get("resource").cloned().ok_or_else(|| {
                        agent_client_protocol::Error::internal_error().data("no tracker resource")
                    })?,
                )
                .map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })?;
            assert_eq!(
                tracker_id.kind,
                echo_sdk_protocol::handle::HandleKind::FacadeResource
            );
            for (node, kind) in [("orchestrator", "orchestrator"), ("researcher", "subagent")] {
                let added = decode(
                    connection
                        .send_request(family_request(
                            "_echo_agent/topology/op",
                            "topology.node.add",
                            vec![
                                serde_json::json!(tracker_id),
                                serde_json::json!(node),
                                serde_json::json!(kind),
                            ],
                        )?)
                        .block_task()
                        .await?,
                )?;
                assert_eq!(added.get("ok"), Some(&serde_json::json!(true)));
            }
            let recorded = decode(
                connection
                    .send_request(family_request(
                        "_echo_agent/topology/op",
                        "topology.call.record",
                        vec![
                            serde_json::json!(tracker_id),
                            serde_json::json!("orchestrator"),
                            serde_json::json!("researcher"),
                            serde_json::json!("dispatch"),
                        ],
                    )?)
                    .block_task()
                    .await?,
            )?;
            assert_eq!(recorded.get("ok"), Some(&serde_json::json!(true)));
            let snapshot = decode(
                connection
                    .send_request(family_request(
                        "_echo_agent/topology/op",
                        "topology.snapshot",
                        vec![serde_json::json!(tracker_id)],
                    )?)
                    .block_task()
                    .await?,
            )?;
            assert_eq!(
                snapshot
                    .get("nodes")
                    .and_then(|value| value.as_array())
                    .map(Vec::len),
                Some(2)
            );
            assert_eq!(
                snapshot
                    .get("edges")
                    .and_then(|value| value.as_array())
                    .map(Vec::len),
                Some(1)
            );
            let topology_receiver = serde_json::json!({
                "kind": "handle",
                "value": tracker_id.clone()
            });
            let exact_node = WireValue::from_json(serde_json::json!({
                "id": "writer",
                "label": "Writer Agent",
                "node_type": "Subagent",
                "metadata": {"model": "fixture-model"}
            }))
            .map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            invoke_facade(
                &connection,
                "echo_agent::topology::TopologyTracker::add_node",
                &agent,
                vec![
                    session_argument(&session.session),
                    topology_receiver.clone(),
                    serde_json::to_value(exact_node).map_err(|error| {
                        agent_client_protocol::Error::internal_error().data(error.to_string())
                    })?,
                ],
            )
            .await?;
            let nodes = invoke_facade(
                &connection,
                "echo_agent::topology::TopologyTracker::nodes",
                &agent,
                vec![session_argument(&session.session), topology_receiver.clone()],
            )
            .await?;
            assert_eq!(nodes.as_array().map(Vec::len), Some(3));
            assert!(nodes.as_array().is_some_and(|nodes| nodes.iter().any(|node| {
                node.get("label").and_then(serde_json::Value::as_str) == Some("Writer Agent")
                    && node
                        .get("metadata")
                        .and_then(|metadata| metadata.get("model"))
                        .and_then(serde_json::Value::as_str)
                        == Some("fixture-model")
            })));
            let edges = invoke_facade(
                &connection,
                "echo_agent::topology::TopologyTracker::edges",
                &agent,
                vec![session_argument(&session.session), topology_receiver.clone()],
            )
            .await?;
            assert_eq!(edges.as_array().map(Vec::len), Some(1));
            let stats = invoke_facade(
                &connection,
                "echo_agent::topology::TopologyTracker::stats",
                &agent,
                vec![session_argument(&session.session), topology_receiver],
            )
            .await?;
            assert!(stats.is_object());
            assert!(stats.get("nodes").is_none(), "stats must not return the full snapshot");

            // Owner isolation: a second session cannot reach the tracker.
            let second_agent = connection
                .send_request(AgentCreateRequest {
                    config: AgentConfigWire::HostDefault,
                    idempotency_id: None,
                })
                .block_task()
                .await?
                .agent;
            let second_session = connection
                .send_request(SessionCreateRequest {
                    agent: second_agent,
                    working_dir: None,
                    session_id: None,
                    idempotency_id: None,
                })
                .block_task()
                .await?;
            let request_for_other = |session: echo_sdk_protocol::handle::WireHandle,
                                     arguments: Vec<serde_json::Value>|
                 -> Result<UntypedMessage, agent_client_protocol::Error> {
                let arguments = arguments
                    .into_iter()
                    .map(|value| {
                        echo_sdk_protocol::scalar::WireValue::from_json(value).map_err(|error| {
                            agent_client_protocol::Error::invalid_params().data(error.to_string())
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let request = echo_sdk_protocol::methods::FeatureOperationRequest {
                    operation: "topology.snapshot".to_string(),
                    signature_digest: digest("topology", "topology.snapshot"),
                    handle: Some(session),
                    arguments,
                };
                serde_json::to_value(&request)
                    .map_err(|error| {
                        agent_client_protocol::Error::internal_error().data(error.to_string())
                    })
                    .and_then(|value| UntypedMessage::new("_echo_agent/topology/op", value))
            };
            let cross = connection
                .send_request(request_for_other(
                    second_session.session.clone(),
                    vec![serde_json::json!(tracker_id)],
                )?)
                .block_task()
                .await
                .expect_err("cross-session topology access must fail");
            assert_eq!(typed_error(&cross)?.code, ExtensionErrorCode::InvalidValue);
            Ok(())
        })
    })
    .await?;
    let _ = host.child.kill().await;
    Ok(())
}
// ── Remaining tool families (plan 07 todo 6) ─────────────────────────────────

#[cfg(all(
    feature = "sdk-facade-adapters",
    feature = "framework-git",
    feature = "framework-database",
    feature = "framework-rag",
    feature = "framework-chart",
    feature = "framework-media",
    feature = "framework-data",
    feature = "framework-statistics",
    feature = "framework-research"
))]
#[tokio::test]
async fn remaining_tool_families_execute_framework_tools_locally()
-> Result<(), Box<dyn std::error::Error>> {
    let (endpoint, _request_seen) = start_model_server("unused").await?;
    // Fixtures live under the workspace `target/` tree so the Host child
    // process (which may run under a stricter file sandbox than the test)
    // sees exactly what the test creates.
    std::fs::create_dir_all("../target").map_err(|error| format!("target dir: {error}"))?;
    let work = tempfile::Builder::new()
        .prefix("facade-tools-e2e-")
        .tempdir_in("../target")?;
    let state_root = tempfile::Builder::new()
        .prefix("facade-tools-state-")
        .tempdir_in("../target")?;
    // Canonicalize so the fixture path handed to the Host carries no `..`
    // component (the git family rejects traversal patterns).
    let work_root = std::fs::canonicalize(work.path())?;
    let config = write_config(&work_root, &endpoint, state_root.path(), None)?;
    let mut host = spawn_host(&config).await?;
    let (events, updates, gaps) = empty_collectors::<EventNotification>();

    // A real git repo plus local data files: every family call below stays
    // inside this temp workspace, no network involved.
    let git_init = std::process::Command::new("git")
        .arg("init")
        .arg(&work_root)
        .output()
        .map_err(|error| format!("git init failed: {error}"))?;
    if !git_init.status.success() {
        return Err(format!(
            "git init failed: {}",
            String::from_utf8_lossy(&git_init.stderr)
        )
        .into());
    }
    let verify = std::process::Command::new("git")
        .arg("-C")
        .arg(&work_root)
        .arg("status")
        .arg("--short")
        .output()
        .map_err(|error| format!("git verify failed: {error}"))?;
    if !verify.status.success() {
        return Err(format!(
            "git verify failed in {}: {}",
            work_root.display(),
            String::from_utf8_lossy(&verify.stderr)
        )
        .into());
    }
    std::fs::write(work_root.join("notes.txt"), "alpha beta\ngamma delta\n")?;
    std::fs::write(work_root.join("points.csv"), "value\n1\n2\n3\n4\n")?;
    let sqlite_db = work_root.join("points.db");
    let bootstrap = std::process::Command::new("python3")
        .arg("-c")
        .arg(format!(
            "import sqlite3; c = sqlite3.connect({db:?});              c.execute('CREATE TABLE points (x INTEGER, y INTEGER)');              c.executemany('INSERT INTO points VALUES (?, ?)', [(1, 2), (3, 4)]);              c.commit()",
            db = sqlite_db.display().to_string()
        ))
        .output()
        .map_err(|error| format!("sqlite bootstrap failed: {error}"))?;
    if !bootstrap.status.success() {
        return Err(format!(
            "sqlite bootstrap failed: {}",
            String::from_utf8_lossy(&bootstrap.stderr)
        )
        .into());
    }

    let stderr_sink = host.stderr.clone();
    let outcome = drive(&mut host, events, updates, gaps, move |connection| {
        let work_dir = work_root.display().to_string();
        let sqlite_url = format!("sqlite://{}", sqlite_db.display());
        let csv_path = work_root.join("points.csv").display().to_string();
        let notes_path = work_root.join("notes.txt").display().to_string();
        Box::pin(async move {
            use agent_client_protocol::UntypedMessage;
            let digest =
                |family: &str, operation: &str| family_op_digest(family, operation);
            let decode = |value: serde_json::Value| -> Result<serde_json::Value, agent_client_protocol::Error> {
                decoded_facade_response(value).map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })
            };
            connection
                .send_request(initialize_request(Some(client_hello())))
                .block_task()
                .await?;
            let agent = connection
                .send_request(AgentCreateRequest {
                    config: AgentConfigWire::HostDefault,
                    idempotency_id: None,
                })
                .block_task()
                .await?
                .agent;
            let session = connection
                .send_request(SessionCreateRequest {
                    agent,
                    working_dir: Some(echo_sdk_protocol::scalar::WirePath::Utf8 {
                        path: work_dir.clone(),
                    }),
                    session_id: None,
                    idempotency_id: None,
                })
                .block_task()
                .await?;
            let tool_request =
                |method: &str,
                 operation: &str,
                 arguments: Vec<serde_json::Value>|
                 -> Result<UntypedMessage, agent_client_protocol::Error> {
                    let arguments = arguments
                        .into_iter()
                        .map(|value| {
                            echo_sdk_protocol::scalar::WireValue::from_json(value).map_err(
                                |error| {
                                    agent_client_protocol::Error::invalid_params()
                                        .data(error.to_string())
                                },
                            )
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    let family = method
                        .trim_start_matches("_echo_agent/")
                        .trim_end_matches("/op")
                        .replace('-', "_");
                    // Tool families accept the family-qualified spelling
                    // (`git.git_status`); the frozen digest is over the
                    // catalog's unqualified operation identity.
                    let canonical_operation = if [
                        "files", "web", "shell", "git", "database", "rag", "chart", "media",
                        "data", "statistics", "research",
                    ]
                    .contains(&family.as_str())
                    {
                        operation
                            .strip_prefix(&format!("{family}."))
                            .unwrap_or(operation)
                    } else {
                        operation
                    };
                    let request = echo_sdk_protocol::methods::FeatureOperationRequest {
                        operation: operation.to_string(),
                        signature_digest: digest(family.as_str(), canonical_operation),
                        handle: Some(session.session.clone()),
                        arguments,
                    };
                    serde_json::to_value(&request)
                        .map_err(|error| {
                            agent_client_protocol::Error::internal_error().data(error.to_string())
                        })
                        .and_then(|value| UntypedMessage::new(method, value))
                };

            // git: real `git status` over the freshly initialized repo; the
            // untracked files must show up through the framework tool.
            let status = decode(
                connection
                    .send_request(tool_request("_echo_agent/git/op", "git_status", vec![serde_json::json!(work_dir)])?)
                    .block_task()
                    .await?,
            )?;
            assert_eq!(status.get("success"), Some(&serde_json::json!(true)));
            assert!(
                status
                    .get("output")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|text| text.contains("points.csv")),
                "git_status must report the untracked fixture, got: {status}"
            );

            // database: read the sqlite fixture through the framework
            // SQL tool; a mutation statement is rejected by the tool's own
            // read-only policy and surfaces as a failure result (the wire
            // projects the tool's success/error contract verbatim).
            let counted = decode(
                connection
                    .send_request(tool_request(
                        "_echo_agent/database/op",
                        "sql_query",
                        vec![
                            serde_json::json!(sqlite_url),
                            serde_json::json!("SELECT COUNT(*) AS n FROM points"),
                        ],
                    )?)
                    .block_task()
                    .await?,
            )?;
            assert_eq!(counted.get("success"), Some(&serde_json::json!(true)));
            let rejected = decode(
                connection
                    .send_request(tool_request(
                        "_echo_agent/database/op",
                        "sql_query",
                        vec![
                            serde_json::json!(sqlite_url),
                            serde_json::json!("DELETE FROM points"),
                        ],
                    )?)
                    .block_task()
                    .await?,
            )?;
            assert_eq!(
                rejected.get("success"),
                Some(&serde_json::json!(false)),
                "non-SELECT SQL must not succeed"
            );
            assert!(
                rejected
                    .get("error")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|text| text.contains("read-only")),
                "rejected SQL must report the read-only policy, got: {rejected}"
            );

            // rag: chunk a document preview locally.
            let chunked = decode(
                connection
                    .send_request(tool_request(
                        "_echo_agent/rag/op",
                        "rag_chunk_document",
                        vec![
                            serde_json::json!("facade rag chunking paragraph. ".repeat(24)),
                            serde_json::json!(120),
                            serde_json::json!(20),
                        ],
                    )?)
                    .block_task()
                    .await?,
            )?;
            assert_eq!(chunked.get("success"), Some(&serde_json::json!(true)));

            // chart: a bar chart renders as a Vega-Lite spec, no rendering
            // engine needed.
            let chart = decode(
                connection
                    .send_request(tool_request(
                        "_echo_agent/chart/op",
                        "generate_chart",
                        // Positional arguments follow the tool schema order
                        // (chart_type, title, x_field, y_field, color_field,
                        // data); color_field stays null (optional).
                        vec![
                            serde_json::json!("bar"),
                            serde_json::json!("Facade chart"),
                            serde_json::json!("label"),
                            serde_json::json!("value"),
                            serde_json::Value::Null,
                            serde_json::json!([
                                {"label": "a", "value": 1},
                                {"label": "b", "value": 2}
                            ]),
                        ],
                    )?)
                    .block_task()
                    .await?,
            )?;
            assert_eq!(chart.get("success"), Some(&serde_json::json!(true)));

            // media: text statistics over a local text file.
            let stats = decode(
                connection
                    .send_request(tool_request(
                        "_echo_agent/media/op",
                        "text_stats",
                        vec![serde_json::json!(notes_path)],
                    )?)
                    .block_task()
                    .await?,
            )?;
            assert_eq!(stats.get("success"), Some(&serde_json::json!(true)));

            // data: read the CSV fixture with preview metadata.
            let read = decode(
                connection
                    .send_request(tool_request(
                        "_echo_agent/data/op",
                        "read_data",
                        vec![serde_json::json!(csv_path)],
                    )?)
                    .block_task()
                    .await?,
            )?;
            assert_eq!(read.get("success"), Some(&serde_json::json!(true)));
            let read_output: serde_json::Value = serde_json::from_str(
                read.get("output")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| {
                        agent_client_protocol::Error::internal_error()
                            .data("read_data output missing")
                    })?,
            )
            .map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            assert_eq!(read_output.get("rows"), Some(&serde_json::json!(4)));

            // statistics: exploratory descriptive stats refuse inference.
            let explored = decode(
                connection
                    .send_request(tool_request(
                        "_echo_agent/statistics/op",
                        "exploratory_statistics",
                        // The derive macro orders schema keys alphabetically
                        // (columns, data_path); columns stays null.
                        vec![serde_json::Value::Null, serde_json::json!(csv_path)],
                    )?)
                    .block_task()
                    .await?,
            )?;
            assert_eq!(explored.get("success"), Some(&serde_json::json!(true)));
            assert!(
                explored
                    .get("output")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|text| text.contains("\"inference\":false")),
                "exploratory statistics must disclaim inference, got: {explored}"
            );

            // research: BibTeX generation runs fully offline.
            let bibtex = decode(
                connection
                    .send_request(tool_request(
                        "_echo_agent/research/op",
                        "bibtex_generate",
                        vec![serde_json::json!([{
                            "title": "Facade Family Adapters",
                            "authors": ["Echo Agent"],
                            "year": 2026
                        }])],
                    )?)
                    .block_task()
                    .await?,
            )?;
            assert_eq!(bibtex.get("success"), Some(&serde_json::json!(true)));
            Ok(())
        })
    })
    .await;
    if let Err(error) = &outcome {
        return Err(format!(
            "{error}; host stderr: {}",
            String::from_utf8_lossy(&stderr_sink.lock().expect("stderr lock"))
        )
        .into());
    }
    outcome?;
    let _ = host.child.kill().await;
    Ok(())
}

// ── Subagent RPC success path (plan 07 todo 6) ──────────────────────────────

/// Drive helper variant whose client answers reverse extension calls:
/// `agent_execute` resolves immediately unless the hang flag is set (the
/// responder is parked silently, exercising control on a live attempt).
#[cfg(all(
    feature = "sdk-facade-adapters",
    feature = "sdk-extension-bridge",
    feature = "framework-subagent"
))]
async fn drive_answering<T, F>(
    host: &mut HostProcess,
    hang_agent_execute: Arc<Mutex<bool>>,
    scenario: F,
) -> Result<T, Box<dyn std::error::Error>>
where
    T: Send + 'static,
    F: FnOnce(
            ConnectionTo<agent_client_protocol::Agent>,
        ) -> BoxFuture<'static, agent_client_protocol::Result<T>>
        + Send
        + 'static,
{
    use agent_client_protocol::Responder;
    use echo_sdk_protocol::methods::{
        AgentStreamChunkWire, AgentStreamTerminalWire, ExtensionInvocation, ExtensionInvokeCall,
        ExtensionInvokeOutcome, ExtensionResult, ExtensionStreamChunkValue,
        ExtensionStreamCompleteValue, ExtensionStreamEvent, ExtensionUnit,
    };
    let _process_lock = acquire_e2e_process_lock();
    let transport = host_transport(&mut host.child);
    let connect = Client
        .builder()
        .on_receive_request(
            move |call: ExtensionInvokeCall,
                  responder: Responder<ExtensionInvokeOutcome>,
                  connection: ConnectionTo<agent_client_protocol::Agent>| {
                let hang = hang_agent_execute.clone();
                async move {
                    match call.invocation {
                        ExtensionInvocation::AgentExecute(_) => {
                            if *hang.lock().expect("hang flag") {
                                // Park the responder: the attempt stays live.
                                std::mem::forget(responder);
                                return Ok(());
                            }
                            responder.respond(ExtensionInvokeOutcome::Result {
                                result: ExtensionResult::AgentExecute(
                                    "SDK subagent executed".to_string(),
                                ),
                            })
                        }
                        ExtensionInvocation::AgentExecuteStream(_) => {
                            let Some(stream) = call.stream.clone() else {
                                return responder.respond(ExtensionInvokeOutcome::Error {
                                    error: echo_sdk_protocol::error::EchoSdkError::new(
                                        echo_sdk_protocol::error::ExtensionErrorCode::ExtensionFailed,
                                        "missing stream handle",
                                        echo_sdk_protocol::error::Retryability::Never,
                                    ),
                                });
                            };
                            responder.respond(ExtensionInvokeOutcome::Stream {
                                stream: stream.clone(),
                            })?;
                            tokio::spawn(async move {
                                let _ = connection.send_notification(ExtensionStreamEvent::Chunk {
                                    stream: stream.clone(),
                                    sequence: nonzero(1),
                                    value: ExtensionStreamChunkValue::Agent(
                                        AgentStreamChunkWire::Token {
                                            text: "SDK subagent executed".to_string(),
                                        },
                                    ),
                                });
                                let _ = connection.send_notification(ExtensionStreamEvent::Complete {
                                    stream,
                                    sequence: nonzero(2),
                                    value: ExtensionStreamCompleteValue::Agent(
                                        AgentStreamTerminalWire::FinalAnswer {
                                            text: "SDK subagent executed".to_string(),
                                        },
                                    ),
                                });
                            });
                            Ok(())
                        }
                        ExtensionInvocation::AgentClose(_) => responder.respond(
                            ExtensionInvokeOutcome::Result {
                                result: ExtensionResult::AgentClose(ExtensionUnit),
                            },
                        ),
                        other => Err(agent_client_protocol::Error::internal_error().data(
                            format!("unexpected reverse invocation: {:?}", other.operation()),
                        )),
                    }
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(transport, async move |connection| scenario(connection).await);
    let outcome = tokio::time::timeout(Duration::from_secs(60), connect)
        .await
        .map_err(|_| "client scenario timed out")??;
    Ok(outcome)
}

#[cfg(all(
    feature = "sdk-facade-adapters",
    feature = "sdk-extension-bridge",
    feature = "framework-subagent"
))]
#[tokio::test]
async fn subagent_family_dispatches_and_controls_live_attempts()
-> Result<(), Box<dyn std::error::Error>> {
    use echo_sdk_protocol::methods::{
        ExtensionDescriptor, ExtensionKind, ExtensionRegisterRequest, SubagentAwaitRequest,
        SubagentControlAction, SubagentControlRequest, SubagentDispatchRequest,
    };

    let (endpoint, _request_seen) = start_model_server("unused").await?;
    let work = tempfile::tempdir()?;
    let state_root = tempfile::tempdir()?;
    let config = write_config(work.path(), &endpoint, state_root.path(), None)?;
    let mut host = spawn_host(&config).await?;
    let hang = Arc::new(Mutex::new(false));
    let stderr_sink = host.stderr.clone();

    drive_answering(&mut host, hang.clone(), move |connection| {
        Box::pin(async move {
            connection
                .send_request(initialize_request(Some(client_hello())))
                .block_task()
                .await?;
            let agent = connection
                .send_request(AgentCreateRequest {
                    config: AgentConfigWire::HostDefault,
                    idempotency_id: None,
                })
                .block_task()
                .await?
                .agent;
            // A bridge-registered custom Agent becomes the dispatch target;
            // RPC, bridge and the delegation tool all share one registry.
            let _registered: echo_sdk_protocol::methods::ExtensionRegisterResponse = connection
                .send_request(ExtensionRegisterRequest {
                    kind: ExtensionKind::CustomAgent,
                    implementation_id: "sdk-subagent-target".to_string(),
                    descriptor: ExtensionDescriptor::CustomAgent {
                        descriptor_version: 1,
                        name: "sdk-subagent-target".to_string(),
                        model_name: "sdk-subagent-model".to_string(),
                        system_prompt: "SDK subagent".to_string(),
                        tool_names: Vec::new(),
                    },
                    timeout: None,
                })
                .block_task()
                .await?;
            let session = connection
                .send_request(SessionCreateRequest {
                    agent,
                    working_dir: None,
                    session_id: None,
                    idempotency_id: None,
                })
                .block_task()
                .await?;

            // Happy path: dispatch resolves through the reverse bridge and
            // awaits with the custom agent's own output.
            let first = connection
                .send_request(SubagentDispatchRequest {
                    session: session.session.clone(),
                    request: echo_sdk_protocol::scalar::WireValue::from_json(serde_json::json!({
                        "agent_name": "sdk-subagent-target",
                        "task": "compute the answer"
                    }))
                    .map_err(|error| {
                        agent_client_protocol::Error::invalid_params().data(error.to_string())
                    })?,
                    idempotency_id: None,
                })
                .block_task()
                .await?;
            let awaited: echo_sdk_protocol::methods::SubagentAwaitResponse = connection
                .send_request(SubagentAwaitRequest {
                    subagent: first.subagent.clone(),
                    timeout: None,
                })
                .block_task()
                .await?;
            assert!(
                awaited.settled,
                "custom agent dispatch must settle; host stderr: {}",
                String::from_utf8_lossy(&stderr_sink.lock().expect("stderr lock"))
            );
            let output = awaited
                .result
                .clone()
                .and_then(|value| value.into_json().ok())
                .ok_or_else(|| {
                    agent_client_protocol::Error::internal_error()
                        .data("await must return the subagent result")
                })?;
            assert!(
                output
                    .get("output")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|text| text.contains("SDK subagent executed")),
                "unexpected subagent output: {output}"
            );

            // Control path: park the next attempt (agent_execute hangs),
            // deliver a tracked message, then cancel and observe settlement.
            *hang.lock().expect("hang flag") = true;
            let second = connection
                .send_request(SubagentDispatchRequest {
                    session: session.session.clone(),
                    request: echo_sdk_protocol::scalar::WireValue::from_json(serde_json::json!({
                        "agent_name": "sdk-subagent-target",
                        "task": "long-running work"
                    }))
                    .map_err(|error| {
                        agent_client_protocol::Error::invalid_params().data(error.to_string())
                    })?,
                    idempotency_id: None,
                })
                .block_task()
                .await?;
            let messaged = connection
                .send_request(SubagentControlRequest {
                    subagent: second.subagent.clone(),
                    action: SubagentControlAction::Message,
                    payload: Some("checkpoint".to_string()),
                })
                .block_task()
                .await;
            // A bridge-backed custom agent cannot be live-steered; the
            // framework rejection surfaces either as a typed framework
            // error or as an unaccepted control — never as fake acceptance.
            match messaged {
                Ok(response) => assert!(
                    !response.accepted,
                    "live steering of a bridge-backed agent must not be accepted"
                ),
                Err(error) => {
                    let typed = typed_facade_error(&error).map_err(|error| {
                        agent_client_protocol::Error::internal_error().data(error.to_string())
                    })?;
                    assert_eq!(typed.code, ExtensionErrorCode::FrameworkError);
                }
            }
            let cancelled: echo_sdk_protocol::methods::SubagentControlResponse = connection
                .send_request(SubagentControlRequest {
                    subagent: second.subagent.clone(),
                    action: SubagentControlAction::Cancel,
                    payload: None,
                })
                .block_task()
                .await?;
            assert!(cancelled.accepted, "cancel must be accepted");
            let settled: echo_sdk_protocol::methods::SubagentAwaitResponse = connection
                .send_request(SubagentAwaitRequest {
                    subagent: second.subagent.clone(),
                    timeout: None,
                })
                .block_task()
                .await?;
            assert!(settled.settled, "cancelled attempt must settle");

            // Stale generation addresses fail closed with a typed error.
            let stale = echo_sdk_protocol::handle::WireHandle {
                id: "never-issued".to_string(),
                generation: first.subagent.generation.clone(),
                kind: echo_sdk_protocol::handle::HandleKind::Subagent,
            };
            let rejected = connection
                .send_request(SubagentAwaitRequest {
                    subagent: stale,
                    timeout: None,
                })
                .block_task()
                .await
                .expect_err("unknown subagent handle must fail");
            let typed = typed_facade_error(&rejected).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            assert_eq!(
                typed.code,
                echo_sdk_protocol::error::ExtensionErrorCode::InvalidValue
            );
            Ok(())
        })
    })
    .await?;
    let _ = host.child.kill().await;
    Ok(())
}

// ── Task graph RPC over the Session's own authority (plan 07 todo 3) ────────

#[cfg(feature = "sdk-facade-adapters")]
#[tokio::test]
async fn task_rpc_shares_the_session_task_authority() -> Result<(), Box<dyn std::error::Error>> {
    let (endpoint, _request_seen) = start_model_server("unused").await?;
    let work = tempfile::tempdir()?;
    let state_root = tempfile::tempdir()?;
    let config = write_config(work.path(), &endpoint, state_root.path(), None)?;
    let mut host = spawn_host(&config).await?;
    let (events, updates, gaps) = empty_collectors::<EventNotification>();

    drive(&mut host, events, updates, gaps, move |connection| {
        Box::pin(async move {
            use echo_sdk_protocol::handle::HandleKind;
            use echo_sdk_protocol::scalar::WireU64;
            connection
                .send_request(initialize_request(Some(client_hello())))
                .block_task()
                .await?;
            let agent = connection
                .send_request(AgentCreateRequest {
                    config: AgentConfigWire::HostDefault,
                    idempotency_id: None,
                })
                .block_task()
                .await?
                .agent;
            let session = connection
                .send_request(SessionCreateRequest {
                    agent,
                    working_dir: None,
                    session_id: None,
                    idempotency_id: None,
                })
                .block_task()
                .await?;

            // The TaskRun handle id is the session-scoped graph identity.
            let task_run = session.task_run.clone();
            let created = connection
                .send_request(TaskCreateRequest {
                    task_run: task_run.clone(),
                    spec: echo_sdk_protocol::scalar::WireValue::from_json(serde_json::json!({
                        "tasks": [
                            {"id": "plan", "title": "Plan", "description": "plan the work"},
                            {"id": "execute", "title": "Execute", "description": "do the work",
                             "depends_on": ["plan"]}
                        ]
                    }))
                    .map_err(|error| {
                        agent_client_protocol::Error::invalid_params().data(error.to_string())
                    })?,
                })
                .block_task()
                .await?;
            assert_eq!(created.tasks.len(), 2);
            assert_eq!(created.tasks[0].kind, HandleKind::PlanTask);
            assert_ne!(created.tasks[0].id, "plan");
            let revision_one = created.revision.to_u64().unwrap_or_default();
            assert!(revision_one >= 1);

            // List observes the same authority: both tasks pending at the
            // committed revision.
            let listed = connection
                .send_request(TaskListRequest {
                    task_run: task_run.clone(),
                })
                .block_task()
                .await?;
            assert_eq!(listed.tasks.len(), 2);
            assert!(listed.tasks.iter().all(|summary| {
                summary.status == echo_sdk_protocol::methods::WireTaskStatus::Pending
                    && summary.revision.to_u64() == Some(revision_one)
            }));

            // A revision-checked patch moves through the same CAS: updating
            // the title at the committed revision succeeds and advances it.
            let updated = connection
                .send_request(TaskUpdateRequest {
                    task_run: task_run.clone(),
                    patch: echo_sdk_protocol::scalar::WireValue::from_json(serde_json::json!({
                        "base_revision": revision_one,
                        "reason": "rpc rename",
                        "operations": [
                            {"op": "update", "task_id": "plan",
                             "patch": {"title": "Plan v2"}}
                        ]
                    }))
                    .map_err(|error| {
                        agent_client_protocol::Error::invalid_params().data(error.to_string())
                    })?,
                })
                .block_task()
                .await?;
            let revision_two = updated.revision.to_u64().unwrap_or_default();
            assert!(revision_two > revision_one);
            let listed = connection
                .send_request(TaskListRequest {
                    task_run: task_run.clone(),
                })
                .block_task()
                .await?;
            assert!(
                listed
                    .tasks
                    .iter()
                    .all(|summary| { summary.revision.to_u64() == Some(revision_two) })
            );

            // A stale writer is rejected by the framework CAS, not by the
            // Host duplicating revision rules.
            let stale = connection
                .send_request(TaskUpdateRequest {
                    task_run: task_run.clone(),
                    patch: echo_sdk_protocol::scalar::WireValue::from_json(serde_json::json!({
                        "base_revision": revision_one,
                        "reason": "stale writer",
                        "operations": [
                            {"op": "update", "task_id": "plan",
                             "patch": {"title": "stale"}}
                        ]
                    }))
                    .map_err(|error| {
                        agent_client_protocol::Error::invalid_params().data(error.to_string())
                    })?,
                })
                .block_task()
                .await
                .expect_err("stale revision must be rejected");
            let typed = typed_facade_error(&stale).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            assert_eq!(
                typed.code,
                echo_sdk_protocol::error::ExtensionErrorCode::FrameworkError
            );

            // A closed session fails the authority ladder, not the framework.
            connection
                .send_request(SessionCloseRequest {
                    session: session.session.clone(),
                })
                .block_task()
                .await?;
            let after_close = connection
                .send_request(TaskListRequest {
                    task_run: task_run.clone(),
                })
                .block_task()
                .await
                .expect_err("closed session must fail task rpc");
            assert!(matches!(
                after_close.code,
                agent_client_protocol::ErrorCode::Other(_)
            ));
            let _ = WireU64::from_u64(0);
            Ok(())
        })
    })
    .await?;
    let _ = host.child.kill().await;
    Ok(())
}

#[cfg(all(feature = "sdk-facade-adapters", feature = "framework-subagent"))]
#[tokio::test]
async fn task_execute_and_control_settle_through_the_runtime()
-> Result<(), Box<dyn std::error::Error>> {
    use echo_sdk_protocol::methods::{
        ControlAction, SubagentDispatchRequest, TaskControlRequest, TaskExecuteRequest,
    };
    let (endpoint, _request_seen) = start_model_server("unused").await?;
    let work = tempfile::tempdir()?;
    let state_root = tempfile::tempdir()?;
    let config = write_config(work.path(), &endpoint, state_root.path(), None)?;
    let mut host = spawn_host(&config).await?;
    let (events, updates, gaps) = empty_collectors::<EventNotification>();

    drive(&mut host, events, updates, gaps, move |connection| {
        Box::pin(async move {
            use echo_sdk_protocol::handle::HandleKind;
            use echo_sdk_protocol::methods::WireTaskStatus;
            connection
                .send_request(initialize_request(Some(client_hello())))
                .block_task()
                .await?;
            let agent = connection
                .send_request(AgentCreateRequest {
                    config: AgentConfigWire::HostDefault,
                    idempotency_id: None,
                })
                .block_task()
                .await?
                .agent;
            let session = connection
                .send_request(SessionCreateRequest {
                    agent,
                    working_dir: None,
                    session_id: None,
                    idempotency_id: None,
                })
                .block_task()
                .await?;
            let task_run = session.task_run.clone();
            // Two sequential tasks: the first dispatches to a missing
            // subagent (the runtime records the framework failure); the
            // dependent sibling never starts.
            let created = connection
                .send_request(TaskCreateRequest {
                    task_run: task_run.clone(),
                    spec: echo_sdk_protocol::scalar::WireValue::from_json(serde_json::json!({
                        "execution_mode": "sequential",
                        "tasks": [
                            {"id": "work", "title": "Work", "description": "do work",
                             "extension": {"subagent": "missing-subagent"}},
                            {"id": "later", "title": "Later", "description": "later work",
                             "depends_on": ["work"]}
                        ]
                    }))
                    .map_err(|error| {
                        agent_client_protocol::Error::invalid_params().data(error.to_string())
                    })?,
                })
                .block_task()
                .await?;
            assert_eq!(created.tasks.len(), 2);
            let work_handle = created.tasks.first().cloned().ok_or_else(|| {
                agent_client_protocol::Error::internal_error().data("missing work handle")
            })?;
            let later_handle = created.tasks.get(1).cloned().ok_or_else(|| {
                agent_client_protocol::Error::internal_error().data("missing later handle")
            })?;

            // Drive the graph through the RuntimeTaskService; the missing
            // subagent settles the dispatch as a framework failure.
            let started = connection
                .send_request(TaskExecuteRequest {
                    task_run: task_run.clone(),
                })
                .block_task()
                .await?;
            assert_eq!(started.run.kind, HandleKind::TaskRun);

            let mut settled = false;
            for _ in 0..100 {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                let listed = connection
                    .send_request(TaskListRequest {
                        task_run: task_run.clone(),
                    })
                    .block_task()
                    .await?;
                let work = listed
                    .tasks
                    .iter()
                    .find(|summary| summary.task == work_handle)
                    .cloned()
                    .ok_or_else(|| {
                        agent_client_protocol::Error::internal_error().data("work task missing")
                    })?;
                if matches!(work.status, WireTaskStatus::Failed { .. }) {
                    settled = true;
                    break;
                }
            }
            assert!(settled, "missing subagent must settle the task as failed");

            // Cancelling a task that never started has no live claim to
            // settle: the control reports not-accepted with the unchanged
            // status (claim-settling semantics, no fake transitions).
            let cancelled = connection
                .send_request(TaskControlRequest {
                    task_run: task_run.clone(),
                    task: later_handle.clone(),
                    action: ControlAction::Cancel,
                })
                .block_task()
                .await?;
            assert!(!cancelled.accepted);
            assert_eq!(cancelled.status, WireTaskStatus::Pending);

            // Subagent RPC over the shared control plane: an unknown
            // subagent fails fast through the executor's own registry.
            let dispatch = connection
                .send_request(SubagentDispatchRequest {
                    session: session.session.clone(),
                    request: echo_sdk_protocol::scalar::WireValue::from_json(serde_json::json!({
                        "agent_name": "ghost",
                        "task": "anything"
                    }))
                    .map_err(|error| {
                        agent_client_protocol::Error::invalid_params().data(error.to_string())
                    })?,
                    idempotency_id: None,
                })
                .block_task()
                .await
                .expect_err("unknown subagent must fail");
            let typed = typed_facade_error(&dispatch).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            assert_eq!(
                typed.code,
                echo_sdk_protocol::error::ExtensionErrorCode::FrameworkError
            );
            Ok(())
        })
    })
    .await?;
    let _ = host.child.kill().await;
    Ok(())
}
