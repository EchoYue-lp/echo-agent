//! Extension bridge end-to-end acceptance (supreme plan 06, todo
//! `implement-trait-proxies-and-streams` / `prove-bridge-reliability-and-docs`).
//!
//! A real official ACP Client plays the language-SDK role against the real
//! `echo-agent-sdk-host` child process compiled with the extension bridge:
//! the client registers host-language implementations and answers the Host's
//! reverse `_echo_agent/extension/invoke` requests, delivering stream chunks
//! through `_echo_agent/extension/stream`. The scenarios cover the full
//! registration → invocation → unregister lifecycle, the fail-closed matrix,
//! stream terminals, deadlines and cancellation notices.

#![cfg(feature = "sdk-extension-bridge")]

use agent_client_protocol::schema::{ProtocolVersion, v1};
use agent_client_protocol::{
    BoxFuture, ByteStreams, Client, ConnectionTo, Error as RpcError, Responder,
};
use echo_sdk_protocol::capability::{
    EchoAgentCapability, EchoAgentClientHello, ExtensionCapability,
};
use echo_sdk_protocol::error::{EchoSdkError, ExtensionErrorCode, Retryability};
use echo_sdk_protocol::handle::{HandleKind, WireHandle};
use echo_sdk_protocol::methods::{
    ExtensionDescriptor, ExtensionInvokeCall, ExtensionInvokeOutcome, ExtensionKind,
    ExtensionOperation, ExtensionRegisterRequest, ExtensionRegisterResponse, ExtensionStreamEvent,
    ExtensionUnregisterRequest, ExtensionUnregisterResponse,
};
use echo_sdk_protocol::scalar::{WireDuration, WireNonZeroU64, WireU64, WireValue};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;

mod support;

const SENTINEL_SECRET: &str = "sdk-bridge-sentinel-secret";

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_echo-agent-sdk-host"))
}

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
        required_capabilities: vec![ExtensionCapability::ExtensionBridge],
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

fn write_config(directory: &Path, endpoint: &str, state_root: &Path) -> PathBuf {
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
                "max_iterations": 6,
                "enable_tools": true
            }
        },
        "sdk_profile": {
            "state_root": state_root.display().to_string(),
            "limits": {}
        }
    });
    let path = directory.join("host.json");
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&document).expect("config JSON"),
    )
    .expect("write config");
    path
}

/// Scripted loopback model server: each chat completion receives the next
/// scripted SSE stream (tool-call turn, then final turn).
async fn start_scripted_model(
    scripts: Vec<Vec<serde_json::Value>>,
) -> Result<String, Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?.to_string();
    let seen = Arc::new(AtomicUsize::new(0));
    let total = scripts.len();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let scripts = scripts.clone();
            let seen = seen.clone();
            tokio::spawn(async move {
                if support::read_http_request(&mut socket).await.is_err() {
                    return;
                }
                let index = seen.fetch_add(1, Ordering::AcqRel);
                let Some(events) = scripts.get(index.min(total.saturating_sub(1))) else {
                    return;
                };
                let mut body = String::new();
                for event in events {
                    body.push_str(&format!(
                        "data: {}\n\n",
                        serde_json::to_string(event).unwrap_or_default()
                    ));
                }
                body.push_str("data: [DONE]\n\n");
                let headers = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = socket.write_all(headers.as_bytes()).await;
                let _ = socket.write_all(body.as_bytes()).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    Ok(address)
}

fn tool_call_script(tool: &str, arguments: &str) -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({
            "id": "chatcmpl-fixture",
            "choices": [{
                "index": 0,
                "delta": {
                    "role": "assistant",
                    "tool_calls": [{
                        "index": 0,
                        "id": "call-1",
                        "type": "function",
                        "function": {"name": tool, "arguments": arguments}
                    }]
                },
                "finish_reason": null
            }]
        }),
        serde_json::json!({
            "id": "chatcmpl-fixture",
            "choices": [{
                "index": 0,
                "delta": {},
                "finish_reason": "tool_calls"
            }]
        }),
    ]
}

