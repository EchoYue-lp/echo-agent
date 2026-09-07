//! Extension bridge end-to-end acceptance (supreme plan 06, todo
//! `implement-trait-proxies-and-streams` / `prove-bridge-reliability-and-docs`).
//!
//! A real official ACP Client plays the language-SDK role against the real
//! `echo-agent-sdk-host` child process compiled with the extension bridge:
//! the client registers host-language implementations and answers the Host's
//! reverse `_echo_agent/extension/invoke` requests, delivering stream chunks
//! through `_echo_agent/extension/stream`. The scenarios cover the full
//! registration → invocation → unregister lifecycle, the fail-closed matrix,
//! typed stream terminals, backpressure, deadlines, late responses,
//! cancellation notices and owner disconnect.

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
use echo_sdk_protocol::methods::*;
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
    write_config_with_features(directory, endpoint, state_root, false, false, false)
}

fn write_config_with_features(
    directory: &Path,
    endpoint: &str,
    state_root: &Path,
    enable_memory: bool,
    enable_human_in_loop: bool,
    enable_subagent: bool,
) -> PathBuf {
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
                "enable_tools": true,
                "enable_memory": enable_memory,
                "enable_human_in_loop": enable_human_in_loop,
                "enable_subagent": enable_subagent,
                "register_agent_dispatch_tool": enable_subagent,
                "memory_path": state_root.join("memory.json").display().to_string()
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

fn set_profile_limit(
    path: &Path,
    name: &str,
    value: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut document: serde_json::Value = serde_json::from_slice(&std::fs::read(path)?)?;
    let limits = document
        .pointer_mut("/sdk_profile/limits")
        .and_then(serde_json::Value::as_object_mut)
        .ok_or_else(|| std::io::Error::other("sdk_profile.limits must be an object"))?;
    limits.insert(name.to_string(), serde_json::json!(value));
    std::fs::write(path, serde_json::to_vec_pretty(&document)?)?;
    Ok(())
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
        // Fixture model servers are loopback; never route them through a
        // developer proxy (reqwest follows the system proxy otherwise).
        .env("NO_PROXY", "127.0.0.1,localhost")
        .env("no_proxy", "127.0.0.1,localhost")
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
    malformed_stream: Arc<std::sync::atomic::AtomicBool>,
    out_of_order_stream: Arc<std::sync::atomic::AtomicBool>,
    duplicate_terminal: Arc<std::sync::atomic::AtomicBool>,
    omit_stream_terminal: Arc<std::sync::atomic::AtomicBool>,
    oversized_stream: Arc<std::sync::atomic::AtomicBool>,
    flood_stream: Arc<std::sync::atomic::AtomicBool>,
    missing_finish_reason: Arc<std::sync::atomic::AtomicBool>,
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
        required_permissions: Vec::new(),
        risk_level: echo_sdk_protocol::methods::ToolRiskLevelWire::ReadOnly,
        supports_streaming: false,
        exempt_from_batch_timeout: false,
        allows_parallel_batch_execution: true,
        manages_own_timeout: false,
    }
}

fn llm_descriptor(model: &str) -> ExtensionDescriptor {
    llm_descriptor_with_streaming(model, true)
}

