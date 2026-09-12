//! Facade feature adapter end-to-end acceptance (plan 07, todo
//! `prove-full-facade-and-document-status`).
//!
//! Real official ACP Client against the real `echo-agent-sdk-host` child
//! process for the plan-07 facade contracts that are host-behavioral rather
//! than inventory-level (the per-family happy paths live in
//! `core_profile_e2e.rs` under the same feature):
//!
//! - facade resources are Host-issued, generation-fenced `WireHandle`s
//!   resolved through one unified authority, and the advertised
//!   `max_facade_resources` is a connection-wide global bound across
//!   families;
//! - a second `task/execute` of a live TaskRun is a typed conflict — never
//!   a silent takeover of the tracked execution;
//! - task control still cancels the live execution afterwards and the
//!   facade admission ladder keeps failing closed for unknown identities.

#![cfg(feature = "sdk-facade-adapters")]

use agent_client_protocol::schema::{ProtocolVersion, v1};
use agent_client_protocol::{BoxFuture, ByteStreams, Client, ConnectionTo};
use echo_sdk_protocol::capability::{EchoAgentClientHello, ExtensionCapability};
use echo_sdk_protocol::error::ExtensionErrorCode;
use echo_sdk_protocol::event::EventNotification;
use echo_sdk_protocol::methods::{
    AgentConfigWire, AgentCreateRequest, SessionCloseRequest, SessionCreateRequest,
};
#[cfg(feature = "sdk-extension-bridge")]
use echo_sdk_protocol::methods::{
    ControlAction, ExtensionDescriptor, ExtensionKind, ExtensionRegisterRequest,
    TaskControlRequest, TaskCreateRequest, TaskExecuteRequest,
};
use echo_sdk_protocol::scalar::WireValue;
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

#[cfg(feature = "framework-a2a")]
fn catalog_invoke_digest(operation: &str) -> Result<String, Box<dyn std::error::Error>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../contracts/sdk/facade-operation-catalog.json");
    let catalog: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    catalog
        .get("routes")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .find(|route| route.get("operation").and_then(serde_json::Value::as_str) == Some(operation))
        .and_then(|route| route.get("signature_digests"))
        .and_then(serde_json::Value::as_array)
        .and_then(|digests| digests.first())
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("missing facade digest for {operation}").into())
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

#[cfg(feature = "framework-a2a")]
async fn start_a2a_stream_server() -> Result<String, Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let _ = support::read_http_request(&mut socket).await;
                let body = concat!(
                    "data: {\"type\":\"artifact\",\"taskId\":\"task-1\",\"artifact\":{\"parts\":[{\"type\":\"text\",\"text\":\"one\"}]},\"final\":false}\n\n",
                    "data: {\"type\":\"artifact\",\"taskId\":\"task-1\",\"artifact\":{\"parts\":[{\"type\":\"text\",\"text\":\"two\"}],\"append\":true},\"final\":true}\n\n"
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
    Ok(format!("http://{address}/a2a"))
}

#[cfg(feature = "framework-a2a")]
async fn start_a2a_task_server()
-> Result<(String, Arc<Mutex<Vec<serde_json::Value>>>), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let requests = Arc::new(Mutex::new(Vec::new()));
    let captured = requests.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let request = support::read_http_request_bytes(&mut socket)
                .await
                .unwrap_or_default();
            let body = request
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .and_then(|position| position.checked_add(4))
                .and_then(|position| request.get(position..))
                .and_then(|body| serde_json::from_slice::<serde_json::Value>(body).ok())
                .unwrap_or(serde_json::Value::Null);
            let state =
                if body.get("method").and_then(serde_json::Value::as_str) == Some("tasks/cancel") {
                    "canceled"
                } else {
                    "completed"
                };
            captured
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(body);
            let body = serde_json::json!({
                "jsonrpc": "2.0",
                "id": "fixture",
                "result": {
                    "id": "task-1",
                    "status": {"state": state},
                    "history": [],
                    "artifacts": []
                }
            })
            .to_string();
            let headers = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = socket.write_all(headers.as_bytes()).await;
            let _ = socket.write_all(body.as_bytes()).await;
            let _ = socket.flush().await;
        }
    });
    Ok((format!("http://{address}/a2a"), requests))
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
    // Only the bridge-gated duplicate-execute scenario asserts on host
    // stderr; facade-only builds keep the sink for diagnostics.
    #[cfg_attr(not(feature = "sdk-extension-bridge"), allow(dead_code))]
    stderr: SharedVec<u8>,
}

async fn spawn_host(config: &Path) -> Result<HostProcess, Box<dyn std::error::Error>> {
    let mut child = tokio::process::Command::new(binary())
        .arg("--config")
        .arg(config)
        .env("NO_PROXY", "127.0.0.1,localhost")
        .env("no_proxy", "127.0.0.1,localhost")
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
            scenario(connection).await
        });
    let outcome = tokio::time::timeout(Duration::from_secs(60), connect)
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