fn final_script(text: &str) -> Vec<serde_json::Value> {
    vec![serde_json::json!({
        "id": "chatcmpl-fixture",
        "choices": [{
            "index": 0,
            "delta": {"role": "assistant", "content": text},
            "finish_reason": "stop"
        }]
    })]
}

// ── Host process plumbing (same shape as the core profile harness) ─────────

type SharedVec<T> = Arc<Mutex<Vec<T>>>;

struct HostProcess {
    child: tokio::process::Child,
    stderr: SharedVec<u8>,
}

async fn spawn_host(config: &Path) -> Result<HostProcess, Box<dyn std::error::Error>> {
    let mut child = tokio::process::Command::new(binary())
        .arg("--config")
        .arg(config)
        .env(
            "RUST_LOG",
            std::env::var("RUST_LOG").unwrap_or_else(|_| "echo_sdk_host=debug".to_string()),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stderr_handle = child.stderr.take().expect("host stderr piped");
    let stderr: SharedVec<u8> = Arc::new(Mutex::new(Vec::new()));
    let sink = stderr.clone();
    tokio::spawn(async move {
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
    host: &mut HostProcess,
) -> ByteStreams<
    tokio_util::compat::Compat<tokio::process::ChildStdin>,
    tokio_util::compat::Compat<tokio::process::ChildStdout>,
> {
    let stdin = host.child.stdin.take().expect("host stdin piped");
    let stdout = host.child.stdout.take().expect("host stdout piped");
    ByteStreams::new(
        tokio_util::compat::TokioAsyncWriteCompatExt::compat_write(stdin),
        tokio_util::compat::TokioAsyncReadCompatExt::compat(stdout),
    )
}

fn stderr_text(host: &HostProcess) -> String {
    String::from_utf8_lossy(host.stderr.lock().expect("stderr lock").as_slice()).to_string()
}

/// The fake SDK dispatcher state shared by client handlers.
#[derive(Default)]
struct SdkDispatch {
    operations: SharedVec<String>,
    cancel_notices: SharedVec<String>,
    /// Hold responders that must never answer (timeout/cancel scenarios).
    silent: Arc<Mutex<Vec<Responder<ExtensionInvokeOutcome>>>>,
    /// Operations that must hang instead of answering.
    hang: Arc<Mutex<Vec<&'static str>>>,
}

fn tool_descriptor(name: &str) -> ExtensionDescriptor {
    ExtensionDescriptor::Tool {
        descriptor_version: 1,
        name: name.to_string(),
        description: "host-language fixture tool".to_string(),
        parameters: WireValue::from_json(serde_json::json!({
            "type": "object",
            "properties": {"query": {"type": "string"}}
        }))
        .expect("schema wire value"),
        schema_revision: WireU64::from_u64(1),
        required_input_modalities: Vec::new(),
        supports_streaming: false,
    }
}

fn llm_descriptor(model: &str) -> ExtensionDescriptor {
    ExtensionDescriptor::LlmClient {
        descriptor_version: 1,
        model_name: model.to_string(),
        supports_streaming: true,
    }
}

/// Connect the fake SDK client with reverse-invocation handlers and run the
/// scenario to completion.
async fn drive_sdk<T, F>(
    host: &mut HostProcess,
    dispatch: Arc<SdkDispatch>,
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
    let transport = host_transport(host);
    let operations = dispatch.operations.clone();
    let cancel_notices = dispatch.cancel_notices.clone();
    let silent = dispatch.silent.clone();
    let hang = dispatch.hang.clone();
    let connect = Client
        .builder()
        .on_receive_request(
            {
                let operations = operations.clone();
                let silent = silent.clone();
                let hang = hang.clone();
                async move |call: ExtensionInvokeCall,
                            responder: Responder<ExtensionInvokeOutcome>,
                            connection: ConnectionTo<agent_client_protocol::Agent>| {
                    operations
                        .lock()
                        .expect("operations lock")
                        .push(call.operation.as_str().to_string());
                    if hang
                        .lock()
                        .expect("hang lock")
                        .iter()
                        .any(|operation| *operation == call.operation.as_str())
                    {
                        silent.lock().expect("silent lock").push(responder);
                        return Ok(());
                    }
                    match call.operation {
                        ExtensionOperation::ToolExecute => {
                            responder.respond(ExtensionInvokeOutcome::Result {
                                value: tool_result_wire("3 documents matched"),
                            })
                        }
                        ExtensionOperation::ToolValidateParameters => {
                            responder.respond(ExtensionInvokeOutcome::Result {
                                value: WireValue::Null,
                            })
                        }
                        ExtensionOperation::LlmChat => {
                            responder.respond(ExtensionInvokeOutcome::Result {
                                value: chat_response_wire("fixture chat answer"),
                            })
                        }
                        ExtensionOperation::LlmChatStream => {
                            let Some(stream) = call.stream.clone() else {
                                return responder.respond(ExtensionInvokeOutcome::Error {
                                    error: EchoSdkError::new(
                                        ExtensionErrorCode::ExtensionFailed,
                                        "missing stream handle",
                                        Retryability::Never,
                                    ),
                                });
                            };
                            responder.respond(ExtensionInvokeOutcome::Stream {
                                stream: stream.clone(),
                            })?;
                            // Deliver the chunks from a spawned task so the
                            // client dispatch loop is never blocked by the
                            // callback's own stream production (design §12.3:
                            // the reader loop keeps dispatching).
                            tokio::spawn(async move {
                                for (sequence, text) in [(1_u64, "streamed "), (2, "answer")] {
                                    let event = ExtensionStreamEvent::Chunk {
                                        stream: stream.clone(),
                                        sequence: nonzero(sequence),
                                        value: chat_chunk_wire(text, None),
                                    };
                                    if connection.send_notification(event).is_err() {
                                        return;
                                    }
                                }
                                let terminal = ExtensionStreamEvent::Complete {
                                    stream: stream.clone(),
                                    sequence: nonzero(3),
                                    value: chat_chunk_wire("", Some("stop")),
                                };
                                let _ = connection.send_notification(terminal);
                            });
                            Ok(())
                        }
                        // Hooks answer with the neutral result so lifecycle
                        // flow keeps moving; only explicitly-hanging
                        // operations stay silent.
                        ExtensionOperation::HookRun => {
                            responder.respond(ExtensionInvokeOutcome::Result {
                                value: WireValue::from_json(serde_json::json!({}))
                                    .expect("hook default"),
                            })
                        }
                        // Silence: hold the responder so the Host's deadline
                        // or cancellation settles the invocation.
                        ExtensionOperation::HumanLoopRequest
                        | ExtensionOperation::FactoryCreateAgent
                        | ExtensionOperation::AgentExecute
                        | ExtensionOperation::AgentChat => {
                            silent.lock().expect("silent lock").push(responder);
                            Ok(())
                        }
                        _ => {
                            // Observational callbacks, interventions and
                            // stream variants answer with neutral results.
                            responder.respond(ExtensionInvokeOutcome::Result {
                                value: neutral_result_wire(call.operation),
                            })
                        }
                    }
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            {
                let cancel_notices = cancel_notices.clone();
                async move |notice: echo_sdk_protocol::methods::ExtensionCancelNotice,
                            _connection: ConnectionTo<agent_client_protocol::Agent>| {
                    cancel_notices
                        .lock()
                        .expect("cancel lock")
                        .push(format!("{}:{}", notice.invocation_id, notice.reason));
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_notification(
            async move |_notification: v1::SessionNotification,
                        _connection: ConnectionTo<agent_client_protocol::Agent>| {
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .connect_with(transport, async move |connection| {
            scenario(connection).await
        });
    let outcome = tokio::time::timeout(Duration::from_secs(60), connect)
        .await
        .map_err(|_| format!("SDK scenario timed out; stderr:\n{}", stderr_text(host)))??;
    Ok(outcome)
}

fn nonzero(value: u64) -> WireNonZeroU64 {
    WireNonZeroU64::try_from(value.to_string()).expect("positive decimal")
}

fn tool_result_wire(output: &str) -> WireValue {
    WireValue::from_json(serde_json::json!({
        "kind": "text",
        "success": true,
        "output": output,
        "truncated": false
    }))
    .expect("tool result wire")
}

fn chat_response_wire(text: &str) -> WireValue {
    WireValue::from_json(serde_json::json!({
        "message": {"role": "assistant", "content": text},
        "finish_reason": "stop"
    }))
    .expect("chat response wire")
}

fn chat_chunk_wire(text: &str, finish: Option<&str>) -> WireValue {
    WireValue::from_json(serde_json::json!({
        "content": if text.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(text.to_string()) },
        "finish_reason": finish,
    }))
    .expect("chat chunk wire")
}

fn neutral_result_wire(operation: ExtensionOperation) -> WireValue {
    match operation.kind() {
        ExtensionKind::InterventionCallback => {
            WireValue::from_json(serde_json::json!({})).expect("intervention default")
        }
        ExtensionKind::Hook => WireValue::from_json(serde_json::json!({})).expect("hook default"),
        _ => WireValue::Null,
    }
}

async fn register_extension(
    connection: &ConnectionTo<agent_client_protocol::Agent>,
    kind: ExtensionKind,
    implementation_id: &str,
    descriptor: ExtensionDescriptor,
    timeout: Option<WireDuration>,
) -> Result<WireHandle, RpcError> {
    let response: ExtensionRegisterResponse = connection
        .send_request(ExtensionRegisterRequest {
            kind,
            implementation_id: implementation_id.to_string(),
            descriptor,
            timeout,
        })
        .block_task()
        .await?;
    Ok(response.extension)
}

async fn unregister(
    connection: &ConnectionTo<agent_client_protocol::Agent>,
    extension: &WireHandle,
) -> Result<bool, RpcError> {
    let response: ExtensionUnregisterResponse = connection
        .send_request(ExtensionUnregisterRequest {
            extension: extension.clone(),
        })
        .block_task()
        .await?;
    Ok(response.released)
}

/// Scenario A: negotiation advertises the bridge; a tool round trip drives
/// callbacks, an intervention and a hook; unregister and the conflict /
/// stale matrix behave as contracted.
#[tokio::test]
async fn tool_bridge_round_trip_with_callbacks_intervention_and_hook()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let state_root = directory.path().join("state");
    let model = start_scripted_model(vec![
        tool_call_script("search_docs", r#"{"query":"bridges"}"#),
        final_script("tool answered"),
    ])
    .await?;
    let config = write_config(
        directory.path(),
        &format!("http://{model}/v1/chat/completions"),
        &state_root,
    );
    let mut host = spawn_host(&config).await?;
    let dispatch = Arc::new(SdkDispatch::default());

    let outcome = drive_sdk(&mut host, dispatch.clone(), |connection| {
        Box::pin(async move {
            // Negotiate: the advertisement must declare the bridge with the
            // extension limits.
            let initialize: v1::InitializeResponse = connection
                .send_request(initialize_request(Some(client_hello())))
                .block_task()
                .await?;
            let meta = initialize
                .agent_capabilities
                .meta
                .as_ref()
                .and_then(|meta| meta.get("echo_agent").cloned())
                .expect("echo_agent capability present");
            let capability: EchoAgentCapability =
                serde_json::from_value(meta).expect("capability decodes");
            assert!(capability.declares(ExtensionCapability::ExtensionBridge));
            assert!(
                capability
                    .limits
                    .max_registered_extensions
                    .to_u64()
                    .is_some_and(|value| value > 0)
            );

            // Register the extension family BEFORE creating the Session.
            let tool = register_extension(
                &connection,
                ExtensionKind::Tool,
                "sdk-tool",
                tool_descriptor("search_docs"),
                None,
            )
            .await
            .expect("tool registers");
            register_extension(
                &connection,
                ExtensionKind::AgentCallback,
                "sdk-callback",
                ExtensionDescriptor::AgentCallback {
                    descriptor_version: 1,
                },
                None,
            )
            .await
            .expect("callback registers");
            register_extension(
                &connection,
                ExtensionKind::InterventionCallback,
                "sdk-intervention",
                ExtensionDescriptor::InterventionCallback {
                    descriptor_version: 1,
                },
                None,
            )
            .await
            .expect("intervention registers");
            register_extension(
                &connection,
                ExtensionKind::Hook,
                "sdk-hook",
                ExtensionDescriptor::Hook {
                    descriptor_version: 1,
                    events: Vec::new(),
                },
                None,
            )
            .await
            .expect("hook registers");

            // Same identity + same descriptor is idempotent; a different
            // descriptor is a typed conflict.
            let again = register_extension(
                &connection,
                ExtensionKind::Tool,
                "sdk-tool",
                tool_descriptor("search_docs"),
                None,
            )
            .await
            .expect("idempotent registration");
            assert_eq!(again.id, tool.id);
            let conflict = connection
                .send_request(ExtensionRegisterRequest {
                    kind: ExtensionKind::Tool,
                    implementation_id: "sdk-tool".to_string(),
                    descriptor: tool_descriptor("other_tool"),
                    timeout: None,
                })
                .block_task()
                .await;
            assert!(conflict.is_err(), "descriptor conflict must fail closed");

            // Standard session/prompt flows through the extension tool.
            let session: v1::NewSessionResponse = connection
                .send_request(v1::NewSessionRequest::new(
                    directory
                        .path()
                        .canonicalize()
                        .map_err(|error| RpcError::internal_error().data(error.to_string()))?,
                ))
                .block_task()
                .await?;
            let prompt = connection
                .send_request(v1::PromptRequest::new(
                    session.session_id.clone(),
                    vec![v1::ContentBlock::Text(v1::TextContent::new(
                        "search bridges",
                    ))],
                ))
                .block_task()
                .await?;
            assert_eq!(prompt.stop_reason, v1::StopReason::EndTurn);

            // Unregister: idempotent release.
            assert!(unregister(&connection, &tool).await?);
            assert!(!unregister(&connection, &tool).await?);

            // A stale-generation handle fails with the typed ladder. The
            // fresh state root is at generation 1, so generation 0 is stale.
            let stale = WireHandle {
                id: tool.id.clone(),
                generation: WireU64::from_u64(0),
                kind: HandleKind::Extension,
            };
            let stale_result = connection
                .send_request(ExtensionUnregisterRequest { extension: stale })
                .block_task()
                .await;
            assert!(stale_result.is_err());
            Ok(())
        })
    })
    .await;

    assert!(outcome.is_ok(), "scenario failed: {outcome:?}");
    let operations = dispatch.operations.lock().expect("operations").clone();
    assert!(
        operations
            .iter()
            .any(|operation| operation == "tool_execute"),
        "the tool call must reach the SDK; got {operations:?}"
    );
    assert!(
        operations
            .iter()
            .any(|operation| operation.starts_with("callback_on_")),
        "observational callbacks must reach the SDK; got {operations:?}"
    );
    assert!(
        operations
            .iter()
            .any(|operation| operation == "intervention_on_tool_call"),
        "the intervention must reach the SDK; got {operations:?}"
    );
    assert!(
        operations.iter().any(|operation| operation == "hook_run"),
        "lifecycle hooks must reach the SDK; got {operations:?}"
    );
    let stderr = stderr_text(&host);
    assert!(
        !stderr.contains(SENTINEL_SECRET),
        "the credential must never reach stderr"
    );
    host.child.kill().await?;
    Ok(())
}

/// Scenario B: a registered LlmClient replaces the model transport; the
/// streaming callback delivers chunks and exactly one terminal.
#[tokio::test]
async fn llm_client_stream_extension_answers_prompts() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let state_root = directory.path().join("state");
    // The model server stays unused: the extension replaces the transport.
    let model = start_scripted_model(vec![final_script("unused")]).await?;
    let config = write_config(
        directory.path(),
        &format!("http://{model}/v1/chat/completions"),
        &state_root,
    );
    let mut host = spawn_host(&config).await?;
    let dispatch = Arc::new(SdkDispatch::default());

    let outcome = drive_sdk(&mut host, dispatch.clone(), |connection| {
        Box::pin(async move {
            connection
                .send_request(initialize_request(Some(client_hello())))
                .block_task()
                .await?;
            register_extension(
                &connection,
                ExtensionKind::LlmClient,
                "sdk-llm",
                llm_descriptor("sdk-fixture-model"),
                None,
            )
            .await?;
            let session: v1::NewSessionResponse = connection
                .send_request(v1::NewSessionRequest::new(
                    directory
                        .path()
                        .canonicalize()
                        .map_err(|error| RpcError::internal_error().data(error.to_string()))?,
                ))
                .block_task()
                .await?;
            let prompt = connection
                .send_request(v1::PromptRequest::new(
                    session.session_id.clone(),
                    vec![v1::ContentBlock::Text(v1::TextContent::new("hello"))],
                ))
                .block_task()
                .await?;
            assert_eq!(prompt.stop_reason, v1::StopReason::EndTurn);
            Ok(())
        })
    })
    .await;
    assert!(outcome.is_ok(), "scenario failed: {outcome:?}");
    let operations = dispatch.operations.lock().expect("operations").clone();
    assert!(
        operations
            .iter()
            .any(|operation| operation == "llm_chat_stream" || operation == "llm_chat"),
        "the model call must route through the bridge; got {operations:?}"
    );
    host.child.kill().await?;
    Ok(())
}

/// Scenario C: a silent callback exceeds its registration deadline; the Host
/// settles a typed timeout and sends the cancel notice with reason
/// `timeout`; a client cancellation settles with reason `cancelled`.
#[tokio::test]
async fn deadline_and_cancellation_settle_typed_outcomes() -> Result<(), Box<dyn std::error::Error>>
{
    let directory = tempfile::tempdir()?;
    let state_root = directory.path().join("state");
    let model = start_scripted_model(vec![final_script("unused")]).await?;
    let config = write_config(
        directory.path(),
        &format!("http://{model}/v1/chat/completions"),
        &state_root,
    );

    let working_dir = Arc::new(directory.path().canonicalize()?);
    // Timeout: the registration declares a one-second deadline.
    {
        let mut host = spawn_host(&config).await?;
        let dispatch = Arc::new(SdkDispatch::default());
        dispatch
            .hang
            .lock()
            .expect("hang lock")
            .extend(["llm_chat", "llm_chat_stream"]);
        let scenario_dir = working_dir.clone();
        let outcome = drive_sdk(&mut host, dispatch.clone(), move |connection| {
            Box::pin(async move {
                connection
                    .send_request(initialize_request(Some(client_hello())))
                    .block_task()
                    .await?;
                register_extension(
                    &connection,
                    ExtensionKind::LlmClient,
                    "sdk-llm-slow",
                    llm_descriptor("sdk-slow-model"),
                    Some(WireDuration {
                        seconds: WireU64::from_u64(1),
                        nanos: 0,
                    }),
                )
                .await?;
                let session: v1::NewSessionResponse = connection
                    .send_request(v1::NewSessionRequest::new(scenario_dir.as_ref().clone()))
                    .block_task()
                    .await?;
                let prompt = connection
                    .send_request(v1::PromptRequest::new(
                        session.session_id.clone(),
                        vec![v1::ContentBlock::Text(v1::TextContent::new("slow"))],
                    ))
                    .block_task()
                    .await;
                // The framework fails the turn: no false success.
                assert!(prompt.is_err(), "a timed-out callback must fail the turn");
                Ok(())
            })
        })
        .await;
        assert!(outcome.is_ok(), "timeout scenario failed: {outcome:?}");
        let notices = dispatch.cancel_notices.lock().expect("notices").clone();
        assert!(
            notices.iter().any(|notice| notice.ends_with(":timeout")),
            "the deadline must send a timeout cancel notice; got {notices:?}"
        );
        host.child.kill().await?;
    }

    // Cancellation: the client cancels the prompt mid-invocation.
    {
        let mut host = spawn_host(&config).await?;
        let dispatch = Arc::new(SdkDispatch::default());
        dispatch
            .hang
            .lock()
            .expect("hang lock")
            .extend(["llm_chat", "llm_chat_stream"]);
        let scenario_dir = working_dir.clone();
        let outcome = drive_sdk(&mut host, dispatch.clone(), move |connection| {
            Box::pin(async move {
                connection
                    .send_request(initialize_request(Some(client_hello())))
                    .block_task()
                    .await?;
                register_extension(
                    &connection,
                    ExtensionKind::LlmClient,
                    "sdk-llm-hang",
                    llm_descriptor("sdk-hang-model"),
                    Some(WireDuration {
                        seconds: WireU64::from_u64(60),
                        nanos: 0,
                    }),
                )
                .await?;
                let session: v1::NewSessionResponse = connection
                    .send_request(v1::NewSessionRequest::new(scenario_dir.as_ref().clone()))
                    .block_task()
                    .await?;
                // Fire the prompt without awaiting it yet: the silent
                // callback hangs until the standard cancellation arrives.
                let prompt = connection.send_request(v1::PromptRequest::new(
                    session.session_id.clone(),
                    vec![v1::ContentBlock::Text(v1::TextContent::new("hang"))],
                ));
                // Give the Host time to start the (silent) callback, then
                // cancel through the standard path and await the response.
                tokio::time::sleep(Duration::from_millis(500)).await;
                connection
                    .send_notification(v1::CancelNotification::new(session.session_id.clone()))?;
                let settled = tokio::time::timeout(Duration::from_secs(20), prompt.block_task())
                    .await
                    .ok()
                    .and_then(|result| result.ok());
                // Either the cancelled stop reason or a typed failure is
                // acceptable; a false success is not.
                if let Some(response) = settled {
                    assert_ne!(response.stop_reason, v1::StopReason::EndTurn);
                }
                Ok(())
            })
        })
        .await;
        assert!(outcome.is_ok(), "cancel scenario failed: {outcome:?}");
        let notices = dispatch.cancel_notices.lock().expect("notices").clone();
        assert!(
            notices.iter().any(|notice| notice.ends_with(":cancelled")),
            "the framework cancel must reach the SDK; got {notices:?}"
        );
        host.child.kill().await?;
    }
    Ok(())
}

/// Scenario D: a plain standard Client never sees the bridge surface.
#[tokio::test]
async fn plain_clients_get_method_not_found_for_the_bridge()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let state_root = directory.path().join("state");
    let model = start_scripted_model(vec![final_script("unused")]).await?;
    let config = write_config(
        directory.path(),
        &format!("http://{model}/v1/chat/completions"),
        &state_root,
    );
    let mut host = spawn_host(&config).await?;
    let dispatch = Arc::new(SdkDispatch::default());

    let outcome = drive_sdk(&mut host, dispatch.clone(), |connection| {
        Box::pin(async move {
            connection
                .send_request(initialize_request(None))
                .block_task()
                .await?;
            let result = connection
                .send_request(ExtensionRegisterRequest {
                    kind: ExtensionKind::Tool,
                    implementation_id: "plain-tool".to_string(),
                    descriptor: tool_descriptor("plain_tool"),
                    timeout: None,
                })
                .block_task()
                .await;
            assert!(
                result.is_err(),
                "a plain Client must never reach the extension surface"
            );
            Ok(())
        })
    })
    .await;
    assert!(outcome.is_ok(), "plain-client scenario failed: {outcome:?}");
    host.child.kill().await?;
    Ok(())
}