fn llm_descriptor_with_streaming(model: &str, supports_streaming: bool) -> ExtensionDescriptor {
    ExtensionDescriptor::LlmClient {
        descriptor_version: 1,
        model_name: model.to_string(),
        supports_streaming,
        capabilities: echo_sdk_protocol::methods::LlmCapabilitiesWire::default(),
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
    let malformed_stream = dispatch.malformed_stream.clone();
    let out_of_order_stream = dispatch.out_of_order_stream.clone();
    let duplicate_terminal = dispatch.duplicate_terminal.clone();
    let omit_stream_terminal = dispatch.omit_stream_terminal.clone();
    let oversized_stream = dispatch.oversized_stream.clone();
    let flood_stream = dispatch.flood_stream.clone();
    let missing_finish_reason = dispatch.missing_finish_reason.clone();
    let connect = Client
        .builder()
        .on_receive_request(
            {
                let operations = operations.clone();
                let silent = silent.clone();
                let hang = hang.clone();
                let malformed_stream = malformed_stream.clone();
                let out_of_order_stream = out_of_order_stream.clone();
                let duplicate_terminal = duplicate_terminal.clone();
                let omit_stream_terminal = omit_stream_terminal.clone();
                let oversized_stream = oversized_stream.clone();
                let flood_stream = flood_stream.clone();
                let missing_finish_reason = missing_finish_reason.clone();
                move |call: ExtensionInvokeCall,
                      responder: Responder<ExtensionInvokeOutcome>,
                      connection: ConnectionTo<agent_client_protocol::Agent>| {
                    let operations = operations.clone();
                    let silent = silent.clone();
                    let hang = hang.clone();
                    let malformed_stream = malformed_stream.clone();
                    let out_of_order_stream = out_of_order_stream.clone();
                    let duplicate_terminal = duplicate_terminal.clone();
                    let omit_stream_terminal = omit_stream_terminal.clone();
                    let oversized_stream = oversized_stream.clone();
                    let flood_stream = flood_stream.clone();
                    let missing_finish_reason = missing_finish_reason.clone();
                    async move {
                        let operation = call.invocation.operation();
                        operations
                            .lock()
                            .expect("operations lock")
                            .push(operation.as_str().to_string());
                        if hang
                            .lock()
                            .expect("hang lock")
                            .iter()
                            .any(|candidate| *candidate == operation.as_str())
                        {
                            silent.lock().expect("silent lock").push(responder);
                            return Ok(());
                        }
                        match call.invocation {
                            ExtensionInvocation::ToolExecute(_) => {
                                responder.respond(ExtensionInvokeOutcome::Result {
                                    result: ExtensionResult::ToolExecute(tool_result_wire(
                                        "3 documents matched",
                                    )),
                                })
                            }
                            ExtensionInvocation::ToolValidateParameters(_) => {
                                responder.respond(ExtensionInvokeOutcome::Result {
                                    result: ExtensionResult::ToolValidateParameters(None),
                                })
                            }
                            ExtensionInvocation::StorePut(_) => {
                                responder.respond(ExtensionInvokeOutcome::Result {
                                    result: ExtensionResult::StorePut(ExtensionUnit),
                                })
                            }
                            ExtensionInvocation::StoreGet(_) => {
                                responder.respond(ExtensionInvokeOutcome::Result {
                                    result: ExtensionResult::StoreGet(None),
                                })
                            }
                            ExtensionInvocation::StoreSearch(_)
                            | ExtensionInvocation::StoreSearchWith(_) => {
                                responder.respond(ExtensionInvokeOutcome::Result {
                                    result: if operation == ExtensionOperation::StoreSearch {
                                        ExtensionResult::StoreSearch(Vec::new())
                                    } else {
                                        ExtensionResult::StoreSearchWith(Vec::new())
                                    },
                                })
                            }
                            ExtensionInvocation::StoreDelete(_) => {
                                responder.respond(ExtensionInvokeOutcome::Result {
                                    result: ExtensionResult::StoreDelete(true),
                                })
                            }
                            ExtensionInvocation::StoreListNamespaces(_) => {
                                responder.respond(ExtensionInvokeOutcome::Result {
                                    result: ExtensionResult::StoreListNamespaces(vec![vec![
                                        "sdk".to_string(),
                                    ]]),
                                })
                            }
                            ExtensionInvocation::StoreList(_) => {
                                responder.respond(ExtensionInvokeOutcome::Result {
                                    result: ExtensionResult::StoreList(Vec::new()),
                                })
                            }
                            ExtensionInvocation::StorePruneExpired(_) => {
                                responder.respond(ExtensionInvokeOutcome::Result {
                                    result: ExtensionResult::StorePruneExpired(WireU64::from_u64(
                                        0,
                                    )),
                                })
                            }
                            ExtensionInvocation::StoreDedupByContent(_) => {
                                responder.respond(ExtensionInvokeOutcome::Result {
                                    result: ExtensionResult::StoreDedupByContent(
                                        WireU64::from_u64(0),
                                    ),
                                })
                            }
                            ExtensionInvocation::LlmChat(_) => {
                                let mut response = chat_response_wire("fixture chat answer");
                                if missing_finish_reason.load(Ordering::Acquire) {
                                    response.finish_reason = None;
                                }
                                responder.respond(ExtensionInvokeOutcome::Result {
                                    result: ExtensionResult::LlmChat(response),
                                })
                            }
                            ExtensionInvocation::LlmChatStream(_) => {
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
                                    if oversized_stream.load(Ordering::Acquire) {
                                        let _ = connection.send_notification(
                                            ExtensionStreamEvent::Chunk {
                                                stream,
                                                sequence: nonzero(1),
                                                value: ExtensionStreamChunkValue::Llm(
                                                    chat_stream_chunk_wire(&"x".repeat(300_000)),
                                                ),
                                            },
                                        );
                                        return;
                                    }
                                    if flood_stream.load(Ordering::Acquire) {
                                        let text = "x";
                                        for sequence in 1_u64..=5_000 {
                                            if connection
                                                .send_notification(ExtensionStreamEvent::Chunk {
                                                    stream: stream.clone(),
                                                    sequence: nonzero(sequence),
                                                    value: ExtensionStreamChunkValue::Llm(
                                                        chat_stream_chunk_wire(text),
                                                    ),
                                                })
                                                .is_err()
                                            {
                                                return;
                                            }
                                        }
                                        let _ = connection.send_notification(
                                            ExtensionStreamEvent::Complete {
                                                stream,
                                                sequence: nonzero(5_001),
                                                value: ExtensionStreamCompleteValue::Llm(
                                                    chat_stream_complete_wire("stop"),
                                                ),
                                            },
                                        );
                                        return;
                                    }
                                    for (sequence, text) in [(1_u64, "streamed "), (2, "answer")] {
                                        let sequence = if out_of_order_stream
                                            .load(Ordering::Acquire)
                                            && sequence == 2
                                        {
                                            1
                                        } else {
                                            sequence
                                        };
                                        let value = if malformed_stream.load(Ordering::Acquire) {
                                            ExtensionStreamChunkValue::Agent(
                                                AgentStreamChunkWire::Token {
                                                    text: text.to_string(),
                                                },
                                            )
                                        } else {
                                            ExtensionStreamChunkValue::Llm(chat_stream_chunk_wire(
                                                text,
                                            ))
                                        };
                                        let event = ExtensionStreamEvent::Chunk {
                                            stream: stream.clone(),
                                            sequence: nonzero(sequence),
                                            value,
                                        };
                                        if connection.send_notification(event).is_err() {
                                            return;
                                        }
                                    }
                                    if omit_stream_terminal.load(Ordering::Acquire) {
                                        return;
                                    }
                                    let value = if malformed_stream.load(Ordering::Acquire) {
                                        ExtensionStreamCompleteValue::Agent(
                                            AgentStreamTerminalWire::FinalAnswer {
                                                text: "terminal".to_string(),
                                            },
                                        )
                                    } else {
                                        ExtensionStreamCompleteValue::Llm(
                                            chat_stream_complete_wire("stop"),
                                        )
                                    };
                                    let terminal = ExtensionStreamEvent::Complete {
                                        stream: stream.clone(),
                                        sequence: nonzero(3),
                                        value,
                                    };
                                    let _ = connection.send_notification(terminal.clone());
                                    if duplicate_terminal.load(Ordering::Acquire) {
                                        let _ = connection.send_notification(terminal);
                                    }
                                });
                                Ok(())
                            }
                            // Hooks answer with the neutral result so lifecycle
                            // flow keeps moving; only explicitly-hanging
                            // operations stay silent.
                            ExtensionInvocation::HookRun(_) => {
                                responder.respond(ExtensionInvokeOutcome::Result {
                                    result: ExtensionResult::HookRun(HookResultWire::default()),
                                })
                            }
                            ExtensionInvocation::HumanLoopRequest(_) => {
                                responder.respond(ExtensionInvokeOutcome::Result {
                                    result: ExtensionResult::HumanLoopRequest(
                                        HumanLoopResponseWire::Text {
                                            text: "SDK human-loop answer".to_string(),
                                        },
                                    ),
                                })
                            }
                            ExtensionInvocation::FactoryCreateAgent(config) => {
                                responder.respond(ExtensionInvokeOutcome::Result {
                                    result: ExtensionResult::FactoryCreateAgent(
                                        CustomAgentDescriptorWire {
                                            name: config.name,
                                            model_name: "sdk-custom-model".to_string(),
                                            system_prompt: "SDK custom agent".to_string(),
                                            tool_names: Vec::new(),
                                        },
                                    ),
                                })
                            }
                            ExtensionInvocation::AgentExecute(_) => {
                                responder.respond(ExtensionInvokeOutcome::Result {
                                    result: ExtensionResult::AgentExecute(
                                        "SDK custom execute".to_string(),
                                    ),
                                })
                            }
                            ExtensionInvocation::AgentChat(_) => {
                                responder.respond(ExtensionInvokeOutcome::Result {
                                    result: ExtensionResult::AgentChat(
                                        "SDK custom chat".to_string(),
                                    ),
                                })
                            }
                            ExtensionInvocation::AgentClose(_) => {
                                responder.respond(ExtensionInvokeOutcome::Result {
                                    result: ExtensionResult::AgentClose(ExtensionUnit),
                                })
                            }
                            ExtensionInvocation::AgentExecuteStream(_)
                            | ExtensionInvocation::AgentChatStream(_) => {
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
                                tokio::spawn(async move {
                                    let _ =
                                        connection.send_notification(ExtensionStreamEvent::Chunk {
                                            stream: stream.clone(),
                                            sequence: nonzero(1),
                                            value: ExtensionStreamChunkValue::Agent(
                                                AgentStreamChunkWire::Token {
                                                    text: "SDK custom event".to_string(),
                                                },
                                            ),
                                        });
                                    let _ = connection.send_notification(
                                        ExtensionStreamEvent::Complete {
                                            stream,
                                            sequence: nonzero(2),
                                            value: ExtensionStreamCompleteValue::Agent(
                                                AgentStreamTerminalWire::FinalAnswer {
                                                    text: "SDK custom answer".to_string(),
                                                },
                                            ),
                                        },
                                    );
                                });
                                Ok(())
                            }
                            _ => {
                                // Observational callbacks, interventions and
                                // stream variants answer with neutral results.
                                responder.respond(ExtensionInvokeOutcome::Result {
                                    result: neutral_result_wire(operation),
                                })
                            }
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
        .map_err(|_| {
            let operations = operations
                .lock()
                .map(|value| value.clone())
                .unwrap_or_default();
            format!(
                "SDK scenario timed out; operations={operations:?}; stderr:\n{}",
                stderr_text(host)
            )
        })??;
    Ok(outcome)
}

fn nonzero(value: u64) -> WireNonZeroU64 {
    WireNonZeroU64::try_from(value.to_string()).expect("positive decimal")
}

fn tool_result_wire(output: &str) -> ToolResultWire {
    ToolResultWire {
        kind: ToolResultKindWire::Text,
        success: true,
        output: output.to_string(),
        error: None,
        failure: None,
        data: None,
        truncated: false,
        mime_type: None,
        artifact: None,
        metadata: std::collections::BTreeMap::new(),
        model_content: Vec::new(),
    }
}

fn chat_response_wire(text: &str) -> LlmChatResponseWire {
    LlmChatResponseWire {
        message: LlmMessageWire {
            role: "assistant".to_string(),
            content: WireValue::String(text.to_string()),
            tool_calls: None,
            name: None,
            tool_call_id: None,
            reasoning_content: None,
            reasoning_blocks: None,
        },
        finish_reason: Some("stop".to_string()),
        usage: None,
        raw: WireValue::from_json(serde_json::json!({})).expect("empty raw response"),
    }
}

fn chat_stream_chunk_wire(text: &str) -> LlmStreamChunkWire {
    LlmStreamChunkWire {
        content: (!text.is_empty()).then(|| text.to_string()),
        ..LlmStreamChunkWire::default()
    }
}

fn chat_stream_complete_wire(finish_reason: &str) -> LlmStreamCompleteWire {
    LlmStreamCompleteWire {
        role: None,
        content: None,
        reasoning_content: None,
        reasoning_blocks: None,
        tool_calls: None,
        finish_reason: finish_reason.to_string(),
        usage: None,
    }
}

fn neutral_result_wire(operation: ExtensionOperation) -> ExtensionResult {
    match operation {
        ExtensionOperation::StorePut => ExtensionResult::StorePut(ExtensionUnit),
        ExtensionOperation::StoreGet => ExtensionResult::StoreGet(None),
        ExtensionOperation::StoreSearch => ExtensionResult::StoreSearch(Vec::new()),
        ExtensionOperation::StoreSearchWith => ExtensionResult::StoreSearchWith(Vec::new()),
        ExtensionOperation::StoreDelete => ExtensionResult::StoreDelete(false),
        ExtensionOperation::StoreListNamespaces => ExtensionResult::StoreListNamespaces(Vec::new()),
        ExtensionOperation::StoreList => ExtensionResult::StoreList(Vec::new()),
        ExtensionOperation::StorePruneExpired => {
            ExtensionResult::StorePruneExpired(WireU64::from_u64(0))
        }
        ExtensionOperation::StoreDedupByContent => {
            ExtensionResult::StoreDedupByContent(WireU64::from_u64(0))
        }
        ExtensionOperation::CallbackOnThinkStart => {
            ExtensionResult::CallbackOnThinkStart(ExtensionUnit)
        }
        ExtensionOperation::CallbackOnThinkEnd => {
            ExtensionResult::CallbackOnThinkEnd(ExtensionUnit)
        }
        ExtensionOperation::CallbackOnToolStart => {
            ExtensionResult::CallbackOnToolStart(ExtensionUnit)
        }
        ExtensionOperation::CallbackOnToolEnd => ExtensionResult::CallbackOnToolEnd(ExtensionUnit),
        ExtensionOperation::CallbackOnToolError => {
            ExtensionResult::CallbackOnToolError(ExtensionUnit)
        }
        ExtensionOperation::CallbackOnFinalAnswer => {
            ExtensionResult::CallbackOnFinalAnswer(ExtensionUnit)
        }
        ExtensionOperation::CallbackOnIteration => {
            ExtensionResult::CallbackOnIteration(ExtensionUnit)
        }
        ExtensionOperation::InterventionOnToolCall => {
            ExtensionResult::InterventionOnToolCall(InterventionResultWire::default())
        }
        ExtensionOperation::InterventionOnThinkStart => {
            ExtensionResult::InterventionOnThinkStart(InterventionResultWire::default())
        }
        ExtensionOperation::InterventionOnFinalAnswer => {
            ExtensionResult::InterventionOnFinalAnswer(InterventionResultWire::default())
        }
        ExtensionOperation::AgentClose => ExtensionResult::AgentClose(ExtensionUnit),
        _ => ExtensionResult::CallbackOnIteration(ExtensionUnit),
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

    let outcome = drive_sdk(&mut host, dispatch.clone(), move |connection| {
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
            let callback = register_extension(
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
            let intervention = register_extension(
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
            let hook = register_extension(
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
            let semantic_conflict = connection
                .send_request(ExtensionRegisterRequest {
                    kind: ExtensionKind::Tool,
                    implementation_id: "sdk-tool-alias".to_string(),
                    descriptor: tool_descriptor("search_docs"),
                    timeout: None,
                })
                .block_task()
                .await;
            assert!(
                semantic_conflict.is_err(),
                "a second implementation cannot claim the same tool name"
            );

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
            assert!(unregister(&connection, &callback).await?);
            assert!(unregister(&connection, &intervention).await?);
            assert!(unregister(&connection, &hook).await?);

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

/// Scenario A2: memory and human-loop providers are real reverse extensions,
/// not aliases of the Tool or Intervention callback contracts.
#[tokio::test]
async fn store_and_human_loop_extensions_round_trip() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let state_root = directory.path().join("state");
    let model = start_scripted_model(vec![
        tool_call_script("remember", r#"{"content":"bridge memory"}"#),
        final_script("remembered"),
    ])
    .await?;
    let config = write_config_with_features(
        directory.path(),
        &format!("http://{model}/v1/chat/completions"),
        &state_root,
        true,
        false,
        false,
    );
    let mut host = spawn_host(&config).await?;
    let dispatch = Arc::new(SdkDispatch::default());
    let session_dir = directory.path().canonicalize()?;
    let outcome = drive_sdk(&mut host, dispatch.clone(), move |connection| {
        let session_dir = session_dir.clone();
        Box::pin(async move {
            connection
                .send_request(initialize_request(Some(client_hello())))
                .block_task()
                .await?;
            let store = register_extension(
                &connection,
                ExtensionKind::Store,
                "sdk-store",
                ExtensionDescriptor::Store {
                    descriptor_version: 1,
                    search_modes: vec![SearchModeWire::Keyword],
                },
                None,
            )
            .await?;
            let session: v1::NewSessionResponse = connection
                .send_request(v1::NewSessionRequest::new(session_dir))
                .block_task()
                .await?;
            let prompt = connection
                .send_request(v1::PromptRequest::new(
                    session.session_id,
                    vec![v1::ContentBlock::Text(v1::TextContent::new(
                        "remember this",
                    ))],
                ))
                .block_task()
                .await?;
            assert_eq!(prompt.stop_reason, v1::StopReason::EndTurn);
            assert!(unregister(&connection, &store).await?);
            Ok(())
        })
    })
    .await;
    assert!(
        outcome.is_ok(),
        "store scenario failed: {outcome:?}; stderr={}",
        stderr_text(&host)
    );
    assert!(
        dispatch
            .operations
            .lock()
            .expect("operations")
            .iter()
            .any(|operation| operation == "store_put")
    );
    host.child.kill().await?;

    let model = start_scripted_model(vec![
        tool_call_script(
            "human_in_loop",
            r#"{"reasoning":"need confirmation","approval_type":"LLM"}"#,
        ),
        final_script("confirmed"),
    ])
    .await?;
    let config = write_config_with_features(
        directory.path(),
        &format!("http://{model}/v1/chat/completions"),
        &state_root,
        false,
        true,
        false,
    );
    let mut host = spawn_host(&config).await?;
    let dispatch = Arc::new(SdkDispatch::default());
    let session_dir = directory.path().canonicalize()?;
    let outcome = drive_sdk(&mut host, dispatch.clone(), move |connection| {
        let session_dir = session_dir.clone();
        Box::pin(async move {
            connection
                .send_request(initialize_request(Some(client_hello())))
                .block_task()
                .await?;
            let human = register_extension(
                &connection,
                ExtensionKind::HumanLoopProvider,
                "sdk-human",
                ExtensionDescriptor::HumanLoopProvider {
                    descriptor_version: 1,
                },
                None,
            )
            .await?;
            let session: v1::NewSessionResponse = connection
                .send_request(v1::NewSessionRequest::new(session_dir))
                .block_task()
                .await?;
            let prompt = connection
                .send_request(v1::PromptRequest::new(
                    session.session_id,
                    vec![v1::ContentBlock::Text(v1::TextContent::new("ask"))],
                ))
                .block_task()
                .await?;
            assert_eq!(prompt.stop_reason, v1::StopReason::EndTurn);
            assert!(unregister(&connection, &human).await?);
            Ok(())
        })
    })
    .await;
    assert!(outcome.is_ok(), "human-loop scenario failed: {outcome:?}");
    assert!(
        dispatch
            .operations
            .lock()
            .expect("operations")
            .iter()
            .any(|operation| operation == "human_loop_request")
    );
    host.child.kill().await?;
    Ok(())
}

/// Scenario A3: an AgentFactory creates a correctly typed CustomAgent handle,
/// then the regular agent dispatch tool invokes that instance.
#[tokio::test]
async fn factory_and_custom_agent_extensions_round_trip() -> Result<(), Box<dyn std::error::Error>>
{
    let directory = tempfile::tempdir()?;
    let state_root = directory.path().join("state");
    let model = start_scripted_model(vec![
        tool_call_script(
            "agent_tool",
            r#"{"agent_name":"sdk-factory","task":"delegate first"}"#,
        ),
        final_script("delegated first"),
        tool_call_script(
            "agent_tool",
            r#"{"agent_name":"sdk-factory","task":"delegate second"}"#,
        ),
        final_script("delegated second"),
    ])
    .await?;
    let config = write_config_with_features(
        directory.path(),
        &format!("http://{model}/v1/chat/completions"),
        &state_root,
        false,
        false,
        true,
    );
    let mut host = spawn_host(&config).await?;
    let dispatch = Arc::new(SdkDispatch::default());
    let session_dir = directory.path().canonicalize()?;
    let outcome = drive_sdk(&mut host, dispatch.clone(), move |connection| {
        let session_dir = session_dir.clone();
        Box::pin(async move {
            connection
                .send_request(initialize_request(Some(client_hello())))
                .block_task()
                .await?;
            let factory = register_extension(
                &connection,
                ExtensionKind::AgentFactory,
                "sdk-factory",
                ExtensionDescriptor::AgentFactory {
                    descriptor_version: 1,
                },
                None,
            )
            .await?;
            let session: v1::NewSessionResponse = connection
                .send_request(v1::NewSessionRequest::new(session_dir))
                .block_task()
                .await?;
            let prompt = connection
                .send_request(v1::PromptRequest::new(
                    session.session_id.clone(),
                    vec![v1::ContentBlock::Text(v1::TextContent::new("delegate"))],
                ))
                .block_task()
                .await?;
            assert_eq!(prompt.stop_reason, v1::StopReason::EndTurn);
            let prompt = connection
                .send_request(v1::PromptRequest::new(
                    session.session_id,
                    vec![v1::ContentBlock::Text(v1::TextContent::new(
                        "delegate again",
                    ))],
                ))
                .block_task()
                .await?;
            assert_eq!(prompt.stop_reason, v1::StopReason::EndTurn);
            assert!(unregister(&connection, &factory).await?);
            Ok(())
        })
    })
    .await;
    assert!(
        outcome.is_ok(),
        "factory scenario failed: {outcome:?}; operations={:?}",
        dispatch.operations.lock().expect("operations")
    );
    let operations = dispatch.operations.lock().expect("operations").clone();
    assert!(
        operations
            .iter()
            .filter(|operation| operation.as_str() == "factory_create_agent")
            .count()
            >= 2,
        "operations={operations:?}"
    );
    assert!(
        operations
            .iter()
            .filter(|operation| {
                operation.as_str() == "agent_execute"
                    || operation.as_str() == "agent_execute_stream"
            })
            .count()
            >= 2,
        "operations={operations:?}"
    );
    assert!(
        operations
            .iter()
            .filter(|operation| operation.as_str() == "agent_close")
            .count()
            >= 2,
        "factory instances must close before dispatch completes: {operations:?}"
    );
    host.child.kill().await?;
    Ok(())
}

#[tokio::test]
async fn cancelled_factory_stream_closes_and_releases_instance()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let state_root = directory.path().join("state");
    let model = start_scripted_model(vec![tool_call_script(
        "agent_tool",
        r#"{"agent_name":"sdk-factory","task":"hang then cancel"}"#,
    )])
    .await?;
    let config = write_config_with_features(
        directory.path(),
        &format!("http://{model}/v1/chat/completions"),
        &state_root,
        false,
        false,
        true,
    );
    set_profile_limit(&config, "max_registered_extensions", 2)?;
    let mut host = spawn_host(&config).await?;
    let dispatch = Arc::new(SdkDispatch::default());
    dispatch.omit_stream_terminal.store(true, Ordering::Release);
    dispatch.hang.lock().expect("hang lock").push("agent_close");
    let operations = dispatch.operations.clone();
    let session_dir = directory.path().canonicalize()?;
    let outcome = drive_sdk(&mut host, dispatch.clone(), move |connection| {
        let operations = operations.clone();
        let session_dir = session_dir.clone();
        Box::pin(async move {
            connection
                .send_request(initialize_request(Some(client_hello())))
                .block_task()
                .await?;
            register_extension(
                &connection,
                ExtensionKind::AgentFactory,
                "sdk-factory",
                ExtensionDescriptor::AgentFactory {
                    descriptor_version: 1,
                },
                Some(WireDuration {
                    seconds: WireU64::from_u64(1),
                    nanos: 0,
                }),
            )
            .await?;
            let session: v1::NewSessionResponse = connection
                .send_request(v1::NewSessionRequest::new(session_dir.clone()))
                .block_task()
                .await?;
            let prompt = connection.send_request(v1::PromptRequest::new(
                session.session_id.clone(),
                vec![v1::ContentBlock::Text(v1::TextContent::new("delegate"))],
            ));
            for _ in 0..100 {
                if operations
                    .lock()
                    .expect("operations lock")
                    .iter()
                    .any(|operation| operation == "agent_execute_stream")
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert!(
                operations
                    .lock()
                    .expect("operations lock")
                    .iter()
                    .any(|operation| operation == "agent_execute_stream"),
                "factory stream did not start"
            );

            // Construct another Session while the factory instance is live.
            // Session construction must see only direct registrations.
            let _second: v1::NewSessionResponse = connection
                .send_request(v1::NewSessionRequest::new(session_dir))
                .block_task()
                .await?;
            connection.send_notification(v1::CancelNotification::new(session.session_id))?;
            let _ = tokio::time::timeout(Duration::from_secs(5), prompt.block_task()).await;

            // AgentClose intentionally hangs. Its independent one-second
            // cleanup deadline must still release the instance slot.
            tokio::time::sleep(Duration::from_millis(1_500)).await;
            let probe = register_extension(
                &connection,
                ExtensionKind::Tool,
                "cleanup-probe",
                tool_descriptor("cleanup_probe"),
                None,
            )
            .await?;
            assert!(unregister(&connection, &probe).await?);
            Ok(())
        })
    })
    .await;
    assert!(
        outcome.is_ok(),
        "factory cancellation scenario failed: {outcome:?}"
    );
    let operations = dispatch.operations.lock().expect("operations").clone();
    assert!(
        operations
            .iter()
            .any(|operation| operation == "agent_close")
    );
    host.child.kill().await?;
    Ok(())
}

#[tokio::test]
async fn directly_registered_custom_agent_round_trips_and_unregisters()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let state_root = directory.path().join("state");
    let model = start_scripted_model(vec![
        tool_call_script(
            "agent_tool",
            r#"{"agent_name":"sdk-custom","task":"delegate this"}"#,
        ),
        final_script("delegated"),
    ])
    .await?;
    let config = write_config_with_features(
        directory.path(),
        &format!("http://{model}/v1/chat/completions"),
        &state_root,
        false,
        false,
        true,
    );
    let mut host = spawn_host(&config).await?;
    let dispatch = Arc::new(SdkDispatch::default());
    let session_dir = directory.path().canonicalize()?;
    let outcome = drive_sdk(&mut host, dispatch.clone(), move |connection| {
        let session_dir = session_dir.clone();
        Box::pin(async move {
            connection
                .send_request(initialize_request(Some(client_hello())))
                .block_task()
                .await?;
            let custom = register_extension(
                &connection,
                ExtensionKind::CustomAgent,
                "sdk-custom-implementation",
                ExtensionDescriptor::CustomAgent {
                    descriptor_version: 1,
                    name: "sdk-custom".to_string(),
                    model_name: "sdk-custom-model".to_string(),
                    system_prompt: "SDK custom agent".to_string(),
                    tool_names: Vec::new(),
                },
                None,
            )
            .await?;
            let session: v1::NewSessionResponse = connection
                .send_request(v1::NewSessionRequest::new(session_dir))
                .block_task()
                .await?;
            let prompt = connection
                .send_request(v1::PromptRequest::new(
                    session.session_id,
                    vec![v1::ContentBlock::Text(v1::TextContent::new("delegate"))],
                ))
                .block_task()
                .await?;
            assert_eq!(prompt.stop_reason, v1::StopReason::EndTurn);
            assert!(unregister(&connection, &custom).await?);
            Ok(())
        })
    })
    .await;
    assert!(outcome.is_ok(), "custom Agent scenario failed: {outcome:?}");
    let operations = dispatch.operations.lock().expect("operations").clone();
    assert!(
        operations
            .iter()
            .any(|operation| operation == "agent_execute" || operation == "agent_execute_stream")
    );
    host.child.kill().await?;
    Ok(())
}

/// Scenario B: a registered LlmClient replaces the model transport; the
/// streaming callback delivers chunks and the Host accepts exactly one of
/// two duplicate wire terminals.
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
    dispatch.duplicate_terminal.store(true, Ordering::Release);

    let outcome = drive_sdk(&mut host, dispatch.clone(), |connection| {
        Box::pin(async move {
            connection
                .send_request(initialize_request(Some(client_hello())))
                .block_task()
                .await?;
            let llm = register_extension(
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
            assert!(unregister(&connection, &llm).await?);
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

#[tokio::test]
async fn non_streaming_llm_extension_adapts_chat_to_framework_stream()
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
    let session_dir = directory.path().canonicalize()?;
    let outcome = drive_sdk(&mut host, dispatch.clone(), move |connection| {
        let session_dir = session_dir.clone();
        Box::pin(async move {
            connection
                .send_request(initialize_request(Some(client_hello())))
                .block_task()
                .await?;
            register_extension(
                &connection,
                ExtensionKind::LlmClient,
                "sdk-llm-non-streaming",
                llm_descriptor_with_streaming("sdk-chat-only-model", false),
                None,
            )
            .await?;
            let session: v1::NewSessionResponse = connection
                .send_request(v1::NewSessionRequest::new(session_dir))
                .block_task()
                .await?;
            let prompt = connection
                .send_request(v1::PromptRequest::new(
                    session.session_id,
                    vec![v1::ContentBlock::Text(v1::TextContent::new("chat only"))],
                ))
                .block_task()
                .await?;
            assert_eq!(prompt.stop_reason, v1::StopReason::EndTurn);
            Ok(())
        })
    })
    .await;
    assert!(
        outcome.is_ok(),
        "non-streaming LLM scenario failed: {outcome:?}"
    );
    let operations = dispatch.operations.lock().expect("operations").clone();
    assert!(operations.iter().any(|operation| operation == "llm_chat"));
    assert!(
        !operations
            .iter()
            .any(|operation| operation == "llm_chat_stream")
    );
    host.child.kill().await?;
    Ok(())
}

#[tokio::test]
async fn non_streaming_llm_without_finish_reason_fails_closed()
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
    dispatch
        .missing_finish_reason
        .store(true, Ordering::Release);
    let session_dir = directory.path().canonicalize()?;
    let outcome = drive_sdk(&mut host, dispatch, move |connection| {
        let session_dir = session_dir.clone();
        Box::pin(async move {
            connection
                .send_request(initialize_request(Some(client_hello())))
                .block_task()
                .await?;
            register_extension(
                &connection,
                ExtensionKind::LlmClient,
                "sdk-llm-missing-finish",
                llm_descriptor_with_streaming("sdk-chat-only-model", false),
                None,
            )
            .await?;
            let session: v1::NewSessionResponse = connection
                .send_request(v1::NewSessionRequest::new(session_dir))
                .block_task()
                .await?;
            let prompt = connection
                .send_request(v1::PromptRequest::new(
                    session.session_id,
                    vec![v1::ContentBlock::Text(v1::TextContent::new(
                        "missing finish",
                    ))],
                ))
                .block_task()
                .await;
            assert!(prompt.is_err());
            Ok(())
        })
    })
    .await;
    assert!(
        outcome.is_ok(),
        "missing-finish scenario failed: {outcome:?}"
    );
    host.child.kill().await?;
    Ok(())
}

#[tokio::test]
async fn malformed_stream_kind_fails_without_waiting_for_deadline()
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
    dispatch.malformed_stream.store(true, Ordering::Release);
    let session_dir = directory.path().canonicalize()?;
    let started = std::time::Instant::now();
    let outcome = drive_sdk(&mut host, dispatch, move |connection| {
        let session_dir = session_dir.clone();
        Box::pin(async move {
            connection
                .send_request(initialize_request(Some(client_hello())))
                .block_task()
                .await?;
            register_extension(
                &connection,
                ExtensionKind::LlmClient,
                "sdk-llm-malformed",
                llm_descriptor("sdk-malformed-model"),
                Some(WireDuration {
                    seconds: WireU64::from_u64(30),
                    nanos: 0,
                }),
            )
            .await?;
            let session: v1::NewSessionResponse = connection
                .send_request(v1::NewSessionRequest::new(session_dir))
                .block_task()
                .await?;
            let prompt = connection
                .send_request(v1::PromptRequest::new(
                    session.session_id,
                    vec![v1::ContentBlock::Text(v1::TextContent::new("malformed"))],
                ))
                .block_task()
                .await;
            assert!(prompt.is_err());
            Ok(())
        })
    })
    .await;
    assert!(
        outcome.is_ok(),
        "malformed stream scenario failed: {outcome:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    host.child.kill().await?;
    Ok(())
}

#[tokio::test]
async fn out_of_order_stream_fails_without_waiting_for_deadline()
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
    dispatch.out_of_order_stream.store(true, Ordering::Release);
    let session_dir = directory.path().canonicalize()?;
    let started = std::time::Instant::now();
    let outcome = drive_sdk(&mut host, dispatch, move |connection| {
        let session_dir = session_dir.clone();
        Box::pin(async move {
            connection
                .send_request(initialize_request(Some(client_hello())))
                .block_task()
                .await?;
            register_extension(
                &connection,
                ExtensionKind::LlmClient,
                "sdk-llm-out-of-order",
                llm_descriptor("sdk-out-of-order-model"),
                Some(WireDuration {
                    seconds: WireU64::from_u64(30),
                    nanos: 0,
                }),
            )
            .await?;
            let session: v1::NewSessionResponse = connection
                .send_request(v1::NewSessionRequest::new(session_dir))
                .block_task()
                .await?;
            let prompt = connection
                .send_request(v1::PromptRequest::new(
                    session.session_id,
                    vec![v1::ContentBlock::Text(v1::TextContent::new("out of order"))],
                ))
                .block_task()
                .await;
            assert!(prompt.is_err());
            Ok(())
        })
    })
    .await;
    assert!(
        outcome.is_ok(),
        "out-of-order stream scenario failed: {outcome:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    host.child.kill().await?;
    Ok(())
}

#[tokio::test]
async fn oversized_stream_chunk_fails_without_waiting_for_deadline()
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
    dispatch.oversized_stream.store(true, Ordering::Release);
    let session_dir = directory.path().canonicalize()?;
    let started = std::time::Instant::now();
    let outcome = drive_sdk(&mut host, dispatch, move |connection| {
        let session_dir = session_dir.clone();
        Box::pin(async move {
            connection
                .send_request(initialize_request(Some(client_hello())))
                .block_task()
                .await?;
            register_extension(
                &connection,
                ExtensionKind::LlmClient,
                "sdk-llm-oversized",
                llm_descriptor("sdk-oversized-model"),
                Some(WireDuration {
                    seconds: WireU64::from_u64(30),
                    nanos: 0,
                }),
            )
            .await?;
            let session: v1::NewSessionResponse = connection
                .send_request(v1::NewSessionRequest::new(session_dir))
                .block_task()
                .await?;
            let prompt = connection
                .send_request(v1::PromptRequest::new(
                    session.session_id,
                    vec![v1::ContentBlock::Text(v1::TextContent::new("oversized"))],
                ))
                .block_task()
                .await;
            assert!(prompt.is_err());
            Ok(())
        })
    })
    .await;
    assert!(
        outcome.is_ok(),
        "oversized stream scenario failed: {outcome:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    host.child.kill().await?;
    Ok(())
}

#[tokio::test]
async fn stream_flood_hits_bounded_host_backpressure() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let state_root = directory.path().join("state");
    let model = start_scripted_model(vec![final_script("unused")]).await?;
    let config = write_config(
        directory.path(),
        &format!("http://{model}/v1/chat/completions"),
        &state_root,
    );
    set_profile_limit(&config, "max_outstanding_live_events", 1)?;
    let mut host = spawn_host(&config).await?;
    let dispatch = Arc::new(SdkDispatch::default());
    dispatch.flood_stream.store(true, Ordering::Release);
    let session_dir = directory.path().canonicalize()?;
    let started = std::time::Instant::now();
    let outcome = drive_sdk(&mut host, dispatch, move |connection| {
        let session_dir = session_dir.clone();
        Box::pin(async move {
            connection
                .send_request(initialize_request(Some(client_hello())))
                .block_task()
                .await?;
            register_extension(
                &connection,
                ExtensionKind::LlmClient,
                "sdk-llm-flood",
                llm_descriptor("sdk-flood-model"),
                Some(WireDuration {
                    seconds: WireU64::from_u64(30),
                    nanos: 0,
                }),
            )
            .await?;
            let session: v1::NewSessionResponse = connection
                .send_request(v1::NewSessionRequest::new(session_dir))
                .block_task()
                .await?;
            let prompt = connection
                .send_request(v1::PromptRequest::new(
                    session.session_id,
                    vec![v1::ContentBlock::Text(v1::TextContent::new("flood"))],
                ))
                .block_task()
                .await;
            assert!(
                prompt.is_err(),
                "a flooded callback must fail, not buffer without bound"
            );
            Ok(())
        })
    })
    .await;
    assert!(outcome.is_ok(), "backpressure scenario failed: {outcome:?}");
    assert!(started.elapsed() < Duration::from_secs(10));
    let stderr = stderr_text(&host);
    assert!(
        stderr.contains("extension stream consumer exceeded its bounded mailbox"),
        "the real Host must report the extension mailbox backpressure branch: {stderr}"
    );
    assert!(!stderr.contains(SENTINEL_SECRET));
    host.child.kill().await?;
    Ok(())
}

#[tokio::test]
async fn sdk_disconnect_cancels_an_unterminated_stream_and_exits_host()
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
    dispatch.omit_stream_terminal.store(true, Ordering::Release);
    let session_dir = directory.path().canonicalize()?;
    let outcome = drive_sdk(&mut host, dispatch, move |connection| {
        let session_dir = session_dir.clone();
        Box::pin(async move {
            connection
                .send_request(initialize_request(Some(client_hello())))
                .block_task()
                .await?;
            register_extension(
                &connection,
                ExtensionKind::LlmClient,
                "sdk-llm-disconnect",
                llm_descriptor("sdk-disconnect-model"),
                Some(WireDuration {
                    seconds: WireU64::from_u64(30),
                    nanos: 0,
                }),
            )
            .await?;
            let session: v1::NewSessionResponse = connection
                .send_request(v1::NewSessionRequest::new(session_dir))
                .block_task()
                .await?;
            let prompt = connection.send_request(v1::PromptRequest::new(
                session.session_id,
                vec![v1::ContentBlock::Text(v1::TextContent::new("disconnect"))],
            ));
            tokio::time::sleep(Duration::from_millis(300)).await;
            drop(prompt);
            Ok(())
        })
    })
    .await;
    assert!(outcome.is_ok(), "disconnect scenario failed: {outcome:?}");
    let status = tokio::time::timeout(Duration::from_secs(5), host.child.wait()).await??;
    let stderr = stderr_text(&host);
    assert!(
        !status.success(),
        "an owner disconnect during an active response must surface as a transport failure"
    );
    assert!(!stderr.contains(SENTINEL_SECRET));
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
        let late_responders = dispatch.silent.clone();
        let outcome = drive_sdk(&mut host, dispatch.clone(), move |connection| {
            let late_responders = late_responders.clone();
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
                if let Some(responder) = late_responders.lock().expect("late responders lock").pop()
                {
                    let _ = responder.respond(ExtensionInvokeOutcome::Error {
                        error: EchoSdkError::new(
                            ExtensionErrorCode::ExtensionFailed,
                            "late fixture response",
                            Retryability::Never,
                        ),
                    });
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
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
        assert!(
            host.child.try_wait()?.is_none(),
            "late response killed the Host"
        );
        host.child.kill().await?;
    }

    // Cancellation: the client cancels the prompt mid-invocation.
    {
        let mut host = spawn_host(&config).await?;
        let dispatch = Arc::new(SdkDispatch::default());
        dispatch.omit_stream_terminal.store(true, Ordering::Release);
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
                // Fire the prompt without awaiting it yet: the callback
                // acknowledges a stream and emits chunks but no terminal.
                // Cancelling the run must drop that active consumer and send
                // the extension-level cancel notice.
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