#[cfg(feature = "sdk-extension-bridge")]
async fn wait_until(predicate: impl Fn() -> bool) -> Result<(), Box<dyn std::error::Error>> {
    tokio::time::timeout(Duration::from_secs(20), async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .map_err(|_| "condition never became true".into())
}

fn typed_facade_error(
    error: &agent_client_protocol::Error,
) -> Result<echo_sdk_protocol::error::EchoSdkError, Box<dyn std::error::Error>> {
    echo_sdk_protocol::error::EchoSdkError::from_jsonrpc_data(error.data.as_ref())
        .map_err(|message| -> Box<dyn std::error::Error> { message.into() })
}

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

/// Connect a Client that answers reverse extension invocations. While
/// `hang` is set, `agent/execute` responders are parked so the attempt
/// stays live; `agent_execute_seen` flips on every such invocation.
#[cfg(feature = "sdk-extension-bridge")]
async fn drive_answering<T, F>(
    host: &mut HostProcess,
    hang: Arc<Mutex<bool>>,
    agent_execute_seen: Arc<std::sync::atomic::AtomicBool>,
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
        ExtensionInvocation, ExtensionInvokeCall, ExtensionInvokeOutcome, ExtensionResult,
        ExtensionUnit,
    };
    let _process_lock = acquire_e2e_process_lock();
    let transport = host_transport(&mut host.child);
    let connect = Client
        .builder()
        .on_receive_request(
            move |call: ExtensionInvokeCall,
                  responder: Responder<ExtensionInvokeOutcome>,
                  _connection: ConnectionTo<agent_client_protocol::Agent>| {
                let hang = hang.clone();
                let seen = agent_execute_seen.clone();
                async move {
                    match call.invocation {
                        ExtensionInvocation::AgentExecute(_) => {
                            seen.store(true, std::sync::atomic::Ordering::SeqCst);
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
                            seen.store(true, std::sync::atomic::Ordering::SeqCst);
                            let Some(stream) = call.stream.clone() else {
                                return responder.respond(ExtensionInvokeOutcome::Error {
                                    error: echo_sdk_protocol::error::EchoSdkError::new(
                                        ExtensionErrorCode::ExtensionFailed,
                                        "missing stream handle",
                                        echo_sdk_protocol::error::Retryability::Never,
                                    ),
                                });
                            };
                            responder.respond(ExtensionInvokeOutcome::Stream {
                                stream: stream.clone(),
                            })?;
                            // While hanging, the stream stays open without a
                            // terminal: the attempt stays live.
                            if *hang.lock().expect("hang flag") {
                                return Ok(());
                            }
                            let connection_for_stream = _connection.clone();
                            tokio::spawn(async move {
                                use echo_sdk_protocol::methods::{
                                    AgentStreamChunkWire, AgentStreamTerminalWire,
                                    ExtensionStreamChunkValue, ExtensionStreamCompleteValue,
                                    ExtensionStreamEvent,
                                };
                                let _ = connection_for_stream.send_notification(
                                    ExtensionStreamEvent::Chunk {
                                        stream: stream.clone(),
                                        sequence: nonzero(1),
                                        value: ExtensionStreamChunkValue::Agent(
                                            AgentStreamChunkWire::Token {
                                                text: "SDK subagent executed".to_string(),
                                            },
                                        ),
                                    },
                                );
                                let _ = connection_for_stream.send_notification(
                                    ExtensionStreamEvent::Complete {
                                        stream,
                                        sequence: nonzero(2),
                                        value: ExtensionStreamCompleteValue::Agent(
                                            AgentStreamTerminalWire::FinalAnswer {
                                                text: "SDK subagent executed".to_string(),
                                            },
                                        ),
                                    },
                                );
                            });
                            Ok(())
                        }
                        ExtensionInvocation::AgentClose(_) => {
                            responder.respond(ExtensionInvokeOutcome::Result {
                                result: ExtensionResult::AgentClose(ExtensionUnit),
                            })
                        }
                        other => Err(agent_client_protocol::Error::internal_error().data(format!(
                            "unexpected reverse invocation: {:?}",
                            other.operation()
                        ))),
                    }
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(transport, async move |connection| {
            scenario(connection).await
        });
    let outcome = tokio::time::timeout(Duration::from_secs(60), connect)
        .await
        .map_err(|_| "client scenario timed out")??;
    Ok(outcome)
}

#[cfg(feature = "sdk-extension-bridge")]
fn nonzero(value: u64) -> echo_sdk_protocol::scalar::WireNonZeroU64 {
    assert!(value >= 1);
    echo_sdk_protocol::scalar::WireNonZeroU64::try_from(value.to_string())
        .expect("non-zero decimal parses")
}

fn family_request(
    family: &str,
    operation: &str,
    handle: echo_sdk_protocol::handle::WireHandle,
    arguments: Vec<serde_json::Value>,
) -> Result<serde_json::Value, agent_client_protocol::Error> {
    let arguments = arguments
        .into_iter()
        .map(|value| {
            WireValue::from_json(value).map_err(|error| {
                agent_client_protocol::Error::invalid_params().data(error.to_string())
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let request = echo_sdk_protocol::methods::FeatureOperationRequest {
        operation: operation.to_string(),
        signature_digest: echo_sdk_protocol::facade::family_operation_signature_digest(
            family, operation,
        ),
        handle: Some(handle),
        arguments,
    };
    serde_json::to_value(&request)
        .map_err(|error| agent_client_protocol::Error::internal_error().data(error.to_string()))
}

fn family_wire_request(
    family: &str,
    operation: &str,
    handle: echo_sdk_protocol::handle::WireHandle,
    arguments: Vec<WireValue>,
) -> Result<serde_json::Value, agent_client_protocol::Error> {
    let request = echo_sdk_protocol::methods::FeatureOperationRequest {
        operation: operation.to_string(),
        signature_digest: echo_sdk_protocol::facade::family_operation_signature_digest(
            family, operation,
        ),
        handle: Some(handle),
        arguments,
    };
    serde_json::to_value(&request)
        .map_err(|error| agent_client_protocol::Error::internal_error().data(error.to_string()))
}

fn decoded_facade_wire(value: serde_json::Value) -> Result<WireValue, Box<dyn std::error::Error>> {
    let response: echo_sdk_protocol::methods::FeatureOperationResponse =
        serde_json::from_value(value)?;
    Ok(response.value)
}

fn stream_event_identity(value: WireValue) -> Result<(String, u64), Box<dyn std::error::Error>> {
    let WireValue::Variant {
        type_id,
        variant,
        fields,
    } = value
    else {
        return Err("facade stream result is not a Variant".into());
    };
    if type_id != "echo_sdk::FacadeStreamEvent" {
        return Err(format!("unexpected stream event type: {type_id}").into());
    }
    let sequence = fields
        .iter()
        .find(|field| field.name == "sequence")
        .and_then(|field| match &field.value {
            WireValue::U64(value) => value.to_u64(),
            _ => None,
        })
        .ok_or("stream event has no sequence")?;
    Ok((variant, sequence))
}

/// The advertised `max_facade_resources` bound is a connection-wide global
/// bound enforced by the unified facade resource authority: exceeding it is
/// a typed failure, and the failure names the advertised number — no
/// private per-family magic constants.
#[tokio::test]
async fn facade_resource_limits_match_the_advertised_bound()
-> Result<(), Box<dyn std::error::Error>> {
    use agent_client_protocol::UntypedMessage;

    let (endpoint, _request_seen) = start_model_server("unused").await?;
    let work = tempfile::tempdir()?;
    let state_root = tempfile::tempdir()?;
    let config = write_config(
        work.path(),
        &endpoint,
        state_root.path(),
        Some(serde_json::json!({"max_facade_resources": 2})),
    )?;
    let mut host = spawn_host(&config).await?;
    let (events, updates, gaps) = empty_collectors::<EventNotification>();

    drive(&mut host, events, updates, gaps, move |connection| {
        Box::pin(async move {
            let initialized = connection
                .send_request(initialize_request(Some(client_hello())))
                .block_task()
                .await?;
            // The advertisement carries the configured resource bound.
            let advertisement: echo_sdk_protocol::capability::EchoAgentCapability =
                serde_json::from_value(
                    initialized
                        .agent_capabilities
                        .meta
                        .as_ref()
                        .and_then(|meta| meta.get("echo_agent"))
                        .cloned()
                        .ok_or_else(|| {
                            agent_client_protocol::Error::internal_error().data("no advertisement")
                        })?,
                )
                .map_err(|error| {
                    agent_client_protocol::Error::invalid_params().data(error.to_string())
                })?;
            assert_eq!(
                advertisement
                    .limits
                    .max_facade_resources
                    .to_u64()
                    .unwrap_or_default(),
                2
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
                    agent,
                    working_dir: None,
                    session_id: None,
                    idempotency_id: None,
                })
                .block_task()
                .await?;

            // Two workflow graphs fit; each open returns a Host-issued,
            // generation-fenced facade resource handle.
            let definition = serde_json::json!({
                "name": "limit_flow",
                "nodes": [{"name": "end", "type": "router"}],
                "edges": [],
                "entry": "end",
                "finish": ["end"]
            })
            .to_string();
            for _ in 0..2 {
                let built = connection
                    .send_request(UntypedMessage::new(
                        "_echo_agent/workflow/op",
                        family_request(
                            "workflow",
                            "workflow.graph.build",
                            session.session.clone(),
                            vec![serde_json::json!(definition)],
                        )?,
                    )?)
                    .block_task()
                    .await?;
                let decoded = decoded_facade_response(built).map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })?;
                let resource: echo_sdk_protocol::handle::WireHandle =
                    serde_json::from_value(decoded.get("resource").cloned().ok_or_else(|| {
                        agent_client_protocol::Error::internal_error().data("no resource")
                    })?)
                    .map_err(|error| {
                        agent_client_protocol::Error::internal_error().data(error.to_string())
                    })?;
                assert_eq!(
                    resource.kind,
                    echo_sdk_protocol::handle::HandleKind::FacadeResource
                );
            }
            // The third open — a DIFFERENT family — exceeds the same global
            // advertised bound: the limit is connection-wide, not per
            // family.
            let rejected = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/delivery/op",
                    family_request(
                        "delivery",
                        "delivery.ledger.open",
                        session.session.clone(),
                        Vec::new(),
                    )?,
                )?)
                .block_task()
                .await
                .expect_err("third facade resource must exceed the global bound");
            let typed = typed_facade_error(&rejected).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            assert_eq!(typed.code, ExtensionErrorCode::PayloadTooLarge);
            assert!(
                typed.message.contains("resource limit 2"),
                "unexpected failure message: {}",
                typed.message
            );
            Ok(())
        })
    })
    .await?;
    let _ = host.child.kill().await;
    Ok(())
}

#[tokio::test]
async fn workflow_stream_uses_unified_handle_backpressure_and_close()
-> Result<(), Box<dyn std::error::Error>> {
    use agent_client_protocol::UntypedMessage;
    use echo_sdk_protocol::handle::{HandleKind, WireHandle};

    let (endpoint, _request_seen) = start_model_server("unused").await?;
    let work = tempfile::tempdir()?;
    let state_root = tempfile::tempdir()?;
    let config = write_config(
        work.path(),
        &endpoint,
        state_root.path(),
        Some(serde_json::json!({"max_facade_streams": 1})),
    )?;
    let mut host = spawn_host(&config).await?;
    let (events, updates, gaps) = empty_collectors::<EventNotification>();

    drive(&mut host, events, updates, gaps, move |connection| {
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
            let first = connection
                .send_request(SessionCreateRequest {
                    agent: agent.clone(),
                    working_dir: None,
                    session_id: None,
                    idempotency_id: None,
                })
                .block_task()
                .await?;
            let second = connection
                .send_request(SessionCreateRequest {
                    agent: agent.clone(),
                    working_dir: None,
                    session_id: None,
                    idempotency_id: None,
                })
                .block_task()
                .await?;
            let definition = serde_json::json!({
                "name": "stream_flow",
                "nodes": [{"name": "end", "type": "router"}],
                "edges": [],
                "entry": "end",
                "finish": ["end"]
            })
            .to_string();
            let built = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/workflow/op",
                    family_request(
                        "workflow",
                        "workflow.graph.build",
                        first.session.clone(),
                        vec![serde_json::json!(definition)],
                    )?,
                )?)
                .block_task()
                .await?;
            let built = decoded_facade_response(built).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            let graph: WireHandle =
                serde_json::from_value(built.get("resource").cloned().ok_or_else(|| {
                    agent_client_protocol::Error::internal_error().data("no graph")
                })?)
                .map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })?;
            let open = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/workflow/op",
                    family_request(
                        "workflow",
                        "workflow.graph.run_stream",
                        first.session.clone(),
                        vec![serde_json::json!(graph), serde_json::json!({})],
                    )?,
                )?)
                .block_task()
                .await?;
            let WireValue::Handle(stream) = decoded_facade_wire(open).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?
            else {
                return Err(agent_client_protocol::Error::internal_error()
                    .data("workflow stream open did not return a Stream handle"));
            };
            assert_eq!(stream.kind, HandleKind::Stream);

            #[cfg(feature = "framework-a2a")]
            {
                let cross_family = connection
                    .send_request(UntypedMessage::new(
                        "_echo_agent/a2a/op",
                        family_wire_request(
                            "a2a",
                            "a2a.stream.next",
                            first.session.clone(),
                            vec![WireValue::Handle(stream.clone())],
                        )?,
                    )?)
                    .block_task()
                    .await
                    .expect_err("an A2A control cannot consume a Workflow stream");
                assert_eq!(
                    typed_facade_error(&cross_family)
                        .map_err(|error| agent_client_protocol::Error::internal_error()
                            .data(error.to_string()))?
                        .code,
                    ExtensionErrorCode::InvalidValue
                );
            }

            let foreign = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/workflow/op",
                    family_wire_request(
                        "workflow",
                        "workflow.stream.next",
                        second.session,
                        vec![WireValue::Handle(stream.clone())],
                    )?,
                )?)
                .block_task()
                .await
                .expect_err("a facade stream cannot cross Session ownership");
            assert_eq!(
                typed_facade_error(&foreign)
                    .map_err(|error| agent_client_protocol::Error::internal_error()
                        .data(error.to_string()))?
                    .code,
                ExtensionErrorCode::InvalidValue
            );

            let overflow = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/workflow/op",
                    family_request(
                        "workflow",
                        "workflow.graph.run_stream",
                        first.session.clone(),
                        vec![serde_json::json!(graph), serde_json::json!({})],
                    )?,
                )?)
                .block_task()
                .await
                .expect_err("the advertised stream bound must reject a second live stream");
            assert_eq!(
                typed_facade_error(&overflow)
                    .map_err(|error| agent_client_protocol::Error::internal_error()
                        .data(error.to_string()))?
                    .code,
                ExtensionErrorCode::PayloadTooLarge
            );

            let first_next = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/workflow/op",
                    family_wire_request(
                        "workflow",
                        "workflow.stream.next",
                        first.session.clone(),
                        vec![WireValue::Handle(stream.clone())],
                    )?,
                )?)
                .block_task();
            let second_next = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/workflow/op",
                    family_wire_request(
                        "workflow",
                        "workflow.stream.next",
                        first.session.clone(),
                        vec![WireValue::Handle(stream.clone())],
                    )?,
                )?)
                .block_task();
            let (first_event, second_event) = tokio::join!(first_next, second_next);
            let first_event = decoded_facade_wire(first_event?).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            let second_event = decoded_facade_wire(second_event?).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            let mut identities = [
                stream_event_identity(first_event).map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })?,
                stream_event_identity(second_event).map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })?,
            ];
            identities.sort_by_key(|(_, sequence)| *sequence);
            assert_eq!(
                identities,
                [("item".to_string(), 1), ("item".to_string(), 2)]
            );

            let mut variants = Vec::new();
            for _ in 0..2 {
                let event = connection
                    .send_request(UntypedMessage::new(
                        "_echo_agent/workflow/op",
                        family_wire_request(
                            "workflow",
                            "workflow.stream.next",
                            first.session.clone(),
                            vec![WireValue::Handle(stream.clone())],
                        )?,
                    )?)
                    .block_task()
                    .await?;
                let event = decoded_facade_wire(event).map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })?;
                variants.push(stream_event_identity(event).map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })?);
            }
            assert_eq!(
                variants,
                vec![("item".to_string(), 3), ("complete".to_string(), 4)],
                "one-node workflow emits start/end/completed then stream completion"
            );
            let closed = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/workflow/op",
                    family_wire_request(
                        "workflow",
                        "workflow.stream.close",
                        first.session.clone(),
                        vec![WireValue::Handle(stream)],
                    )?,
                )?)
                .block_task()
                .await?;
            assert_eq!(
                decoded_facade_wire(closed).map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })?,
                WireValue::Bool(false)
            );

            let cancellable = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/workflow/op",
                    family_request(
                        "workflow",
                        "workflow.graph.run_stream",
                        first.session.clone(),
                        vec![serde_json::json!(graph), serde_json::json!({})],
                    )?,
                )?)
                .block_task()
                .await?;
            let WireValue::Handle(cancellable) =
                decoded_facade_wire(cancellable).map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })?
            else {
                return Err(agent_client_protocol::Error::internal_error()
                    .data("cancellable workflow stream did not return a Stream handle"));
            };
            for expected in [true, false] {
                let cancelled = connection
                    .send_request(UntypedMessage::new(
                        "_echo_agent/workflow/op",
                        family_wire_request(
                            "workflow",
                            "workflow.stream.cancel",
                            first.session.clone(),
                            vec![WireValue::Handle(cancellable.clone())],
                        )?,
                    )?)
                    .block_task()
                    .await?;
                assert_eq!(
                    decoded_facade_wire(cancelled).map_err(|error| {
                        agent_client_protocol::Error::internal_error().data(error.to_string())
                    })?,
                    WireValue::Bool(expected)
                );
            }
            let after_cancel = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/workflow/op",
                    family_wire_request(
                        "workflow",
                        "workflow.stream.next",
                        first.session.clone(),
                        vec![WireValue::Handle(cancellable.clone())],
                    )?,
                )?)
                .block_task()
                .await?;
            assert_eq!(
                stream_event_identity(decoded_facade_wire(after_cancel).map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })?)
                .map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })?,
                ("cancelled".to_string(), 1)
            );
            let closed = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/workflow/op",
                    family_wire_request(
                        "workflow",
                        "workflow.stream.close",
                        first.session.clone(),
                        vec![WireValue::Handle(cancellable)],
                    )?,
                )?)
                .block_task()
                .await?;
            assert_eq!(
                decoded_facade_wire(closed).map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })?,
                WireValue::Bool(false)
            );

            let broken_definition = serde_json::json!({
                "name": "broken_stream_flow",
                "nodes": [{"name": "orphan", "type": "router"}],
                "edges": [],
                "entry": "orphan",
                "finish": []
            })
            .to_string();
            let broken = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/workflow/op",
                    family_request(
                        "workflow",
                        "workflow.graph.build",
                        first.session.clone(),
                        vec![serde_json::json!(broken_definition)],
                    )?,
                )?)
                .block_task()
                .await?;
            let broken = decoded_facade_response(broken).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            let broken: WireHandle =
                serde_json::from_value(broken.get("resource").cloned().ok_or_else(|| {
                    agent_client_protocol::Error::internal_error().data("no broken graph resource")
                })?)
                .map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })?;
            let failed_stream = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/workflow/op",
                    family_request(
                        "workflow",
                        "workflow.graph.run_stream",
                        first.session.clone(),
                        vec![serde_json::json!(broken), serde_json::json!({})],
                    )?,
                )?)
                .block_task()
                .await?;
            let WireValue::Handle(failed_stream) =
                decoded_facade_wire(failed_stream).map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })?
            else {
                return Err(agent_client_protocol::Error::internal_error()
                    .data("broken workflow did not return a Stream handle"));
            };
            let mut failed_variants = Vec::new();
            for _ in 0..3 {
                let event = connection
                    .send_request(UntypedMessage::new(
                        "_echo_agent/workflow/op",
                        family_wire_request(
                            "workflow",
                            "workflow.stream.next",
                            first.session.clone(),
                            vec![WireValue::Handle(failed_stream.clone())],
                        )?,
                    )?)
                    .block_task()
                    .await?;
                failed_variants.push(
                    stream_event_identity(decoded_facade_wire(event).map_err(|error| {
                        agent_client_protocol::Error::internal_error().data(error.to_string())
                    })?)
                    .map_err(|error| {
                        agent_client_protocol::Error::internal_error().data(error.to_string())
                    })?,
                );
            }
            assert_eq!(
                failed_variants,
                vec![
                    ("item".to_string(), 1),
                    ("item".to_string(), 2),
                    ("failed".to_string(), 3),
                ]
            );

            let teardown_stream = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/workflow/op",
                    family_request(
                        "workflow",
                        "workflow.graph.run_stream",
                        first.session.clone(),
                        vec![serde_json::json!(broken), serde_json::json!({})],
                    )?,
                )?)
                .block_task()
                .await?;
            let WireValue::Handle(teardown_stream) =
                decoded_facade_wire(teardown_stream).map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })?
            else {
                return Err(agent_client_protocol::Error::internal_error()
                    .data("teardown workflow did not return a Stream handle"));
            };
            tokio::time::sleep(Duration::from_millis(50)).await;
            tokio::time::timeout(
                Duration::from_secs(5),
                connection
                    .send_request(SessionCloseRequest {
                        session: first.session.clone(),
                    })
                    .block_task(),
            )
            .await
            .map_err(|_| {
                agent_client_protocol::Error::internal_error()
                    .data("Session close did not settle a blocked facade producer")
            })??;
            let third = connection
                .send_request(SessionCreateRequest {
                    agent,
                    working_dir: None,
                    session_id: None,
                    idempotency_id: None,
                })
                .block_task()
                .await?;
            let closed_stream = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/workflow/op",
                    family_wire_request(
                        "workflow",
                        "workflow.stream.next",
                        third.session.clone(),
                        vec![WireValue::Handle(teardown_stream)],
                    )?,
                )?)
                .block_task()
                .await
                .expect_err("Session close must tombstone its facade stream");
            let closed_stream = typed_facade_error(&closed_stream).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            assert!(matches!(
                closed_stream.code,
                ExtensionErrorCode::StaleHandle | ExtensionErrorCode::ClosedHandle
            ));
            let rebuilt = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/workflow/op",
                    family_request(
                        "workflow",
                        "workflow.graph.build",
                        third.session.clone(),
                        vec![serde_json::json!(definition)],
                    )?,
                )?)
                .block_task()
                .await?;
            let rebuilt = decoded_facade_response(rebuilt).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            let rebuilt: WireHandle =
                serde_json::from_value(rebuilt.get("resource").cloned().ok_or_else(|| {
                    agent_client_protocol::Error::internal_error().data("no graph")
                })?)
                .map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })?;
            let reopened = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/workflow/op",
                    family_request(
                        "workflow",
                        "workflow.graph.run_stream",
                        third.session.clone(),
                        vec![serde_json::json!(rebuilt), serde_json::json!({})],
                    )?,
                )?)
                .block_task()
                .await?;
            let WireValue::Handle(reopened) = decoded_facade_wire(reopened).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?
            else {
                return Err(agent_client_protocol::Error::internal_error()
                    .data("reopened workflow did not return a Stream handle"));
            };
            let _ = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/workflow/op",
                    family_wire_request(
                        "workflow",
                        "workflow.stream.close",
                        third.session,
                        vec![WireValue::Handle(reopened)],
                    )?,
                )?)
                .block_task()
                .await?;
            Ok(())
        })
    })
    .await?;
    let _ = host.child.kill().await;
    Ok(())
}

