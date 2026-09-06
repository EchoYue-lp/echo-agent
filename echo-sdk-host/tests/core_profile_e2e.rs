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

use agent_client_protocol::schema::{ProtocolVersion, v1};
use agent_client_protocol::{BoxFuture, ByteStreams, Client, ConnectionTo, LineDirection};
use echo_sdk_protocol::capability::{
    EchoAgentCapability, EchoAgentClientHello, ExtensionCapability,
};
use echo_sdk_protocol::event::{EventAck, EventAckNotification, EventNotification, ReplayRequest};
use echo_sdk_protocol::handle::HandleKind;
use echo_sdk_protocol::methods::{
    AgentCloseRequest, AgentConfigWire, AgentCreateRequest, AgentDescribeRequest, RunGetRequest,
    RunInput, RunStartRequest, RunStatus, RunWaitRequest, SessionCloseRequest,
    SessionCreateRequest, SessionLoadRequest,
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
                "enable_tools": true
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
                    agent,
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
