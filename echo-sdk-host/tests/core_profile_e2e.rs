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
    AgentCloseRequest, AgentConfigWire, AgentCreateRequest, AgentDescribeRequest, ControlAction,
    RunGetRequest, RunInput, RunStartRequest, RunStatus, RunWaitRequest, SessionCloseRequest,
    SessionCreateRequest, SessionLoadRequest, SubagentDispatchRequest, TaskControlRequest,
    TaskCreateRequest, TaskExecuteRequest, TaskListRequest, TaskUpdateRequest,
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

// ── Facade admission ladder (plan 07 todo 2) ────────────────────────────────

#[cfg(feature = "sdk-facade-adapters")]
fn first_catalog_invoke_operation() -> Result<String, Box<dyn std::error::Error>> {
    let catalog_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../contracts/sdk/facade-operation-catalog.json");
    let catalog: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(catalog_path)?)?;
    let mut operations: Vec<String> = catalog
        .get("routes")
        .and_then(|routes| routes.as_array())
        .into_iter()
        .flatten()
        .filter(|route| route.get("surface").and_then(|v| v.as_str()) == Some("invoke"))
        .filter_map(|route| {
            route
                .get("operation")
                .and_then(|operation| operation.as_str())
                .map(str::to_string)
        })
        .collect();
    operations.sort();
    operations
        .first()
        .cloned()
        .ok_or_else(|| "catalog carries no invoke identities".into())
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
    let config = write_config(work.path(), &endpoint, state_root.path(), None)?;
    let mut host = spawn_host(&config).await?;
    let (events, updates, gaps) = empty_collectors::<EventNotification>();
    let known_operation = first_catalog_invoke_operation()?;

    drive(&mut host, events, updates, gaps, move |connection| {
        let known_operation = known_operation.clone();
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

            let digest = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
            // Unknown operation identities fail closed as invalid_value with
            // the typed facade detail carrying the rejected identity.
            let unknown = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/facade/invoke",
                    serde_json::json!({
                        "operation": "totally::unknown::operation",
                        "signature_digest": digest,
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

            // A canonical operation resolves through the embedded catalog but
            // no family dispatcher is compiled yet: typed feature_unavailable,
            // never a simulated result.
            let known = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/facade/invoke",
                    serde_json::json!({
                        "operation": known_operation,
                        "signature_digest": digest,
                        "arguments": [],
                    }),
                )?)
                .block_task()
                .await
                .expect_err("family dispatch is not compiled yet");
            let typed = typed_facade_error(&known).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
            assert_eq!(typed.code, ExtensionErrorCode::FeatureUnavailable);

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
                        signature_digest: digest.to_string(),
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
                        signature_digest: digest.to_string(),
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
                    agent,
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

            // A family method whose handler family did not compile stays the
            // official method-not-found even on a negotiated connection
            // (memory is compiled now; channels needs a host-language
            // handler factory and stays unbound by design).
            let family = connection
                .send_request(UntypedMessage::new(
                    "_echo_agent/channels/op",
                    serde_json::json!({
                        "operation": "channels.start",
                        "signature_digest": digest,
                        "arguments": [],
                    }),
                )?)
                .block_task()
                .await
                .expect_err("uncompiled family method must fail");
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
            use echo_sdk_protocol::error::ExtensionErrorCode;
            let digest =
                "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
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
                    agent,
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
                        signature_digest: digest.to_string(),
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
            let graph_id = built
                .get("graph_id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    agent_client_protocol::Error::internal_error().data("no graph_id")
                })?
                .to_string();
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

            // Approving the checkpoint resumes to completion through the
            // `yes` branch; the agent node's mock answer landed in state.
            let resumed = decode(
                connection
                    .send_request(workflow_op(
                        "workflow.graph.resume",
                        session.session.clone(),
                        vec![
                            serde_json::json!(graph_id),
                            serde_json::json!(checkpoint),
                            serde_json::json!("approve"),
                        ],
                    )?)
                    .block_task()
                    .await?,
            )?;
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
            let state_id = state_new
                .get("state_id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    agent_client_protocol::Error::internal_error().data("no state_id")
                })?
                .to_string();
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
                .get("graph_id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    agent_client_protocol::Error::internal_error().data("no graph_id")
                })?
                .to_string();
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
            use echo_sdk_protocol::error::ExtensionErrorCode;
            let digest =
                "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
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
                    agent,
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
                    let request = echo_sdk_protocol::methods::FeatureOperationRequest {
                        operation: operation.to_string(),
                        signature_digest: digest.to_string(),
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
            let ledger_id = ledger
                .get("ledger_id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    agent_client_protocol::Error::internal_error().data("no ledger id")
                })?
                .to_string();
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
            let store_id = store
                .get("store_id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    agent_client_protocol::Error::internal_error().data("no store id")
                })?
                .to_string();
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
            use echo_sdk_protocol::error::ExtensionErrorCode;
            let digest =
                "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
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
                    agent,
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
                    let request = echo_sdk_protocol::methods::FeatureOperationRequest {
                        operation: operation.to_string(),
                        signature_digest: digest.to_string(),
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
    let config = write_config(work.path(), &endpoint, state_root.path(), None)?;
    let mut host = spawn_host(&config).await?;
    let (events, updates, gaps) = empty_collectors::<EventNotification>();

    drive(&mut host, events, updates, gaps, move |connection| {
        Box::pin(async move {
            use agent_client_protocol::UntypedMessage;
            use echo_sdk_protocol::error::ExtensionErrorCode;
            let digest =
                "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
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
                    agent,
                    working_dir: Some(echo_sdk_protocol::scalar::WirePath::Utf8 {
                        path: work_dir,
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
                    let request = echo_sdk_protocol::methods::FeatureOperationRequest {
                        operation: operation.to_string(),
                        signature_digest: digest.to_string(),
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
            use echo_sdk_protocol::error::ExtensionErrorCode;
            let digest =
                "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
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
                    agent,
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
                    let request = echo_sdk_protocol::methods::FeatureOperationRequest {
                        operation: operation.to_string(),
                        signature_digest: digest.to_string(),
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
            let manager_id = manager
                .get("manager_id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    agent_client_protocol::Error::internal_error().data("no manager id")
                })?
                .to_string();
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
            let client_id = client
                .get("client_id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    agent_client_protocol::Error::internal_error().data("no client id")
                })?
                .to_string();
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
            let lsp_id = lsp
                .get("manager_id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    agent_client_protocol::Error::internal_error().data("no lsp manager id")
                })?
                .to_string();
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
            let tracker_id = tracker
                .get("tracker_id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    agent_client_protocol::Error::internal_error().data("no tracker id")
                })?
                .to_string();
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
                    signature_digest: digest.to_string(),
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
            let task_run = echo_sdk_protocol::handle::WireHandle {
                id: session.acp_session_id.clone(),
                generation: session.session.generation.clone(),
                kind: HandleKind::TaskRun,
            };
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
            assert_eq!(created.tasks[0].id, "plan");
            assert_eq!(created.tasks[0].kind, HandleKind::PlanTask);
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
            let task_run = echo_sdk_protocol::handle::WireHandle {
                id: session.acp_session_id.clone(),
                generation: session.session.generation.clone(),
                kind: HandleKind::TaskRun,
            };
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
                    .find(|summary| summary.task.id == "work")
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
                    task: echo_sdk_protocol::handle::WireHandle {
                        id: "later".to_string(),
                        generation: task_run.generation.clone(),
                        kind: HandleKind::PlanTask,
                    },
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