#[tokio::test]
async fn connection_teardown_settles_a_blocked_facade_stream()
-> Result<(), Box<dyn std::error::Error>> {
    use agent_client_protocol::UntypedMessage;
    use echo_sdk_protocol::handle::WireHandle;

    let (endpoint, _request_seen) = start_model_server("unused").await?;
    let work = tempfile::tempdir()?;
    let state_root = tempfile::tempdir()?;
    let config = write_config(
        work.path(),
        &endpoint,
        state_root.path(),
        Some(serde_json::json!({"max_facade_streams": 1})),
    )?;
    let mut host = spawn_host(&config).await?;
    let (events, updates, gaps) = empty_collectors::<EventNotification>();
    let outcome = drive(&mut host, events, updates, gaps, move |connection| {
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
                    agent,
                    working_dir: None,
                    session_id: None,
                    idempotency_id: None,
                })
                .block_task()
                .await?;
            let definition = serde_json::json!({
                "name": "disconnect_stream_flow",
                "nodes": [{"name": "orphan", "type": "router"}],
                "edges": [],
                "entry": "orphan",
                "finish": []
            })
            .to_string();
            let built = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/workflow/op",
                    family_request(
                        "workflow",
                        "workflow.graph.build",
                        session.session.clone(),
                        vec![serde_json::json!(definition)],
                    )?,
                )?)
                .block_task()
                .await?;
            let built = decoded_facade_response(built).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            let graph: WireHandle =
                serde_json::from_value(built.get("resource").cloned().ok_or_else(|| {
                    agent_client_protocol::Error::internal_error().data("no graph")
                })?)
                .map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })?;
            let _stream = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/workflow/op",
                    family_request(
                        "workflow",
                        "workflow.graph.run_stream",
                        session.session,
                        vec![serde_json::json!(graph), serde_json::json!({})],
                    )?,
                )?)
                .block_task()
                .await?;
            tokio::time::sleep(Duration::from_millis(100)).await;
            Ok(())
        })
    })
    .await;
    assert!(
        outcome.is_ok(),
        "disconnect stream scenario failed: {outcome:?}"
    );
    let status = tokio::time::timeout(Duration::from_secs(10), host.child.wait())
        .await
        .map_err(|error| {
            format!(
                "Host did not exit after facade stream owner disconnect: {error}; stderr:\n{}",
                String::from_utf8_lossy(
                    host.stderr
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .as_slice()
                )
            )
        })??;
    assert!(
        status.success(),
        "facade stream producer teardown must let a clean owner EOF exit successfully"
    );
    Ok(())
}

#[cfg(feature = "framework-a2a")]
#[tokio::test]
async fn a2a_sse_stream_delivers_items_then_terminal() -> Result<(), Box<dyn std::error::Error>> {
    use agent_client_protocol::UntypedMessage;
    use echo_sdk_protocol::handle::{HandleKind, WireHandle};

    let (endpoint, _request_seen) = start_model_server("unused").await?;
    let a2a_endpoint = start_a2a_stream_server().await?;
    let (a2a_task_endpoint, a2a_requests) = start_a2a_task_server().await?;
    let work = tempfile::tempdir()?;
    let state_root = tempfile::tempdir()?;
    let config = write_config(work.path(), &endpoint, state_root.path(), None)?;
    let mut host = spawn_host(&config).await?;
    let (events, updates, gaps) = empty_collectors::<EventNotification>();

    drive(&mut host, events, updates, gaps, move |connection| {
        let a2a_endpoint = a2a_endpoint.clone();
        let a2a_task_endpoint = a2a_task_endpoint.clone();
        let a2a_requests = a2a_requests.clone();
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
            let opened = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/a2a/op",
                    family_request(
                        "a2a",
                        "a2a.client.open",
                        session.session.clone(),
                        Vec::new(),
                    )?,
                )?)
                .block_task()
                .await?;
            let opened = decoded_facade_response(opened).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            let client: WireHandle =
                serde_json::from_value(opened.get("resource").cloned().ok_or_else(|| {
                    agent_client_protocol::Error::internal_error().data("no A2A client")
                })?)
                .map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })?;
            let source_operation = "echo_agent::a2a::client::A2AClient::send_task_with_session";
            let source_send = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/facade/invoke",
                    serde_json::to_value(echo_sdk_protocol::methods::FeatureOperationRequest {
                        operation: source_operation.to_string(),
                        signature_digest: catalog_invoke_digest(source_operation).map_err(
                            |error| {
                                agent_client_protocol::Error::internal_error()
                                    .data(error.to_string())
                            },
                        )?,
                        handle: Some(agent),
                        arguments: vec![
                            WireValue::Handle(session.session.clone()),
                            WireValue::Handle(client.clone()),
                            WireValue::String(a2a_task_endpoint.clone()),
                            WireValue::String("send with session".to_string()),
                            WireValue::String("remote-session".to_string()),
                        ],
                    })
                    .map_err(|error| {
                        agent_client_protocol::Error::internal_error().data(error.to_string())
                    })?,
                )?)
                .block_task()
                .await?;
            let source_send = decoded_facade_response(source_send).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            assert_eq!(source_send.get("id"), Some(&serde_json::json!("task-1")));
            let fetched = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/a2a/op",
                    family_request(
                        "a2a",
                        "a2a.task.get",
                        session.session.clone(),
                        vec![
                            serde_json::json!(client.clone()),
                            serde_json::json!(a2a_task_endpoint.clone()),
                            serde_json::json!("task-1"),
                        ],
                    )?,
                )?)
                .block_task()
                .await?;
            let fetched = decoded_facade_response(fetched).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            assert_eq!(
                fetched.pointer("/status/state"),
                Some(&serde_json::json!("completed"))
            );
            let cancelled = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/a2a/op",
                    family_request(
                        "a2a",
                        "a2a.task.cancel",
                        session.session.clone(),
                        vec![
                            serde_json::json!(client.clone()),
                            serde_json::json!(a2a_task_endpoint),
                            serde_json::json!("task-1"),
                        ],
                    )?,
                )?)
                .block_task()
                .await?;
            let cancelled = decoded_facade_response(cancelled).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            assert_eq!(
                cancelled.pointer("/status/state"),
                Some(&serde_json::json!("canceled"))
            );
            assert!(
                a2a_requests
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .iter()
                    .any(|request| {
                        request.pointer("/params/sessionId")
                            == Some(&serde_json::json!("remote-session"))
                    }),
                "source send_task_with_session must preserve sessionId"
            );
            let stream = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/a2a/op",
                    family_request(
                        "a2a",
                        "a2a.task.stream.open",
                        session.session.clone(),
                        vec![
                            serde_json::json!(client),
                            serde_json::json!(a2a_endpoint),
                            serde_json::json!("stream it"),
                            serde_json::Value::Null,
                        ],
                    )?,
                )?)
                .block_task()
                .await?;
            let WireValue::Handle(stream) = decoded_facade_wire(stream).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?
            else {
                return Err(agent_client_protocol::Error::internal_error()
                    .data("A2A stream open did not return a Stream handle"));
            };
            assert_eq!(stream.kind, HandleKind::Stream);
            let mut variants = Vec::new();
            for _ in 0..3 {
                let event = connection
                    .send_request(UntypedMessage::new(
                        "_echo_agent/a2a/op",
                        family_wire_request(
                            "a2a",
                            "a2a.stream.next",
                            session.session.clone(),
                            vec![WireValue::Handle(stream.clone())],
                        )?,
                    )?)
                    .block_task()
                    .await?;
                let event = decoded_facade_wire(event).map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })?;
                let WireValue::Variant { variant, .. } = event else {
                    return Err(agent_client_protocol::Error::internal_error()
                        .data("A2A stream next did not return an event variant"));
                };
                variants.push(variant);
            }
            assert_eq!(variants, vec!["item", "item", "complete"]);
            Ok(())
        })
    })
    .await?;
    let _ = host.child.kill().await;
    Ok(())
}

/// A second `task/execute` of a live TaskRun is a typed conflict, and task
/// control still cancels the live execution afterwards. The live state is
/// made deterministic by parking the reverse `agent/execute` invocation,
/// which requires the extension bridge compiled into the Host.
#[cfg(feature = "sdk-extension-bridge")]
#[tokio::test]
async fn duplicate_task_execute_is_a_typed_conflict() -> Result<(), Box<dyn std::error::Error>> {
    let (endpoint, _request_seen) = start_model_server("unused").await?;
    let work = tempfile::tempdir()?;
    let state_root = tempfile::tempdir()?;
    let config = write_config(work.path(), &endpoint, state_root.path(), None)?;
    let mut host = spawn_host(&config).await?;
    let hang = Arc::new(Mutex::new(true));
    let agent_execute_seen = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stderr = host.stderr.clone();

    drive_answering(
        &mut host,
        hang.clone(),
        agent_execute_seen.clone(),
        move |connection| {
            let agent_execute_seen = agent_execute_seen.clone();
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
                // A bridge-registered custom Agent is the dispatch target for
                // the task's `extension.subagent` name.
                let _registered: echo_sdk_protocol::methods::ExtensionRegisterResponse = connection
                    .send_request(ExtensionRegisterRequest {
                        kind: ExtensionKind::CustomAgent,
                        implementation_id: "sdk-task-target".to_string(),
                        descriptor: ExtensionDescriptor::CustomAgent {
                            descriptor_version: 1,
                            name: "sdk-task-target".to_string(),
                            model_name: "sdk-task-model".to_string(),
                            system_prompt: "SDK task subagent".to_string(),
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
                let created = connection
                    .send_request(TaskCreateRequest {
                        task_run: session.task_run.clone(),
                        spec: WireValue::from_json(serde_json::json!({
                            "tasks": [
                                {"id": "step", "title": "Step", "description": "run the step",
                                 "extension": {"subagent": "sdk-task-target"}}
                            ]
                        }))
                        .map_err(|error| {
                            agent_client_protocol::Error::invalid_params().data(error.to_string())
                        })?,
                    })
                    .block_task()
                    .await?;
                let plan_task = created.tasks[0].clone();

                // First execute: accepted, and the live claim reaches the
                // parked reverse agent invocation.
                let first = connection
                    .send_request(TaskExecuteRequest {
                        task_run: session.task_run.clone(),
                    })
                    .block_task()
                    .await?;
                assert_eq!(first.run, session.task_run);
                wait_until(|| agent_execute_seen.load(std::sync::atomic::Ordering::SeqCst))
                    .await
                    .map_err(|error| {
                        agent_client_protocol::Error::internal_error().data(format!(
                            "{error}; host stderr: {}",
                            String::from_utf8_lossy(&stderr.lock().expect("stderr lock"))
                        ))
                    })?;

                // Second execute of the same live run: typed conflict, never
                // a silent takeover of the tracked execution.
                let duplicate = connection
                    .send_request(TaskExecuteRequest {
                        task_run: session.task_run.clone(),
                    })
                    .block_task()
                    .await
                    .expect_err("second execute of a live run must fail");
                let typed = typed_facade_error(&duplicate).map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })?;
                assert_eq!(typed.code, ExtensionErrorCode::InvalidValue);
                assert!(
                    typed.message.contains("already executing"),
                    "unexpected conflict message: {}",
                    typed.message
                );

                // Control cancels the live execution; whether this control
                // or the cancellation-driven interruption settles the claim
                // first, the store must converge on the cancelled terminal.
                let _controlled: echo_sdk_protocol::methods::TaskControlResponse = connection
                    .send_request(TaskControlRequest {
                        task_run: session.task_run.clone(),
                        task: plan_task.clone(),
                        action: ControlAction::Cancel,
                    })
                    .block_task()
                    .await?;
                let mut settled_cancelled = false;
                for _ in 0..100 {
                    let listed: echo_sdk_protocol::methods::TaskListResponse = connection
                        .send_request(echo_sdk_protocol::methods::TaskListRequest {
                            task_run: session.task_run.clone(),
                        })
                        .block_task()
                        .await?;
                    if listed.tasks.iter().any(|summary| {
                        matches!(
                            summary.status,
                            echo_sdk_protocol::methods::WireTaskStatus::Cancelled
                        )
                    }) {
                        settled_cancelled = true;
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                assert!(settled_cancelled, "the live claim must settle as cancelled");
                Ok(())
            })
        },
    )
    .await?;
    let _ = host.child.kill().await;
    Ok(())
}
