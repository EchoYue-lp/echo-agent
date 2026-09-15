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

use agent_client_protocol::UntypedMessage;
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
use echo_sdk_protocol::scalar::{WireDuration, WireNonZeroU64, WirePath, WireU64, WireValue};
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

fn facade_digest(operation: &str) -> Result<String, Box<dyn std::error::Error>> {
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
    start_scripted_model_with_delay(scripts, Duration::ZERO).await
}

async fn start_scripted_model_with_delay(
    scripts: Vec<Vec<serde_json::Value>>,
    response_delay: Duration,
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
                if !response_delay.is_zero() {
                    tokio::time::sleep(response_delay).await;
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
    String::from_utf8_lossy(
        host.stderr
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_slice(),
    )
    .to_string()
}

/// The fake SDK dispatcher state shared by client handlers.
#[derive(Default)]
struct SdkDispatch {
    operations: SharedVec<String>,
    compressor_extensions: SharedVec<String>,
    component_operations: SharedVec<String>,
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
    hang_sandbox_cancel: Arc<std::sync::atomic::AtomicBool>,
    reentrant_mutation: Arc<Mutex<Option<(WireHandle, WireHandle)>>>,
    reentrant_conflicts: Arc<AtomicUsize>,
    tokenizer_counts: Arc<AtomicUsize>,
    checkpoints: Arc<CheckpointDispatch>,
}

#[derive(Default)]
struct CheckpointDispatch {
    pending: Mutex<Option<WireValue>>,
    claimed: Mutex<Option<(String, String, WireValue)>>,
    renewals: AtomicUsize,
}

type CheckpointDispatchResult<T> = Result<T, Box<EchoSdkError>>;

impl CheckpointDispatch {
    fn invalid(message: impl Into<String>) -> Box<EchoSdkError> {
        Box::new(EchoSdkError::new(
            ExtensionErrorCode::InvalidValue,
            message.into(),
            Retryability::Never,
        ))
    }

    fn json(value: &WireValue) -> CheckpointDispatchResult<serde_json::Value> {
        value
            .clone()
            .into_json()
            .map_err(|error| Self::invalid(error.to_string()))
    }

    fn checkpoint_id(value: &WireValue) -> CheckpointDispatchResult<String> {
        Self::json(value)?
            .get("id")
            .and_then(serde_json::Value::as_str)
            .map(ToString::to_string)
            .ok_or_else(|| Self::invalid("checkpoint has no id"))
    }

    fn with_attempt(
        value: &WireValue,
        attempt_id: Option<&str>,
    ) -> CheckpointDispatchResult<WireValue> {
        let mut json = Self::json(value)?;
        let object = json
            .as_object_mut()
            .ok_or_else(|| Self::invalid("checkpoint is not an object"))?;
        object.insert(
            "resume_attempt_id".to_string(),
            attempt_id
                .map(|value| serde_json::Value::String(value.to_string()))
                .unwrap_or(serde_json::Value::Null),
        );
        WireValue::from_json(json).map_err(|error| Self::invalid(error.to_string()))
    }

    fn save(&self, checkpoint: WireValue) {
        *self
            .pending
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(checkpoint);
    }

    fn load(&self, id: &str) -> CheckpointDispatchResult<Option<WireValue>> {
        if let Some(checkpoint) = self
            .pending
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            && Self::checkpoint_id(checkpoint)? == id
        {
            return Ok(Some(checkpoint.clone()));
        }
        let claimed = self
            .claimed
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        Ok(claimed
            .as_ref()
            .filter(|(checkpoint_id, _, _)| checkpoint_id == id)
            .map(|(_, _, checkpoint)| checkpoint.clone()))
    }

    fn claim(&self, id: &str) -> CheckpointDispatchResult<Option<WireValue>> {
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(checkpoint) = pending.as_ref() else {
            return Ok(None);
        };
        if Self::checkpoint_id(checkpoint)? != id {
            return Ok(None);
        }
        let checkpoint = pending
            .take()
            .ok_or_else(|| Self::invalid("checkpoint claim disappeared"))?;
        drop(pending);
        let attempt_id = "sdk-resume-attempt".to_string();
        let claimed = Self::with_attempt(&checkpoint, Some(&attempt_id))?;
        *self
            .claimed
            .lock()
            .unwrap_or_else(|error| error.into_inner()) =
            Some((id.to_string(), attempt_id, claimed.clone()));
        Ok(Some(claimed))
    }

    fn settle_claim(
        &self,
        id: &str,
        attempt_id: &str,
        requeue: bool,
    ) -> CheckpointDispatchResult<()> {
        let mut claimed = self
            .claimed
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let matches = claimed
            .as_ref()
            .is_some_and(|(stored_id, stored_attempt, _)| {
                stored_id == id && stored_attempt == attempt_id
            });
        if !matches {
            return Err(Self::invalid("checkpoint claim owner mismatch"));
        }
        let (_, _, checkpoint) = claimed
            .take()
            .ok_or_else(|| Self::invalid("checkpoint claim disappeared"))?;
        drop(claimed);
        if requeue {
            self.save(Self::with_attempt(&checkpoint, None)?);
        }
        Ok(())
    }

    fn renew(&self, id: &str, attempt_id: &str) -> CheckpointDispatchResult<()> {
        let claimed = self
            .claimed
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !claimed
            .as_ref()
            .is_some_and(|(stored_id, stored_attempt, _)| {
                stored_id == id && stored_attempt == attempt_id
            })
        {
            return Err(Self::invalid("checkpoint claim owner mismatch"));
        }
        self.renewals.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }

    fn save_if_generation(
        &self,
        checkpoint: WireValue,
        expected_generation: WireU64,
    ) -> CheckpointDispatchResult<bool> {
        let Some(expected_generation) = expected_generation.to_u64() else {
            return Err(Self::invalid("expected_generation is out of range"));
        };
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(current) = pending.as_ref() else {
            return Ok(false);
        };
        let generation = Self::json(current)?
            .get("generation")
            .and_then(serde_json::Value::as_u64);
        if generation != Some(expected_generation) {
            return Ok(false);
        }
        *pending = Some(checkpoint);
        Ok(true)
    }
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

fn critic_descriptor(name: &str) -> ExtensionDescriptor {
    ExtensionDescriptor::Critic {
        descriptor_version: 1,
        name: name.to_string(),
    }
}

fn channel_handler_descriptor(handler_id: &str) -> ExtensionDescriptor {
    ExtensionDescriptor::ChannelMessageHandler(ChannelMessageHandlerDescriptorWire {
        descriptor_version: 1,
        handler_id: handler_id.to_string(),
    })
}

#[cfg(feature = "framework-channels")]
fn channel_plugin_descriptor(channel_id: &str, handler_id: &str) -> ExtensionDescriptor {
    ExtensionDescriptor::ChannelPlugin(ChannelPluginDescriptorWire {
        descriptor_version: 1,
        channel_id: channel_id.to_string(),
        label: "SDK channel".to_string(),
        capabilities: ChannelCapabilitiesWire {
            chat_types: vec![ChannelChatTypeWire::Direct],
            supports_media: false,
            supports_threads: false,
        },
        handler_id: handler_id.to_string(),
    })
}

#[cfg(feature = "framework-channels")]
fn channel_outbound_fixture(text: &str) -> ChannelOutboundMessageWire {
    ChannelOutboundMessageWire {
        channel_id: "sdk-channel".to_string(),
        to: "chat".to_string(),
        chat_type: ChannelChatTypeWire::Direct,
        text: text.to_string(),
        reply_to: None,
        attachments: Vec::new(),
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
    let compressor_extensions = dispatch.compressor_extensions.clone();
    let component_operations = dispatch.component_operations.clone();
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
    let hang_sandbox_cancel = dispatch.hang_sandbox_cancel.clone();
    let reentrant_mutation = dispatch.reentrant_mutation.clone();
    let reentrant_conflicts = dispatch.reentrant_conflicts.clone();
    let tokenizer_counts = dispatch.tokenizer_counts.clone();
    let checkpoints = dispatch.checkpoints.clone();
    let connect = Client
        .builder()
        .on_receive_request(
            {
                let operations = operations.clone();
                let compressor_extensions = compressor_extensions.clone();
                let component_operations = component_operations.clone();
                let silent = silent.clone();
                let hang = hang.clone();
                let malformed_stream = malformed_stream.clone();
                let out_of_order_stream = out_of_order_stream.clone();
                let duplicate_terminal = duplicate_terminal.clone();
                let omit_stream_terminal = omit_stream_terminal.clone();
                let oversized_stream = oversized_stream.clone();
                let flood_stream = flood_stream.clone();
                let missing_finish_reason = missing_finish_reason.clone();
                let hang_sandbox_cancel = hang_sandbox_cancel.clone();
                let reentrant_mutation = reentrant_mutation.clone();
                let reentrant_conflicts = reentrant_conflicts.clone();
                let tokenizer_counts = tokenizer_counts.clone();
                let checkpoints = checkpoints.clone();
                move |call: ExtensionInvokeCall,
                      responder: Responder<ExtensionInvokeOutcome>,
                      connection: ConnectionTo<agent_client_protocol::Agent>| {
                    let operations = operations.clone();
                    let compressor_extensions = compressor_extensions.clone();
                    let component_operations = component_operations.clone();
                    let silent = silent.clone();
                    let hang = hang.clone();
                    let malformed_stream = malformed_stream.clone();
                    let out_of_order_stream = out_of_order_stream.clone();
                    let duplicate_terminal = duplicate_terminal.clone();
                    let omit_stream_terminal = omit_stream_terminal.clone();
                    let oversized_stream = oversized_stream.clone();
                    let flood_stream = flood_stream.clone();
                    let missing_finish_reason = missing_finish_reason.clone();
                    let hang_sandbox_cancel = hang_sandbox_cancel.clone();
                    let reentrant_mutation = reentrant_mutation.clone();
                    let reentrant_conflicts = reentrant_conflicts.clone();
                    let tokenizer_counts = tokenizer_counts.clone();
                    let checkpoints = checkpoints.clone();
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
                            ExtensionInvocation::CompressorCompress(input) => {
                                compressor_extensions
                                    .lock()
                                    .unwrap_or_else(|error| error.into_inner())
                                    .push(call.extension.id.clone());
                                assert_eq!(
                                    input.focus_instructions.as_deref(),
                                    Some("preserve decisions")
                                );
                                let tokenizer_operation =
                                    "echo_core::tokenizer::Tokenizer::count_tokens";
                                let signature_digest = facade_digest(tokenizer_operation)
                                    .map_err(|error| agent_client_protocol::Error::internal_error().data(error.to_string()))?;
                                let counts = tokenizer_counts.clone();
                                let tokenizer = input.tokenizer.resource.clone();
                                let tokenizer_owner = input.tokenizer.owner_session_id.clone();
                                tokio::spawn(async move {
                                    let Ok(message) = UntypedMessage::new(
                                        "_echo_agent/facade/invoke",
                                        serde_json::json!({
                                            "operation": tokenizer_operation,
                                            "signature_digest": signature_digest,
                                            "handle": tokenizer,
                                            "arguments": [
                                                {"kind": "string", "value": tokenizer_owner},
                                                {"kind": "string", "value": "hello"}
                                            ],
                                        }),
                                    ) else {
                                        return;
                                    };
                                    let Ok(counted) = connection.send_request(message).block_task().await else {
                                        return;
                                    };
                                    let Ok(counted) = serde_json::from_value::<FeatureOperationResponse>(counted) else {
                                        return;
                                    };
                                    if counted.value.into_json().ok() == Some(serde_json::json!(1)) {
                                        counts.fetch_add(1, Ordering::AcqRel);
                                    }
                                });
                                tokio::time::sleep(Duration::from_millis(25)).await;
                                responder.respond(ExtensionInvokeOutcome::Result {
                                    result: ExtensionResult::CompressorCompress(
                                        CompressionOutputWire {
                                            messages: input.messages,
                                            evicted: Vec::new(),
                                            checkpoint: None,
                                        },
                                    ),
                                })
                            }
                            ExtensionInvocation::AgentComponentCall(input) => {
                                let operation = input.call.operation();
                                component_operations
                                    .lock()
                                    .unwrap_or_else(|error| error.into_inner())
                                    .push(format!("{:?}:{:?}", input.component, operation));
                                if operation
                                    == AgentComponentOperationWire::SandboxExecuteWithLimitsAndCancel
                                    && hang_sandbox_cancel.load(Ordering::Acquire)
                                {
                                    silent
                                        .lock()
                                        .unwrap_or_else(|error| error.into_inner())
                                        .push(responder);
                                    return Ok(());
                                }
                                let reentrant = reentrant_mutation
                                    .lock()
                                    .unwrap_or_else(|error| error.into_inner())
                                    .clone();
                                if operation == AgentComponentOperationWire::AuditLog
                                    && let Some((agent, session)) = reentrant
                                {
                                    let mutation =
                                        "echo_agent::agent::react::ReactAgent::set_plan_mode";
                                    let signature_digest = facade_digest(mutation)
                                        .map_err(|error| agent_client_protocol::Error::internal_error().data(error.to_string()))?;
                                    let conflicts = reentrant_conflicts.clone();
                                    tokio::spawn(async move {
                                        let Ok(message) = UntypedMessage::new(
                                            "_echo_agent/facade/invoke",
                                            serde_json::json!({
                                                "operation": mutation,
                                                "signature_digest": signature_digest,
                                                "handle": agent,
                                                "arguments": [
                                                    {"kind": "handle", "value": session},
                                                    {"kind": "bool", "value": true}
                                                ],
                                            }),
                                        ) else {
                                            return;
                                        };
                                        if let Err(error) = connection
                                            .send_request(message)
                                            .block_task()
                                            .await
                                            && EchoSdkError::from_jsonrpc_data(error.data.as_ref())
                                                .is_ok_and(|typed| typed.code == ExtensionErrorCode::ExtensionConflict)
                                        {
                                            conflicts.fetch_add(1, Ordering::AcqRel);
                                        }
                                    });
                                    tokio::time::sleep(Duration::from_millis(25)).await;
                                }
                                let result = match input.call {
                                    AgentComponentCallInputWire::ConversationCreate { conversation } => {
                                        AgentComponentCallResultWire::ConversationCreate { conversation }
                                    }
                                    AgentComponentCallInputWire::ConversationGet { .. } => {
                                        AgentComponentCallResultWire::ConversationGet { conversation: None }
                                    }
                                    AgentComponentCallInputWire::ConversationList { .. } => {
                                        AgentComponentCallResultWire::ConversationList { conversations: Vec::new() }
                                    }
                                    AgentComponentCallInputWire::ConversationUpdate { .. } => AgentComponentCallResultWire::ConversationUpdate,
                                    AgentComponentCallInputWire::ConversationDelete { .. } => AgentComponentCallResultWire::ConversationDelete,
                                    AgentComponentCallInputWire::ConversationSaveMessages { .. } => AgentComponentCallResultWire::ConversationSaveMessages,
                                    AgentComponentCallInputWire::ConversationGetMessages { .. } => {
                                        AgentComponentCallResultWire::ConversationGetMessages { messages: Vec::new() }
                                    }
                                    AgentComponentCallInputWire::ConversationCountMessages { .. } => {
                                        AgentComponentCallResultWire::ConversationCountMessages { count: WireU64::from_u64(0) }
                                    }
                                    AgentComponentCallInputWire::ConversationEnsure { conversation } => {
                                        AgentComponentCallResultWire::ConversationEnsure { conversation }
                                    }
                                    AgentComponentCallInputWire::ConversationSearch { .. } => {
                                        AgentComponentCallResultWire::ConversationSearch { conversations: Vec::new() }
                                    }
                                    AgentComponentCallInputWire::RunSave { .. } => AgentComponentCallResultWire::RunSave,
                                    AgentComponentCallInputWire::RunLoad { .. } => AgentComponentCallResultWire::RunLoad { run: None },
                                    AgentComponentCallInputWire::RunListBySession { .. } => AgentComponentCallResultWire::RunListBySession { runs: Vec::new() },
                                    AgentComponentCallInputWire::RunListAll { .. } => AgentComponentCallResultWire::RunListAll { runs: Vec::new() },
                                    AgentComponentCallInputWire::RunAppendEvent { .. } => AgentComponentCallResultWire::RunAppendEvent,
                                    AgentComponentCallInputWire::RunListByParent { .. } => AgentComponentCallResultWire::RunListByParent { runs: Vec::new() },
                                    AgentComponentCallInputWire::RuntimeGetCheckpoint { .. } => AgentComponentCallResultWire::RuntimeGetCheckpoint { checkpoint: None },
                                    AgentComponentCallInputWire::RuntimeSaveCheckpoint { .. } => AgentComponentCallResultWire::RuntimeSaveCheckpoint,
                                    AgentComponentCallInputWire::RuntimeSaveCheckpointForScope { .. } => AgentComponentCallResultWire::RuntimeSaveCheckpointForScope,
                                    AgentComponentCallInputWire::RuntimeStateIds { .. } => AgentComponentCallResultWire::RuntimeStateIds { state_ids: Vec::new() },
                                    AgentComponentCallInputWire::RuntimeClearState { .. } => {
                                        let receipt = WireValue::from_json(serde_json::json!({
                                            "scope_id": "scope",
                                            "runtime_state_id": "runtime",
                                            "checkpoint_removed": false
                                        }))
                                        .map_err(|error| agent_client_protocol::Error::internal_error().data(error.to_string()))?;
                                        AgentComponentCallResultWire::RuntimeClearState { receipt }
                                    }
                                    AgentComponentCallInputWire::RuntimeClearScope { .. } => {
                                        let receipt = WireValue::from_json(serde_json::json!({
                                            "scope_id": "scope",
                                            "runtime_state_ids": []
                                        }))
                                        .map_err(|error| agent_client_protocol::Error::internal_error().data(error.to_string()))?;
                                        AgentComponentCallResultWire::RuntimeClearScope { receipt }
                                    }
                                    AgentComponentCallInputWire::RuntimeClearConversation { .. } => AgentComponentCallResultWire::RuntimeClearConversation,
                                    AgentComponentCallInputWire::AuditLog { .. } => AgentComponentCallResultWire::AuditLog,
                                    AgentComponentCallInputWire::AuditQuery { .. } => AgentComponentCallResultWire::AuditQuery { events: Vec::new() },
                                    AgentComponentCallInputWire::ContextProject { .. } => AgentComponentCallResultWire::ContextProject { projections: Vec::new() },
                                    AgentComponentCallInputWire::MemoryTrigger { .. } => AgentComponentCallResultWire::MemoryTrigger { disposition: "persist".to_string() },
                                    AgentComponentCallInputWire::GuardCheck { .. } => {
                                        AgentComponentCallResultWire::GuardCheck {
                                            result: WireValue::String("Pass".to_string()),
                                        }
                                    }
                                    AgentComponentCallInputWire::SearchProviderSearch { .. } => {
                                        AgentComponentCallResultWire::SearchProviderSearch {
                                            results: Vec::new(),
                                        }
                                    }
                                    AgentComponentCallInputWire::WorkflowCheckpointSave { checkpoint } => {
                                        checkpoints.save(checkpoint);
                                        AgentComponentCallResultWire::WorkflowCheckpointSave
                                    }
                                    AgentComponentCallInputWire::WorkflowCheckpointSaveIfGeneration { checkpoint, expected_generation } => {
                                        let committed = match checkpoints.save_if_generation(checkpoint, expected_generation) {
                                            Ok(committed) => committed,
                                            Err(error) => return responder.respond(ExtensionInvokeOutcome::Error { error: *error }),
                                        };
                                        AgentComponentCallResultWire::WorkflowCheckpointSaveIfGeneration { committed }
                                    }
                                    AgentComponentCallInputWire::WorkflowCheckpointLoad { checkpoint_id } => {
                                        let checkpoint = match checkpoints.load(&checkpoint_id) {
                                            Ok(checkpoint) => checkpoint,
                                            Err(error) => return responder.respond(ExtensionInvokeOutcome::Error { error: *error }),
                                        };
                                        AgentComponentCallResultWire::WorkflowCheckpointLoad { checkpoint }
                                    }
                                    AgentComponentCallInputWire::WorkflowCheckpointClaim { checkpoint_id } => {
                                        let checkpoint = match checkpoints.claim(&checkpoint_id) {
                                            Ok(checkpoint) => checkpoint,
                                            Err(error) => return responder.respond(ExtensionInvokeOutcome::Error { error: *error }),
                                        };
                                        AgentComponentCallResultWire::WorkflowCheckpointClaim { checkpoint }
                                    }
                                    AgentComponentCallInputWire::WorkflowCheckpointAckClaim { checkpoint_id, attempt_id } => {
                                        if let Err(error) = checkpoints.settle_claim(&checkpoint_id, &attempt_id, false) {
                                            return responder.respond(ExtensionInvokeOutcome::Error { error: *error });
                                        }
                                        AgentComponentCallResultWire::WorkflowCheckpointAckClaim
                                    }
                                    AgentComponentCallInputWire::WorkflowCheckpointRequeueClaim { checkpoint_id, attempt_id } => {
                                        if let Err(error) = checkpoints.settle_claim(&checkpoint_id, &attempt_id, true) {
                                            return responder.respond(ExtensionInvokeOutcome::Error { error: *error });
                                        }
                                        AgentComponentCallResultWire::WorkflowCheckpointRequeueClaim
                                    }
                                    AgentComponentCallInputWire::WorkflowCheckpointRenewClaim { checkpoint_id, attempt_id } => {
                                        if let Err(error) = checkpoints.renew(&checkpoint_id, &attempt_id) {
                                            return responder.respond(ExtensionInvokeOutcome::Error { error: *error });
                                        }
                                        AgentComponentCallResultWire::WorkflowCheckpointRenewClaim
                                    }
                                    AgentComponentCallInputWire::WorkflowCheckpointList => AgentComponentCallResultWire::WorkflowCheckpointList { checkpoints: Vec::new() },
                                    AgentComponentCallInputWire::WorkflowCheckpointListByGraph { .. } => AgentComponentCallResultWire::WorkflowCheckpointListByGraph { checkpoints: Vec::new() },
                                    AgentComponentCallInputWire::WorkflowCheckpointListFiltered { .. } => AgentComponentCallResultWire::WorkflowCheckpointListFiltered { checkpoints: Vec::new() },
                                    AgentComponentCallInputWire::WorkflowCheckpointDelete { .. } => AgentComponentCallResultWire::WorkflowCheckpointDelete,
                                    AgentComponentCallInputWire::WorkflowCheckpointClear => AgentComponentCallResultWire::WorkflowCheckpointClear,
                                    AgentComponentCallInputWire::RevisionedTaskLoad { .. } => AgentComponentCallResultWire::RevisionedTaskLoad { graph: None },
                                    AgentComponentCallInputWire::RevisionedTaskCompareAndCommit { commit, .. } => AgentComponentCallResultWire::RevisionedTaskCompareAndCommit { graph: commit },
                                    AgentComponentCallInputWire::SandboxIsAvailable => AgentComponentCallResultWire::SandboxIsAvailable { available: true },
                                    AgentComponentCallInputWire::SandboxExecute { .. } => AgentComponentCallResultWire::SandboxExecute {
                                        result: WireValue::from_json(serde_json::json!({
                                            "exit_code": 0,
                                            "stdout": "sandbox answer",
                                            "stderr": "",
                                            "duration": {"secs": 0, "nanos": 1_000_000},
                                            "sandbox_type": "sdk",
                                            "timed_out": false,
                                            "cancelled": false,
                                            "output_truncated": false,
                                            "stdout_bytes": 14,
                                            "stderr_bytes": 0
                                        })).map_err(|error| agent_client_protocol::Error::internal_error().data(error.to_string()))?,
                                    },
                                    AgentComponentCallInputWire::SandboxExecuteWithLimits { .. } => AgentComponentCallResultWire::SandboxExecuteWithLimits {
                                        result: WireValue::from_json(serde_json::json!({
                                            "exit_code": 0,
                                            "stdout": "sandbox answer",
                                            "stderr": "",
                                            "duration": {"secs": 0, "nanos": 1_000_000},
                                            "sandbox_type": "sdk",
                                            "timed_out": false,
                                            "cancelled": false,
                                            "output_truncated": false,
                                            "stdout_bytes": 14,
                                            "stderr_bytes": 0
                                        })).map_err(|error| agent_client_protocol::Error::internal_error().data(error.to_string()))?,
                                    },
                                    AgentComponentCallInputWire::SandboxExecuteWithLimitsAndCancel { .. } => AgentComponentCallResultWire::SandboxExecuteWithLimitsAndCancel {
                                        result: WireValue::from_json(serde_json::json!({
                                            "exit_code": 0,
                                            "stdout": "sandbox answer",
                                            "stderr": "",
                                            "duration": {"secs": 0, "nanos": 1_000_000},
                                            "sandbox_type": "sdk",
                                            "timed_out": false,
                                            "cancelled": false,
                                            "output_truncated": false,
                                            "stdout_bytes": 14,
                                            "stderr_bytes": 0
                                        })).map_err(|error| agent_client_protocol::Error::internal_error().data(error.to_string()))?,
                                    },
                                    AgentComponentCallInputWire::SandboxCleanup => AgentComponentCallResultWire::SandboxCleanup,
                                    AgentComponentCallInputWire::McpTransportSend { request } => {
                                        let Ok(request) = request.into_json() else {
                                            return responder.respond(ExtensionInvokeOutcome::Error {
                                                error: EchoSdkError::new(
                                                    ExtensionErrorCode::InvalidValue,
                                                    "MCP request is not JSON",
                                                    Retryability::Never,
                                                ),
                                            });
                                        };
                                        let Ok(response) = WireValue::from_json(serde_json::json!({
                                            "jsonrpc": "2.0",
                                            "id": request.get("id").cloned(),
                                            "result": {
                                                "protocolVersion": "2025-11-25",
                                                "capabilities": {},
                                                "serverInfo": {
                                                    "name": "sdk-transport",
                                                    "version": "1.0.0"
                                                }
                                            }
                                        })) else {
                                            return responder.respond(ExtensionInvokeOutcome::Error {
                                                error: EchoSdkError::new(
                                                    ExtensionErrorCode::SerializationViolation,
                                                    "MCP response could not be encoded",
                                                    Retryability::Never,
                                                ),
                                            });
                                        };
                                        AgentComponentCallResultWire::McpTransportSend { response }
                                    },
                                    AgentComponentCallInputWire::McpTransportNotify { .. } => AgentComponentCallResultWire::McpTransportNotify,
                                    AgentComponentCallInputWire::McpTransportClose => AgentComponentCallResultWire::McpTransportClose,
                                    AgentComponentCallInputWire::McpTransportTryNotification => AgentComponentCallResultWire::McpTransportTryNotification { notification: None },
                                    AgentComponentCallInputWire::EmbedderEmbed { .. } => AgentComponentCallResultWire::EmbedderEmbed { vector: vec![0.0, 1.0] },
                                    AgentComponentCallInputWire::MemoryPromoterPromote { evicted } => AgentComponentCallResultWire::MemoryPromoterPromote {
                                        submitted: WireU64::from_u64(u64::try_from(evicted.len()).unwrap_or(u64::MAX)),
                                        promoted: WireU64::from_u64(0),
                                        deduplicated: WireU64::from_u64(0),
                                    },
                                    AgentComponentCallInputWire::WorkflowRun { .. } => AgentComponentCallResultWire::WorkflowRun { output: WireValue::Null },
                                    AgentComponentCallInputWire::IntentClassify { .. } => AgentComponentCallResultWire::IntentClassify {
                                        intent: WireValue::String("Fallback".to_string()),
                                    },
                                    AgentComponentCallInputWire::SkillLoadAllows { descriptor } => {
                                        AgentComponentCallResultWire::SkillLoadAllows {
                                            allowed: descriptor.name != "blocked",
                                        }
                                    }
                                    AgentComponentCallInputWire::SandboxExecuteStream { .. }
                                    | AgentComponentCallInputWire::WorkflowRunStream { .. } => {
                                        return responder.respond(ExtensionInvokeOutcome::Error {
                                            error: EchoSdkError::new(
                                                ExtensionErrorCode::InvalidValue,
                                                "streaming component used non-streaming invocation",
                                                Retryability::Never,
                                            ),
                                        });
                                    }
                                };
                                responder.respond(ExtensionInvokeOutcome::Result {
                                    result: ExtensionResult::AgentComponentCall(
                                        AgentComponentResultWire {
                                            component: input.component,
                                            result,
                                        },
                                    ),
                                })
                            }
                            ExtensionInvocation::AgentComponentCallStream(input) => {
                                let Some(stream) = call.stream.clone() else {
                                    return responder.respond(ExtensionInvokeOutcome::Error {
                                        error: EchoSdkError::new(
                                            ExtensionErrorCode::ExtensionFailed,
                                            "missing Agent component stream handle",
                                            Retryability::Never,
                                        ),
                                    });
                                };
                                responder.respond(ExtensionInvokeOutcome::Stream {
                                    stream: stream.clone(),
                                })?;
                                tokio::spawn(async move {
                                    let value = match input.call {
                                        AgentComponentCallInputWire::SandboxExecuteStream { .. } => {
                                            let Ok(result) = WireValue::from_json(serde_json::json!({
                                                "exit_code": 0,
                                                "stdout": "streamed sandbox",
                                                "stderr": "",
                                                "duration": {"secs": 0, "nanos": 1_000_000},
                                                "sandbox_type": "sdk",
                                                "timed_out": false,
                                                "cancelled": false,
                                                "output_truncated": false,
                                                "stdout_bytes": 16,
                                                "stderr_bytes": 0
                                            })) else {
                                                return;
                                            };
                                            ExtensionStreamCompleteValue::AgentComponent(
                                                AgentComponentStreamCompleteWire::Sandbox(
                                                    SandboxStreamCompleteWire::Complete { result },
                                                ),
                                            )
                                        }
                                        AgentComponentCallInputWire::WorkflowRunStream { .. } => {
                                            ExtensionStreamCompleteValue::AgentComponent(
                                                AgentComponentStreamCompleteWire::Workflow(
                                                    WorkflowStreamCompleteWire {
                                                        result: "streamed workflow".to_string(),
                                                        total_steps: WireU64::from_u64(1),
                                                        elapsed: WireDuration {
                                                            seconds: WireU64::from_u64(0),
                                                            nanos: 1_000_000,
                                                        },
                                                    },
                                                ),
                                            )
                                        }
                                        _ => return,
                                    };
                                    let _ = connection.send_notification(
                                        ExtensionStreamEvent::Complete {
                                            stream,
                                            sequence: nonzero(1),
                                            value,
                                        },
                                    );
                                });
                                Ok(())
                            }
                            ExtensionInvocation::CriticCritique(input) => {
                                assert_eq!(input.task, "solve");
                                assert_eq!(input.answer, "42");
                                assert_eq!(input.context, "math");
                                responder.respond(ExtensionInvokeOutcome::Result {
                                    result: ExtensionResult::CriticCritique(CritiqueWire {
                                        score: 9.5,
                                        passed: true,
                                        feedback: "good".to_string(),
                                        suggestions: Vec::new(),
                                    }),
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
                                if flood_stream.load(Ordering::Acquire) {
                                    // Deliver the first burst before answering
                                    // the reverse request. The Host has already
                                    // minted and registered the sink, but the
                                    // framework stream consumer cannot start
                                    // draining until the stream outcome is
                                    // received. This makes the bounded-mailbox
                                    // branch deterministic instead of racing
                                    // the consumer scheduler.
                                    for sequence in 1_u64..=2 {
                                        connection.send_notification(
                                            ExtensionStreamEvent::Chunk {
                                                stream: stream.clone(),
                                                sequence: nonzero(sequence),
                                                value: ExtensionStreamChunkValue::Llm(
                                                    chat_stream_chunk_wire("x"),
                                                ),
                                            },
                                        )?;
                                    }
                                    return responder
                                        .respond(ExtensionInvokeOutcome::Stream { stream });
                                }
                                responder.respond(ExtensionInvokeOutcome::Stream {
                                    stream: stream.clone(),
                                })?;
                                // Deliver the chunks from a spawned task so the
                                // client dispatch loop is never blocked by the
                                // callback's own stream production (design §12.3:
                                // the reader loop keeps dispatching).
                                tokio::spawn(async move {
                                    let reentrant = reentrant_mutation
                                        .lock()
                                        .unwrap_or_else(|error| error.into_inner())
                                        .clone();
                                    if let Some((agent, session)) = reentrant {
                                        let mutation = "echo_agent::agent::react::ReactAgent::set_plan_mode";
                                        let signature_digest =
                                            facade_digest(mutation).ok();
                                        let message = signature_digest.and_then(|signature_digest| {
                                            UntypedMessage::new(
                                                "_echo_agent/facade/invoke",
                                                serde_json::json!({
                                                    "operation": mutation,
                                                    "signature_digest": signature_digest,
                                                    "handle": agent,
                                                    "arguments": [
                                                        {"kind": "handle", "value": session},
                                                        {"kind": "bool", "value": true}
                                                    ],
                                                }),
                                            )
                                            .ok()
                                        });
                                        if let Some(message) = message
                                            && let Err(error) = connection
                                                .send_request(message)
                                                .block_task()
                                                .await
                                            && EchoSdkError::from_jsonrpc_data(error.data.as_ref())
                                                .is_ok_and(|typed| {
                                                    typed.code
                                                        == ExtensionErrorCode::ExtensionConflict
                                                })
                                        {
                                            reentrant_conflicts.fetch_add(1, Ordering::AcqRel);
                                        }
                                    }
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
                            #[cfg(feature = "framework-channels")]
                            ExtensionInvocation::ChannelStart(_) => {
                                responder.respond(ExtensionInvokeOutcome::Result {
                                    result: ExtensionResult::ChannelStart(ExtensionUnit),
                                })
                            }
                            #[cfg(feature = "framework-channels")]
                            ExtensionInvocation::ChannelStop(_) => {
                                responder.respond(ExtensionInvokeOutcome::Result {
                                    result: ExtensionResult::ChannelStop(ExtensionUnit),
                                })
                            }
                            #[cfg(feature = "framework-channels")]
                            ExtensionInvocation::ChannelSend(_) => {
                                responder.respond(ExtensionInvokeOutcome::Result {
                                    result: ExtensionResult::ChannelSend(ExtensionUnit),
                                })
                            }
                            #[cfg(feature = "framework-channels")]
                            ExtensionInvocation::ChannelHealth(_) => {
                                responder.respond(ExtensionInvokeOutcome::Result {
                                    result: ExtensionResult::ChannelHealth(ExtensionUnit),
                                })
                            }
                            #[cfg(feature = "framework-channels")]
                            ExtensionInvocation::ChannelHandle(input) => {
                                responder.respond(ExtensionInvokeOutcome::Result {
                                    result: ExtensionResult::ChannelHandle(
                                        channel_outbound_fixture(&format!(
                                            "reply: {}",
                                            input.message.text
                                        )),
                                    ),
                                })
                            }
                            #[cfg(feature = "framework-channels")]
                            ExtensionInvocation::ChannelHandleStream(input) => {
                                let Some(stream) = call.stream.clone() else {
                                    return responder.respond(ExtensionInvokeOutcome::Error {
                                        error: EchoSdkError::new(
                                            ExtensionErrorCode::ExtensionFailed,
                                            "missing stream handle",
                                            Retryability::Never,
                                        ),
                                    });
                                };
                                let text = format!("stream reply: {}", input.message.text);
                                responder.respond(ExtensionInvokeOutcome::Stream {
                                    stream: stream.clone(),
                                })?;
                                tokio::spawn(async move {
                                    let _ =
                                        connection.send_notification(ExtensionStreamEvent::Chunk {
                                            stream: stream.clone(),
                                            sequence: nonzero(1),
                                            value: ExtensionStreamChunkValue::Channel(
                                                channel_outbound_fixture(&text),
                                            ),
                                        });
                                    let _ = connection.send_notification(
                                        ExtensionStreamEvent::Complete {
                                            stream,
                                            sequence: nonzero(2),
                                            value: ExtensionStreamCompleteValue::Channel(
                                                channel_outbound_fixture("stream done"),
                                            ),
                                        },
                                    );
                                });
                                Ok(())
                            }
                            #[cfg(feature = "framework-channels")]
                            ExtensionInvocation::ChannelReply(_) => {
                                responder.respond(ExtensionInvokeOutcome::Result {
                                    result: ExtensionResult::ChannelReply(ExtensionUnit),
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
        ExtensionOperation::CriticCritique => ExtensionResult::CriticCritique(CritiqueWire {
            score: 10.0,
            passed: true,
            feedback: String::new(),
            suggestions: Vec::new(),
        }),
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

async fn invoke_source_operation(
    connection: &ConnectionTo<agent_client_protocol::Agent>,
    agent: WireHandle,
    operation: &str,
    arguments: Vec<WireValue>,
) -> Result<FeatureOperationResponse, RpcError> {
    let signature_digest = facade_digest(operation)
        .map_err(|error| RpcError::internal_error().data(error.to_string()))?;
    let request = FeatureOperationRequest {
        operation: operation.to_string(),
        signature_digest,
        handle: Some(agent),
        arguments,
    };
    let value = connection
        .send_request(UntypedMessage::new(
            "_echo_agent/facade/invoke",
            serde_json::to_value(request)
                .map_err(|error| RpcError::internal_error().data(error.to_string()))?,
        )?)
        .block_task()
        .await?;
    serde_json::from_value(value)
        .map_err(|error| RpcError::internal_error().data(error.to_string()))
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
    let operations = dispatch
        .operations
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
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

/// ChannelPlugin and MessageHandler registrations use the same connection
/// owned extension authority as the other reverse traits. The facade manager
/// owns the framework resource, while plugin lifecycle and send operations
/// cross the typed bridge with no local fallback.
#[cfg(feature = "framework-channels")]
#[tokio::test]
async fn channel_plugin_facade_round_trip() -> Result<(), Box<dyn std::error::Error>> {
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

    drive_sdk(&mut host, dispatch, move |connection| {
        Box::pin(async move {
            connection
                .send_request(initialize_request(Some(client_hello())))
                .block_task()
                .await?;
            let handler = register_extension(
                &connection,
                ExtensionKind::ChannelMessageHandler,
                "sdk-channel-handler",
                channel_handler_descriptor("sdk-channel-handler"),
                None,
            )
            .await?;
            let plugin = register_extension(
                &connection,
                ExtensionKind::ChannelPlugin,
                "sdk-channel-plugin",
                channel_plugin_descriptor("sdk-channel", "sdk-channel-handler"),
                None,
            )
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
            let request = |operation: &str, arguments: Vec<WireValue>| FeatureOperationRequest {
                operation: operation.to_string(),
                signature_digest: echo_sdk_protocol::facade::family_operation_signature_digest(
                    "channels", operation,
                ),
                handle: Some(session.session.clone()),
                arguments,
            };
            let to_wire = |value: serde_json::Value| {
                WireValue::from_json(value)
                    .map_err(|error| RpcError::internal_error().data(error.to_string()))
            };
            let invoke = |request: FeatureOperationRequest| async {
                let value = connection
                    .send_request(UntypedMessage::new(
                        "_echo_agent/channels/op",
                        serde_json::to_value(request)
                            .map_err(|error| RpcError::internal_error().data(error.to_string()))?,
                    )?)
                    .block_task()
                    .await?;
                let response: FeatureOperationResponse = serde_json::from_value(value)
                    .map_err(|error| RpcError::internal_error().data(error.to_string()))?;
                response
                    .value
                    .into_json()
                    .map_err(|error| RpcError::internal_error().data(error.to_string()))
            };
            let opened = invoke(request("channels.manager.open", Vec::new())).await?;
            let resource: WireHandle =
                serde_json::from_value(opened.get("resource").cloned().ok_or_else(|| {
                    RpcError::internal_error().data("channel manager did not return a resource")
                })?)
                .map_err(|error| RpcError::internal_error().data(error.to_string()))?;
            let registered =
                invoke(request(
                    "channels.plugin.register",
                    vec![
                        to_wire(serde_json::to_value(&resource).map_err(|error| {
                            RpcError::internal_error().data(error.to_string())
                        })?)?,
                        to_wire(serde_json::to_value(&plugin).map_err(|error| {
                            RpcError::internal_error().data(error.to_string())
                        })?)?,
                        to_wire(serde_json::to_value(&handler).map_err(|error| {
                            RpcError::internal_error().data(error.to_string())
                        })?)?,
                    ],
                ))
                .await?;
            assert_eq!(registered.get("registered"), Some(&serde_json::json!(true)));
            let started =
                invoke(request(
                    "channels.manager.start",
                    vec![
                        to_wire(serde_json::to_value(&resource).map_err(|error| {
                            RpcError::internal_error().data(error.to_string())
                        })?)?,
                        to_wire(serde_json::to_value(&handler).map_err(|error| {
                            RpcError::internal_error().data(error.to_string())
                        })?)?,
                    ],
                ))
                .await?;
            assert_eq!(started.get("channels"), Some(&serde_json::json!(1)));
            assert_eq!(started.get("failures"), Some(&serde_json::json!(0)));
            let listed = invoke(request(
                "channels.manager.list",
                vec![to_wire(serde_json::to_value(&resource).map_err(
                    |error| RpcError::internal_error().data(error.to_string()),
                )?)?],
            ))
            .await?;
            assert_eq!(
                listed.get("channels"),
                Some(&serde_json::json!(["sdk-channel"]))
            );
            let healthy = invoke(request(
                "channels.manager.health",
                vec![
                    to_wire(
                        serde_json::to_value(&resource)
                            .map_err(|error| RpcError::internal_error().data(error.to_string()))?,
                    )?,
                    WireValue::String("sdk-channel".to_string()),
                ],
            ))
            .await?;
            assert_eq!(healthy.get("healthy"), Some(&serde_json::json!(true)));
            let sent = invoke(request(
                "channels.plugin.send",
                vec![
                    to_wire(
                        serde_json::to_value(&resource)
                            .map_err(|error| RpcError::internal_error().data(error.to_string()))?,
                    )?,
                    WireValue::String("sdk-channel".to_string()),
                    to_wire(
                        serde_json::to_value(channel_outbound_fixture("hello"))
                            .map_err(|error| RpcError::internal_error().data(error.to_string()))?,
                    )?,
                ],
            ))
            .await?;
            assert_eq!(sent.get("sent"), Some(&serde_json::json!(true)));
            let stopped = invoke(request(
                "channels.manager.stop",
                vec![
                    to_wire(
                        serde_json::to_value(&resource)
                            .map_err(|error| RpcError::internal_error().data(error.to_string()))?,
                    )?,
                    WireValue::String("sdk-channel".to_string()),
                ],
            ))
            .await?;
            assert_eq!(stopped.get("stopped"), Some(&serde_json::json!(true)));
            Ok(())
        })
    })
    .await?;
    let _ = host.child.kill().await;
    Ok(())
}

/// A bridge-only Host must reject channel registrations because it has no
/// compiled framework channel authority to consume them.
#[cfg(not(feature = "framework-channels"))]
#[tokio::test]
async fn channel_extension_requires_framework_feature() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let state_root = directory.path().join("state");
    let model = start_scripted_model(vec![final_script("unused")]).await?;
    let config = write_config(
        directory.path(),
        &format!("http://{model}/v1/chat/completions"),
        &state_root,
    );
    let mut host = spawn_host(&config).await?;
    drive_sdk(
        &mut host,
        Arc::new(SdkDispatch::default()),
        move |connection| {
            Box::pin(async move {
                connection
                    .send_request(initialize_request(Some(client_hello())))
                    .block_task()
                    .await?;
                let result = register_extension(
                    &connection,
                    ExtensionKind::ChannelMessageHandler,
                    "sdk-channel-handler",
                    channel_handler_descriptor("sdk-channel-handler"),
                    None,
                )
                .await;
                assert!(
                    result.is_err(),
                    "channel registration must require the feature"
                );
                Ok(())
            })
        },
    )
    .await?;
    let _ = host.child.kill().await;
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
            .unwrap_or_else(|error| error.into_inner())
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

/// Critic registration is connection-owned and injects a typed proxy into
/// newly created Agents. Host defaults keep verifier_enabled=false, so a
/// normal prompt must remain fail-open without silently invoking the callback;
/// the protocol contract test covers the Critique DTO shape independently.
#[tokio::test]
async fn critic_registration_preserves_default_verifier_disabled_behavior()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let state_root = directory.path().join("state");
    let model = start_scripted_model(vec![final_script("answer")]).await?;
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
            let critic = register_extension(
                &connection,
                ExtensionKind::Critic,
                "sdk-critic",
                critic_descriptor("sdk-critic"),
                None,
            )
            .await?;
            let dto = CritiqueInput {
                task: "solve".to_string(),
                answer: "42".to_string(),
                context: "math".to_string(),
            };
            let invocation = ExtensionInvocation::CriticCritique(dto.clone());
            assert_eq!(invocation.operation(), ExtensionOperation::CriticCritique);
            let round_trip: CritiqueInput = serde_json::from_value(serde_json::to_value(dto)?)?;
            assert_eq!(round_trip.task, "solve");
            let session: v1::NewSessionResponse = connection
                .send_request(v1::NewSessionRequest::new(session_dir))
                .block_task()
                .await?;
            let prompt = connection
                .send_request(v1::PromptRequest::new(
                    session.session_id,
                    vec![v1::ContentBlock::Text(v1::TextContent::new("solve"))],
                ))
                .block_task()
                .await?;
            assert_eq!(prompt.stop_reason, v1::StopReason::EndTurn);
            assert!(unregister(&connection, &critic).await?);
            Ok::<_, RpcError>(())
        })
    })
    .await;
    assert!(outcome.is_ok(), "critic scenario failed: {outcome:?}");
    let operations = dispatch.operations.lock().expect("operations").clone();
    assert!(
        !operations
            .iter()
            .any(|operation| operation == "critic_critique"),
        "default verifier disabled must not invoke the registered Critic: {operations:?}"
    );
    host.child.kill().await?;
    Ok(())
}

#[tokio::test]
async fn context_compressor_registration_drives_session_compression()
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
    let outcome = drive_sdk(&mut host, dispatch.clone(), move |connection| {
        Box::pin(async move {
            connection
                .send_request(initialize_request(Some(client_hello())))
                .block_task()
                .await?;
            let first_compressor = register_extension(
                &connection,
                ExtensionKind::ContextCompressor,
                "sdk-context-compressor-first",
                ExtensionDescriptor::ContextCompressor {
                    descriptor_version: 1,
                    name: "sdk-context-compressor-first".to_string(),
                },
                None,
            )
            .await?;
            let second_compressor = register_extension(
                &connection,
                ExtensionKind::ContextCompressor,
                "sdk-context-compressor-second",
                ExtensionDescriptor::ContextCompressor {
                    descriptor_version: 1,
                    name: "sdk-context-compressor-second".to_string(),
                },
                None,
            )
            .await?;
            let agent = connection
                .send_request(AgentCreateRequest {
                    config: AgentConfigWire::HostDefault,
                    idempotency_id: None,
                })
                .block_task()
                .await?
                .agent;
            let first_session = connection
                .send_request(SessionCreateRequest {
                    agent: agent.clone(),
                    working_dir: None,
                    session_id: None,
                    idempotency_id: None,
                })
                .block_task()
                .await?;
            let second_session = connection
                .send_request(SessionCreateRequest {
                    agent: agent.clone(),
                    working_dir: None,
                    session_id: None,
                    idempotency_id: None,
                })
                .block_task()
                .await?;
            let bind_operation = "echo_agent::agent::react::ReactAgent::set_compressor";
            let bound = invoke_source_operation(
                &connection,
                agent.clone(),
                bind_operation,
                vec![
                    WireValue::Handle(first_session.session.clone()),
                    WireValue::Handle(first_compressor.clone()),
                ],
            )
            .await?;
            assert_eq!(bound.value, WireValue::Null);
            let operation =
                "echo_agent::agent::react::ReactAgent::force_compress_with_focus_and_hooks";
            for session in [first_session.session, second_session.session] {
                let response = invoke_source_operation(
                    &connection,
                    agent.clone(),
                    operation,
                    vec![
                        WireValue::Handle(session),
                        WireValue::String("preserve decisions".to_string()),
                        WireValue::U64(WireU64::from_u64(64)),
                        WireValue::String("manual".to_string()),
                    ],
                )
                .await?;
                assert!(matches!(response.value, WireValue::Map(_)));
            }
            assert!(unregister(&connection, &first_compressor).await?);
            assert!(unregister(&connection, &second_compressor).await?);
            Ok::<_, RpcError>(())
        })
    })
    .await;
    assert!(outcome.is_ok(), "compressor scenario failed: {outcome:?}");
    let operations = dispatch.operations.lock().expect("operations").clone();
    assert!(
        operations
            .iter()
            .any(|operation| operation == "compressor_compress"),
        "custom compressor was not invoked: {operations:?}"
    );
    let compressor_extensions = dispatch
        .compressor_extensions
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    assert_eq!(
        compressor_extensions.len(),
        2,
        "both Session compressors must be invoked: {compressor_extensions:?}"
    );
    assert_ne!(
        compressor_extensions.first(),
        compressor_extensions.get(1),
        "explicit Session binding must override the default latest registration"
    );
    assert_eq!(
        dispatch.tokenizer_counts.load(Ordering::Acquire),
        2,
        "each compressor callback must reach its Host-owned tokenizer"
    );
    host.child.kill().await?;
    Ok(())
}

#[cfg(all(feature = "framework-web", feature = "framework-shell"))]
#[tokio::test]
async fn agent_components_are_consumed_by_new_session_agents()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let state_root = directory.path().join("state");
    let model = start_scripted_model_with_delay(
        vec![
            vec![serde_json::json!({"invalid": true})],
            final_script("checkpoint resume answer"),
            tool_call_script("web_search", r#"{"query":"bridge"}"#),
            final_script("search component answer"),
            tool_call_script("shell", r#"{"command":"printf sandbox"}"#),
            final_script("sandbox component answer"),
        ],
        Duration::from_millis(10),
    )
    .await?;
    let config = write_config(
        directory.path(),
        &format!("http://{model}/v1/chat/completions"),
        &state_root,
    );
    let mut host = spawn_host(&config).await?;
    let dispatch = Arc::new(SdkDispatch::default());
    let reentrant_mutation = dispatch.reentrant_mutation.clone();
    let session_dir = directory.path().to_path_buf();
    let skill_dir = directory.path().join("policy-skills");
    for name in ["allowed", "blocked"] {
        let directory = skill_dir.join(name);
        std::fs::create_dir_all(&directory)?;
        std::fs::write(
            directory.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {name} skill\n---\nbody"),
        )?;
    }
    let outcome = drive_sdk(&mut host, dispatch.clone(), move |connection| {
        let session_dir = session_dir.clone();
        let skill_dir = skill_dir.clone();
        let reentrant_mutation = reentrant_mutation.clone();
        Box::pin(async move {
            connection
                .send_request(initialize_request(Some(client_hello())))
                .block_task()
                .await?;
            let audit = register_extension(
                &connection,
                ExtensionKind::AgentComponent,
                "sdk-audit",
                ExtensionDescriptor::AgentComponent {
                    descriptor_version: 1,
                    component: AgentComponentKindWire::AuditLogger,
                    name: "sdk-audit".to_string(),
                    capabilities: AgentComponentCapabilitiesWire::default(),
                },
                None,
            )
            .await?;
            let conversation = register_extension(
                &connection,
                ExtensionKind::AgentComponent,
                "sdk-conversation",
                ExtensionDescriptor::AgentComponent {
                    descriptor_version: 1,
                    component: AgentComponentKindWire::ConversationStore,
                    name: "sdk-conversation".to_string(),
                    capabilities: AgentComponentCapabilitiesWire::default(),
                },
                None,
            )
            .await?;
            let run_store = register_extension(
                &connection,
                ExtensionKind::AgentComponent,
                "sdk-run-store",
                ExtensionDescriptor::AgentComponent {
                    descriptor_version: 1,
                    component: AgentComponentKindWire::RunStore,
                    name: "sdk-run-store".to_string(),
                    capabilities: AgentComponentCapabilitiesWire::default(),
                },
                None,
            )
            .await?;
            let projector = register_extension(
                &connection,
                ExtensionKind::AgentComponent,
                "sdk-projector",
                ExtensionDescriptor::AgentComponent {
                    descriptor_version: 1,
                    component: AgentComponentKindWire::ContextProjector,
                    name: "sdk-projector".to_string(),
                    capabilities: AgentComponentCapabilitiesWire::default(),
                },
                None,
            )
            .await?;
            let guard = register_extension(
                &connection,
                ExtensionKind::AgentComponent,
                "sdk-guard",
                ExtensionDescriptor::AgentComponent {
                    descriptor_version: 1,
                    component: AgentComponentKindWire::Guard,
                    name: "sdk-guard".to_string(),
                    capabilities: AgentComponentCapabilitiesWire::default(),
                },
                None,
            )
            .await?;
            let search = register_extension(
                &connection,
                ExtensionKind::AgentComponent,
                "sdk-search",
                ExtensionDescriptor::AgentComponent {
                    descriptor_version: 1,
                    component: AgentComponentKindWire::SearchProvider,
                    name: "sdk-search".to_string(),
                    capabilities: AgentComponentCapabilitiesWire::default(),
                },
                None,
            )
            .await?;
            let sandbox = register_extension(
                &connection,
                ExtensionKind::AgentComponent,
                "sdk-sandbox",
                ExtensionDescriptor::AgentComponent {
                    descriptor_version: 1,
                    component: AgentComponentKindWire::SandboxExecutor,
                    name: "sdk-sandbox".to_string(),
                    capabilities: AgentComponentCapabilitiesWire {
                        isolation_level: Some("process".to_string()),
                        ..AgentComponentCapabilitiesWire::default()
                    },
                },
                None,
            )
            .await?;
            let intent = register_extension(
                &connection,
                ExtensionKind::AgentComponent,
                "sdk-intent",
                ExtensionDescriptor::AgentComponent {
                    descriptor_version: 1,
                    component: AgentComponentKindWire::IntentClassifier,
                    name: "sdk-intent".to_string(),
                    capabilities: AgentComponentCapabilitiesWire::default(),
                },
                None,
            )
            .await?;
            let skill_policy = register_extension(
                &connection,
                ExtensionKind::AgentComponent,
                "sdk-skill-policy",
                ExtensionDescriptor::AgentComponent {
                    descriptor_version: 1,
                    component: AgentComponentKindWire::SkillLoadPolicy,
                    name: "sdk-skill-policy".to_string(),
                    capabilities: AgentComponentCapabilitiesWire::default(),
                },
                None,
            )
            .await?;
            let checkpoint_store = register_extension(
                &connection,
                ExtensionKind::AgentComponent,
                "sdk-workflow-checkpoints",
                ExtensionDescriptor::AgentComponent {
                    descriptor_version: 1,
                    component: AgentComponentKindWire::WorkflowCheckpointStore,
                    name: "sdk-workflow-checkpoints".to_string(),
                    capabilities: AgentComponentCapabilitiesWire {
                        claim_heartbeat_interval_ms: Some(WireU64::from_u64(1)),
                        ..AgentComponentCapabilitiesWire::default()
                    },
                },
                None,
            )
            .await?;
            let agent = connection
                .send_request(AgentCreateRequest {
                    config: AgentConfigWire::HostDefault,
                    idempotency_id: Some("agent-component-reentry-agent".to_string()),
                })
                .block_task()
                .await?
                .agent;
            let session = connection
                .send_request(SessionCreateRequest {
                    agent: agent.clone(),
                    working_dir: Some(WirePath::Utf8 {
                        path: session_dir.display().to_string(),
                    }),
                    session_id: None,
                    idempotency_id: Some("agent-component-reentry-session".to_string()),
                })
                .block_task()
                .await?;
            *reentrant_mutation
                .lock()
                .unwrap_or_else(|error| error.into_inner()) =
                Some((agent, session.session.clone()));
            let agent_handle = reentrant_mutation
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .as_ref()
                .map(|(agent, _)| agent.clone())
                .ok_or_else(|| RpcError::internal_error().data("missing Agent handle"))?;
            let workflow_call = |operation: &str, arguments: Vec<WireValue>| {
                UntypedMessage::new(
                    "_echo_agent/workflow/op",
                    serde_json::to_value(FeatureOperationRequest {
                        operation: operation.to_string(),
                        signature_digest:
                            echo_sdk_protocol::facade::family_operation_signature_digest(
                                "workflow",
                                operation,
                            ),
                        handle: Some(session.session.clone()),
                        arguments,
                    })
                    .map_err(|error| RpcError::internal_error().data(error.to_string()))?,
                )
            };
            let definition = serde_json::json!({
                "name": "remote_checkpoint_settlement",
                "nodes": [{
                    "name": "work",
                    "type": "agent",
                    "system_prompt": "return a short answer",
                    "input_key": "task",
                    "output_key": "answer"
                }],
                "edges": [],
                "entry": "work",
                "finish": ["work"],
                "interrupt_before": ["work"]
            })
            .to_string();
            let built = connection
                .send_request(workflow_call(
                    "workflow.graph.build",
                    vec![WireValue::String(definition)],
                )?)
                .block_task()
                .await?;
            let built: FeatureOperationResponse = serde_json::from_value(built)
                .map_err(|error| RpcError::internal_error().data(error.to_string()))?;
            let built = built
                .value
                .into_json()
                .map_err(|error| RpcError::internal_error().data(error.to_string()))?;
            let graph: WireHandle = serde_json::from_value(
                built
                    .get("resource")
                    .cloned()
                    .ok_or_else(|| RpcError::internal_error().data("missing graph resource"))?,
            )
            .map_err(|error| RpcError::internal_error().data(error.to_string()))?;
            let graph_argument = WireValue::from_json(
                serde_json::to_value(&graph)
                    .map_err(|error| RpcError::internal_error().data(error.to_string()))?,
            )
            .map_err(|error| RpcError::internal_error().data(error.to_string()))?;
            let interrupted = connection
                .send_request(workflow_call(
                    "workflow.graph.run_until_interrupt",
                    vec![
                        graph_argument.clone(),
                        WireValue::from_json(serde_json::json!({"task": "checkpoint"}))
                            .map_err(|error| RpcError::internal_error().data(error.to_string()))?,
                    ],
                )?)
                .block_task()
                .await?;
            let interrupted: FeatureOperationResponse = serde_json::from_value(interrupted)
                .map_err(|error| RpcError::internal_error().data(error.to_string()))?;
            let interrupted = interrupted
                .value
                .into_json()
                .map_err(|error| RpcError::internal_error().data(error.to_string()))?;
            let checkpoint_id = interrupted
                .get("checkpoint")
                .and_then(|checkpoint| checkpoint.get("id"))
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| RpcError::internal_error().data("missing checkpoint id"))?
                .to_string();
            connection
                .send_request(workflow_call(
                    "workflow.graph.tag_checkpoint",
                    vec![
                        graph_argument.clone(),
                        WireValue::String(checkpoint_id.clone()),
                        WireValue::String("remote".to_string()),
                        WireValue::List(vec![WireValue::String("sdk".to_string())]),
                    ],
                )?)
                .block_task()
                .await?;
            let first_resume = connection
                .send_request(workflow_call(
                    "workflow.graph.resume",
                    vec![
                        graph_argument.clone(),
                        WireValue::String(checkpoint_id.clone()),
                        WireValue::String("approve".to_string()),
                    ],
                )?)
                .block_task()
                .await;
            assert!(
                first_resume.is_err(),
                "malformed provider response must fail and requeue the remote claim"
            );
            let resumed = connection
                .send_request(workflow_call(
                    "workflow.graph.resume",
                    vec![
                        graph_argument,
                        WireValue::String(checkpoint_id),
                        WireValue::String("approve".to_string()),
                    ],
                )?)
                .block_task()
                .await?;
            let resumed: FeatureOperationResponse = serde_json::from_value(resumed)
                .map_err(|error| RpcError::internal_error().data(error.to_string()))?;
            assert!(resumed
                .value
                .into_json()
                .is_ok_and(|value| value.get("outcome") == Some(&serde_json::json!("completed"))));
            let tool_names = invoke_source_operation(
                &connection,
                agent_handle.clone(),
                "echo_core::agent::Agent::tool_names",
                vec![WireValue::Handle(session.session.clone())],
            )
            .await?;
            let WireValue::List(tool_names) = tool_names.value else {
                return Err(RpcError::internal_error().data("tool_names was not a list"));
            };
            for expected in ["web_search", "shell"] {
                assert!(
                    tool_names
                        .iter()
                        .any(|value| matches!(value, WireValue::String(name) if name == expected)),
                    "{expected} tool was not installed: {tool_names:?}"
                );
            }
            invoke_source_operation(
                &connection,
                agent_handle.clone(),
                "echo_agent::agent::react::ReactAgent::set_permission_mode",
                vec![
                    WireValue::Handle(session.session.clone()),
                    WireValue::String("full-auto".to_string()),
                ],
            )
            .await?;
            let discovered = invoke_source_operation(
                &connection,
                agent_handle.clone(),
                "echo_agent::agent::react::ReactAgent::discover_skills",
                vec![
                    WireValue::Handle(session.session.clone()),
                    WireValue::List(vec![WireValue::Path(WirePath::Utf8 {
                        path: skill_dir.display().to_string(),
                    })]),
                ],
            )
            .await?;
            assert!(
                matches!(discovered.value, WireValue::List(ref values)
                    if values == &vec![WireValue::String("allowed".to_string())]),
                "SkillLoadPolicy did not filter discovery: {:?}",
                discovered.value
            );
            let reconciled = invoke_source_operation(
                &connection,
                agent_handle.clone(),
                "echo_agent::agent::react::ReactAgent::reconcile_skill_load_policy",
                vec![WireValue::Handle(session.session.clone())],
            )
            .await?;
            assert!(
                matches!(reconciled.value, WireValue::List(ref values) if values.is_empty()),
                "unchanged SkillLoadPolicy should not remove allowed skills: {:?}",
                reconciled.value
            );
            let prompt_context = invoke_source_operation(
                &connection,
                agent_handle.clone(),
                "echo_execution::skills::external::prompt_exec::PromptContext",
                vec![
                    WireValue::Handle(session.session.clone()),
                    WireValue::String(session_dir.display().to_string()),
                    WireValue::String(session.acp_session_id.clone()),
                    WireValue::List(Vec::new()),
                    WireValue::Null,
                    WireValue::Duration(WireDuration::from_nanos(30_000_000_000)),
                    WireValue::String("local".to_string()),
                    WireValue::Handle(sandbox.clone()),
                ],
            )
            .await?;
            let WireValue::Handle(prompt_context) = prompt_context.value else {
                return Err(RpcError::internal_error().data("no PromptContext resource"));
            };
            let rendered = invoke_source_operation(
                &connection,
                agent_handle,
                "echo_execution::skills::external::prompt_exec::process_skill_content",
                vec![
                    WireValue::Handle(session.session.clone()),
                    WireValue::String("Version: !`echo test`".to_string()),
                    WireValue::Handle(prompt_context),
                ],
            )
            .await?;
            assert!(
                matches!(rendered.value, WireValue::String(ref value) if value.contains("sandbox answer"))
            );
            let prompt = connection
                .send_request(v1::PromptRequest::new(
                    v1::SessionId::new(session.acp_session_id.clone()),
                    vec![v1::ContentBlock::Text(v1::TextContent::new(
                        "use components",
                    ))],
                ))
                .block_task()
                .await?;
            assert_eq!(prompt.stop_reason, v1::StopReason::EndTurn);
            let prompt = connection
                .send_request(v1::PromptRequest::new(
                    v1::SessionId::new(session.acp_session_id),
                    vec![v1::ContentBlock::Text(v1::TextContent::new(
                        "use the shell",
                    ))],
                ))
                .block_task()
                .await?;
            assert_eq!(prompt.stop_reason, v1::StopReason::EndTurn);
            assert!(unregister(&connection, &audit).await?);
            assert!(unregister(&connection, &conversation).await?);
            assert!(unregister(&connection, &run_store).await?);
            assert!(unregister(&connection, &projector).await?);
            assert!(unregister(&connection, &guard).await?);
            assert!(unregister(&connection, &search).await?);
            assert!(unregister(&connection, &sandbox).await?);
            assert!(unregister(&connection, &intent).await?);
            assert!(unregister(&connection, &skill_policy).await?);
            assert!(unregister(&connection, &checkpoint_store).await?);
            Ok::<_, RpcError>(())
        })
    })
    .await;
    assert!(
        outcome.is_ok(),
        "component scenario failed: {outcome:?}; stderr: {}",
        stderr_text(&host)
    );
    assert!(
        dispatch.reentrant_conflicts.load(Ordering::Acquire) > 0,
        "same-Session callback mutation did not return extension_conflict"
    );
    let operations = dispatch
        .component_operations
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    assert!(
        operations
            .iter()
            .any(|operation| operation.contains("AuditLogger:AuditLog")),
        "AuditLogger was not consumed: {operations:?}"
    );
    assert!(
        operations
            .iter()
            .any(|operation| operation.contains("ContextProjector:ContextProject")),
        "ContextProjector was not consumed: {operations:?}"
    );
    for expected in [
        "Guard:GuardCheck",
        "SearchProvider:SearchProviderSearch",
        "SandboxExecutor:SandboxExecute",
        "IntentClassifier:IntentClassify",
        "SkillLoadPolicy:SkillLoadAllows",
        "ConversationStore:ConversationEnsure",
        "RunStore:RunAppendEvent",
        "WorkflowCheckpointStore:WorkflowCheckpointSave",
        "WorkflowCheckpointStore:WorkflowCheckpointSaveIfGeneration",
        "WorkflowCheckpointStore:WorkflowCheckpointClaim",
        "WorkflowCheckpointStore:WorkflowCheckpointRenewClaim",
        "WorkflowCheckpointStore:WorkflowCheckpointRequeueClaim",
        "WorkflowCheckpointStore:WorkflowCheckpointAckClaim",
    ] {
        assert!(
            operations
                .iter()
                .any(|operation| operation.contains(expected)),
            "{expected} was not consumed: {operations:?}"
        );
    }
    assert!(
        dispatch.checkpoints.renewals.load(Ordering::Acquire) > 0,
        "remote checkpoint claim was not renewed: {operations:?}"
    );
    host.child.kill().await?;
    Ok(())
}

#[tokio::test]
async fn mcp_transport_is_closed_when_handle_publication_exceeds_quota()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let state_root = directory.path().join("state");
    let model = start_scripted_model(vec![final_script("unused")]).await?;
    let config = write_config(
        directory.path(),
        &format!("http://{model}/v1/chat/completions"),
        &state_root,
    );
    set_profile_limit(&config, "max_facade_resources", 1)?;
    let mut host = spawn_host(&config).await?;
    let dispatch = Arc::new(SdkDispatch::default());
    let outcome = drive_sdk(&mut host, dispatch.clone(), move |connection| {
        Box::pin(async move {
            connection
                .send_request(initialize_request(Some(client_hello())))
                .block_task()
                .await?;
            let transport = register_extension(
                &connection,
                ExtensionKind::AgentComponent,
                "sdk-mcp-transport",
                ExtensionDescriptor::AgentComponent {
                    descriptor_version: 1,
                    component: AgentComponentKindWire::McpTransport,
                    name: "sdk-mcp-transport".to_string(),
                    capabilities: AgentComponentCapabilitiesWire::default(),
                },
                None,
            )
            .await?;
            let agent = connection
                .send_request(AgentCreateRequest {
                    config: AgentConfigWire::HostDefault,
                    idempotency_id: Some("mcp-publication-agent".to_string()),
                })
                .block_task()
                .await?
                .agent;
            let session = connection
                .send_request(SessionCreateRequest {
                    agent: agent.clone(),
                    working_dir: None,
                    session_id: None,
                    idempotency_id: Some("mcp-publication-session".to_string()),
                })
                .block_task()
                .await?;
            let family_call =
                |family: &str, method: &str, operation: &str, arguments: Vec<WireValue>| {
                    UntypedMessage::new(
                        method,
                        serde_json::to_value(FeatureOperationRequest {
                            operation: operation.to_string(),
                            signature_digest:
                                echo_sdk_protocol::facade::family_operation_signature_digest(
                                    family, operation,
                                ),
                            handle: Some(session.session.clone()),
                            arguments,
                        })
                        .map_err(|error| RpcError::internal_error().data(error.to_string()))?,
                    )
                };
            let state = connection
                .send_request(family_call(
                    "workflow",
                    "_echo_agent/workflow/op",
                    "workflow.state.new",
                    Vec::new(),
                )?)
                .block_task()
                .await?;
            let state: FeatureOperationResponse = serde_json::from_value(state)
                .map_err(|error| RpcError::internal_error().data(error.to_string()))?;
            let state = state
                .value
                .into_json()
                .map_err(|error| RpcError::internal_error().data(error.to_string()))?;
            let state_resource: WireHandle =
                serde_json::from_value(state.get("resource").cloned().ok_or_else(|| {
                    RpcError::internal_error().data("workflow state resource missing")
                })?)
                .map_err(|error| RpcError::internal_error().data(error.to_string()))?;

            let arguments = vec![
                WireValue::Handle(session.session.clone()),
                WireValue::String("quota-mcp".to_string()),
                WireValue::Handle(transport.clone()),
            ];
            let failed = invoke_source_operation(
                &connection,
                agent.clone(),
                "echo_integration::mcp::client::McpClient::from_transport",
                arguments.clone(),
            )
            .await;
            let Err(error) = failed else {
                return Err(RpcError::internal_error()
                    .data("MCP publication unexpectedly succeeded at the resource limit"));
            };
            let typed = EchoSdkError::from_jsonrpc_data(error.data.as_ref())
                .map_err(|decode| RpcError::internal_error().data(decode.to_string()))?;
            assert_eq!(typed.code, ExtensionErrorCode::PayloadTooLarge);

            connection
                .send_request(family_call(
                    "invoke",
                    "_echo_agent/facade/invoke",
                    "facade.resource.close",
                    vec![WireValue::Handle(state_resource)],
                )?)
                .block_task()
                .await?;
            let reopened = invoke_source_operation(
                &connection,
                agent,
                "echo_integration::mcp::client::McpClient::from_transport",
                arguments,
            )
            .await?;
            let WireValue::Handle(client) = reopened.value else {
                return Err(RpcError::internal_error().data("MCP client resource missing"));
            };
            connection
                .send_request(family_call(
                    "invoke",
                    "_echo_agent/facade/invoke",
                    "facade.resource.close",
                    vec![WireValue::Handle(client)],
                )?)
                .block_task()
                .await?;
            assert!(unregister(&connection, &transport).await?);
            Ok::<_, RpcError>(())
        })
    })
    .await;
    assert!(
        outcome.is_ok(),
        "MCP publication rollback scenario failed: {outcome:?}; stderr: {}",
        stderr_text(&host)
    );
    let operations = dispatch
        .component_operations
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    let closes = operations
        .iter()
        .filter(|operation| operation.contains("McpTransport:McpTransportClose"))
        .count();
    assert_eq!(
        closes, 2,
        "failed publication and explicit resource close must each close the transport: {operations:?}"
    );
    host.child.kill().await?;
    Ok(())
}

#[cfg(feature = "framework-shell")]
#[tokio::test]
async fn sandbox_cancel_bridge_waits_for_cleanup_and_preserves_cancelled_terminal()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let state_root = directory.path().join("state");
    let model = start_scripted_model(vec![tool_call_script(
        "run_code",
        r#"{"language":"python","code":"print(1)"}"#,
    )])
    .await?;
    let config = write_config(
        directory.path(),
        &format!("http://{model}/v1/chat/completions"),
        &state_root,
    );
    let mut host = spawn_host(&config).await?;
    let dispatch = Arc::new(SdkDispatch::default());
    dispatch.hang_sandbox_cancel.store(true, Ordering::Release);
    let component_operations = dispatch.component_operations.clone();
    let outcome = drive_sdk(&mut host, dispatch.clone(), move |connection| {
        let component_operations = component_operations.clone();
        Box::pin(async move {
            connection
                .send_request(initialize_request(Some(client_hello())))
                .block_task()
                .await?;
            let sandbox = register_extension(
                &connection,
                ExtensionKind::AgentComponent,
                "sdk-cancellable-sandbox",
                ExtensionDescriptor::AgentComponent {
                    descriptor_version: 1,
                    component: AgentComponentKindWire::SandboxExecutor,
                    name: "sdk-cancellable-sandbox".to_string(),
                    capabilities: AgentComponentCapabilitiesWire {
                        isolation_level: Some("os-sandbox".to_string()),
                        ..AgentComponentCapabilitiesWire::default()
                    },
                },
                None,
            )
            .await?;
            let agent = connection
                .send_request(AgentCreateRequest {
                    config: AgentConfigWire::HostDefault,
                    idempotency_id: Some("sandbox-cancel-agent".to_string()),
                })
                .block_task()
                .await?
                .agent;
            let session = connection
                .send_request(SessionCreateRequest {
                    agent: agent.clone(),
                    working_dir: Some(WirePath::Utf8 {
                        path: directory.path().display().to_string(),
                    }),
                    session_id: None,
                    idempotency_id: Some("sandbox-cancel-session".to_string()),
                })
                .block_task()
                .await?;
            invoke_source_operation(
                &connection,
                agent,
                "echo_agent::agent::react::ReactAgent::set_permission_mode",
                vec![
                    WireValue::Handle(session.session.clone()),
                    WireValue::String("full-auto".to_string()),
                ],
            )
            .await?;
            let started = connection
                .send_request(RunStartRequest {
                    session: session.session,
                    input: RunInput::Chat {
                        text: "run the code".to_string(),
                    },
                    idempotency_id: Some("sandbox-cancel-run".to_string()),
                })
                .block_task()
                .await?;
            tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    if component_operations
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .iter()
                        .any(|operation| {
                            operation.contains("SandboxExecutor:SandboxExecuteWithLimitsAndCancel")
                        })
                    {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .map_err(|_| {
                let operations = component_operations
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .clone();
                RpcError::internal_error()
                    .data(format!("sandbox callback did not start: {operations:?}"))
            })?;
            connection
                .send_request(RunCancelRequest {
                    run: started.run.clone(),
                })
                .block_task()
                .await?;
            let waited = connection
                .send_request(RunWaitRequest {
                    run: started.run,
                    timeout: Some(WireDuration::from_nanos(10_000_000_000)),
                })
                .block_task()
                .await?;
            assert!(
                matches!(waited.terminal, Some(RunTerminal::Cancelled)),
                "sandbox cancellation must preserve the Run cancelled terminal: {:?}",
                waited.terminal
            );
            assert!(
                component_operations
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .iter()
                    .any(|operation| operation.contains("SandboxExecutor:SandboxCleanup")),
                "sandbox cancellation must wait for cleanup"
            );
            assert!(unregister(&connection, &sandbox).await?);
            Ok::<_, RpcError>(())
        })
    })
    .await;
    assert!(
        outcome.is_ok(),
        "sandbox cancellation scenario failed: {outcome:?}; stderr: {}",
        stderr_text(&host)
    );
    host.child.kill().await?;
    Ok(())
}

#[cfg(all(feature = "framework-eval", feature = "framework-improve"))]
#[tokio::test]
async fn eval_and_improve_source_adapters_invoke_agent_factory_lazily()
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
    let workspace = directory.path().join("eval-workspace");
    let outcome = drive_sdk(&mut host, dispatch.clone(), move |connection| {
        let workspace = workspace.clone();
        Box::pin(async move {
            connection
                .send_request(initialize_request(Some(client_hello())))
                .block_task()
                .await?;
            let factory = register_extension(
                &connection,
                ExtensionKind::AgentFactory,
                "sdk-eval-factory",
                ExtensionDescriptor::AgentFactory {
                    descriptor_version: 1,
                },
                None,
            )
            .await?;
            let agent = connection
                .send_request(AgentCreateRequest {
                    config: AgentConfigWire::HostDefault,
                    idempotency_id: Some("eval-factory-agent".to_string()),
                })
                .block_task()
                .await?
                .agent;
            let session = connection
                .send_request(SessionCreateRequest {
                    agent: agent.clone(),
                    working_dir: Some(WirePath::Utf8 {
                        path: workspace.display().to_string(),
                    }),
                    session_id: None,
                    idempotency_id: Some("eval-factory-session".to_string()),
                })
                .block_task()
                .await?;
            let cases = ["case-1", "case-2"]
                .into_iter()
                .map(|id| {
                    WireValue::from_json(serde_json::json!({
                        "id": id,
                        "name": id,
                        "description": "lazy factory",
                        "domain": null,
                        "task": "return SDK output",
                        "project_fixture": null,
                        "success_criteria": {
                            "type": "output_contains",
                            "substring": "SDK"
                        },
                        "constraints": {}
                    }))
                    .map_err(|error| RpcError::invalid_params().data(error.to_string()))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let runner = invoke_source_operation(
                &connection,
                agent.clone(),
                "echo_agent::eval::runner::EvalRunner::new",
                vec![
                    WireValue::Handle(session.session.clone()),
                    WireValue::Path(WirePath::Utf8 {
                        path: workspace.display().to_string(),
                    }),
                ],
            )
            .await?;
            let WireValue::Handle(runner) = runner.value else {
                return Err(RpcError::internal_error().data("no EvalRunner resource"));
            };
            let configured_runner = invoke_source_operation(
                &connection,
                agent.clone(),
                "echo_agent::eval::runner::EvalRunner::timeout_secs",
                vec![
                    WireValue::Handle(session.session.clone()),
                    WireValue::Handle(runner),
                    WireValue::U64(WireU64::from_u64(7)),
                ],
            )
            .await?;
            let WireValue::Handle(runner) = configured_runner.value else {
                return Err(RpcError::internal_error().data("no configured EvalRunner"));
            };
            invoke_source_operation(
                &connection,
                agent.clone(),
                "echo_agent::eval::runner::EvalRunner::run_all",
                vec![
                    WireValue::Handle(session.session.clone()),
                    WireValue::Handle(runner),
                    WireValue::List(cases.clone()),
                    WireValue::Handle(factory.clone()),
                ],
            )
            .await?;
            let improvement = invoke_source_operation(
                &connection,
                agent.clone(),
                "echo_agent::improve::loop::ImprovementLoop::new",
                vec![WireValue::Handle(session.session.clone())],
            )
            .await?;
            let WireValue::Handle(improvement) = improvement.value else {
                return Err(RpcError::internal_error().data("no ImprovementLoop resource"));
            };
            let configured_improvement = invoke_source_operation(
                &connection,
                agent.clone(),
                "echo_agent::improve::loop::ImprovementLoop::max_iterations",
                vec![
                    WireValue::Handle(session.session.clone()),
                    WireValue::Handle(improvement),
                    WireValue::U64(WireU64::from_u64(1)),
                ],
            )
            .await?;
            let WireValue::Handle(improvement) = configured_improvement.value else {
                return Err(RpcError::internal_error().data("no configured ImprovementLoop"));
            };
            invoke_source_operation(
                &connection,
                agent,
                "echo_agent::improve::loop::ImprovementLoop::run",
                vec![
                    WireValue::Handle(session.session),
                    WireValue::Handle(improvement),
                    WireValue::List(cases),
                    WireValue::Handle(factory.clone()),
                    WireValue::Null,
                ],
            )
            .await?;
            assert!(unregister(&connection, &factory).await?);
            Ok::<_, RpcError>(())
        })
    })
    .await;
    assert!(outcome.is_ok(), "lazy factory scenario failed: {outcome:?}");
    let operations = dispatch
        .operations
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    let factory_calls = operations
        .iter()
        .filter(|operation| operation.as_str() == "factory_create_agent")
        .count();
    assert_eq!(
        factory_calls, 4,
        "EvalRunner and early-stopped ImprovementLoop must construct exactly one Agent per executed case"
    );
    host.child.kill().await?;
    Ok(())
}

#[cfg(feature = "framework-shell")]
#[tokio::test]
async fn workflow_agent_component_stream_flows_through_facade_pull_stream()
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
    let outcome = drive_sdk(&mut host, dispatch.clone(), move |connection| {
        Box::pin(async move {
            connection
                .send_request(initialize_request(Some(client_hello())))
                .block_task()
                .await?;
            let workflow = register_extension(
                &connection,
                ExtensionKind::AgentComponent,
                "sdk-stream-workflow",
                ExtensionDescriptor::AgentComponent {
                    descriptor_version: 1,
                    component: AgentComponentKindWire::Workflow,
                    name: "sdk-stream-workflow".to_string(),
                    capabilities: AgentComponentCapabilitiesWire {
                        supports_streaming: true,
                        ..AgentComponentCapabilitiesWire::default()
                    },
                },
                None,
            )
            .await?;
            let sandbox = register_extension(
                &connection,
                ExtensionKind::AgentComponent,
                "sdk-stream-sandbox",
                ExtensionDescriptor::AgentComponent {
                    descriptor_version: 1,
                    component: AgentComponentKindWire::SandboxExecutor,
                    name: "sdk-stream-sandbox".to_string(),
                    capabilities: AgentComponentCapabilitiesWire {
                        isolation_level: Some("process".to_string()),
                        supports_streaming: true,
                        ..AgentComponentCapabilitiesWire::default()
                    },
                },
                None,
            )
            .await?;
            let agent = connection
                .send_request(AgentCreateRequest {
                    config: AgentConfigWire::HostDefault,
                    idempotency_id: Some("workflow-stream-agent".to_string()),
                })
                .block_task()
                .await?
                .agent;
            let session = connection
                .send_request(SessionCreateRequest {
                    agent,
                    working_dir: None,
                    session_id: None,
                    idempotency_id: Some("workflow-stream-session".to_string()),
                })
                .block_task()
                .await?;
            let call = |operation: &str, arguments: Vec<WireValue>| {
                UntypedMessage::new(
                    "_echo_agent/workflow/op",
                    serde_json::to_value(FeatureOperationRequest {
                        operation: operation.to_string(),
                        signature_digest:
                            echo_sdk_protocol::facade::family_operation_signature_digest(
                                "workflow",
                                operation,
                            ),
                        handle: Some(session.session.clone()),
                        arguments,
                    })
                    .map_err(|error| RpcError::internal_error().data(error.to_string()))?,
                )
            };
            let opened = connection
                .send_request(call(
                    "workflow.extension.run_stream",
                    vec![
                        WireValue::Handle(workflow.clone()),
                        WireValue::String("start".to_string()),
                    ],
                )?)
                .block_task()
                .await?;
            let opened: FeatureOperationResponse = serde_json::from_value(opened)
                .map_err(|error| RpcError::internal_error().data(error.to_string()))?;
            let WireValue::Handle(stream) = opened.value else {
                return Err(RpcError::internal_error().data("no workflow stream"));
            };
            for expected in ["item", "complete"] {
                let next = connection
                    .send_request(call(
                        "workflow.stream.next",
                        vec![WireValue::Handle(stream.clone())],
                    )?)
                    .block_task()
                    .await?;
                let next: FeatureOperationResponse = serde_json::from_value(next)
                    .map_err(|error| RpcError::internal_error().data(error.to_string()))?;
                assert!(
                    matches!(next.value, WireValue::Variant { ref variant, .. } if variant == expected),
                    "workflow stream did not yield {expected}: {:?}",
                    next.value
                );
                if expected == "item" {
                    let WireValue::Variant { fields, .. } = &next.value else {
                        return Err(RpcError::internal_error().data("workflow item is not a variant"));
                    };
                    let Some(value) = fields
                        .iter()
                        .find(|field| field.name == "value")
                        .map(|field| &field.value)
                    else {
                        return Err(RpcError::internal_error().data("workflow item has no value"));
                    };
                    assert!(
                        matches!(value, WireValue::Variant { type_id, variant, .. }
                            if type_id == "echo_orchestration::workflow::WorkflowEvent"
                                && variant == "completed"),
                        "extension WorkflowEvent must use the canonical graph stream shape: {value:?}"
                    );
                }
            }
            let shell_call = |operation: &str, arguments: Vec<WireValue>| {
                UntypedMessage::new(
                    "_echo_agent/shell/op",
                    serde_json::to_value(FeatureOperationRequest {
                        operation: operation.to_string(),
                        signature_digest:
                            echo_sdk_protocol::facade::family_operation_signature_digest(
                                "shell",
                                operation.strip_prefix("shell.").unwrap_or(operation),
                            ),
                        handle: Some(session.session.clone()),
                        arguments,
                    })
                    .map_err(|error| RpcError::internal_error().data(error.to_string()))?,
                )
            };
            let command = WireValue::from_json(serde_json::json!({
                "kind": {"Shell": "printf stream"},
                "minimum_isolation": null,
                "working_dir": null,
                "env": {},
                "timeout": {"secs": 30, "nanos": 0},
                "stdin": null
            }))
            .map_err(|error| RpcError::invalid_params().data(error.to_string()))?;
            let opened = connection
                .send_request(shell_call(
                    "shell.sandbox.extension.run_stream",
                    vec![WireValue::Handle(sandbox.clone()), command],
                )?)
                .block_task()
                .await?;
            let opened: FeatureOperationResponse = serde_json::from_value(opened)
                .map_err(|error| RpcError::internal_error().data(error.to_string()))?;
            let WireValue::Handle(stream) = opened.value else {
                return Err(RpcError::internal_error().data("no sandbox stream"));
            };
            for expected in ["item", "complete"] {
                let next = connection
                    .send_request(shell_call(
                        "shell.sandbox.stream.next",
                        vec![WireValue::Handle(stream.clone())],
                    )?)
                    .block_task()
                    .await?;
                let next: FeatureOperationResponse = serde_json::from_value(next)
                    .map_err(|error| RpcError::internal_error().data(error.to_string()))?;
                assert!(
                    matches!(next.value, WireValue::Variant { ref variant, .. } if variant == expected),
                    "sandbox stream did not yield {expected}: {:?}",
                    next.value
                );
            }
            assert!(unregister(&connection, &workflow).await?);
            assert!(unregister(&connection, &sandbox).await?);
            Ok::<_, RpcError>(())
        })
    })
    .await;
    assert!(
        outcome.is_ok(),
        "workflow stream scenario failed: {outcome:?}"
    );
    assert!(
        dispatch
            .operations
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .filter(|operation| operation.as_str() == "agent_component_call_stream")
            .count()
            >= 2
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
    let reentrant_mutation = dispatch.reentrant_mutation.clone();

    let outcome = drive_sdk(&mut host, dispatch.clone(), move |connection| {
        let reentrant_mutation = reentrant_mutation.clone();
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
            let agent = connection
                .send_request(AgentCreateRequest {
                    config: AgentConfigWire::HostDefault,
                    idempotency_id: Some("stream-reentry-agent".to_string()),
                })
                .block_task()
                .await?
                .agent;
            let session = connection
                .send_request(SessionCreateRequest {
                    agent: agent.clone(),
                    working_dir: Some(WirePath::Utf8 {
                        path: directory
                            .path()
                            .canonicalize()
                            .map_err(|error| RpcError::internal_error().data(error.to_string()))?
                            .display()
                            .to_string(),
                    }),
                    session_id: None,
                    idempotency_id: Some("stream-reentry-session".to_string()),
                })
                .block_task()
                .await?;
            *reentrant_mutation
                .lock()
                .unwrap_or_else(|error| error.into_inner()) =
                Some((agent, session.session.clone()));
            let prompt = connection
                .send_request(v1::PromptRequest::new(
                    v1::SessionId::new(session.acp_session_id),
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
    let operations = dispatch
        .operations
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    assert!(
        operations
            .iter()
            .any(|operation| operation == "llm_chat_stream" || operation == "llm_chat"),
        "the model call must route through the bridge; got {operations:?}"
    );
    assert!(
        dispatch.reentrant_conflicts.load(Ordering::Acquire) > 0,
        "stream callback mutation after the initial outcome did not return extension_conflict"
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
    // The Host's internal shutdown bound remains five seconds. Keep a small
    // process-reaping margin here so scheduler/pipe teardown jitter does not
    // race the assertion at the exact internal deadline.
    let status = tokio::time::timeout(Duration::from_secs(10), host.child.wait())
        .await
        .map_err(|error| {
            format!(
                "Host did not exit after owner disconnect: {error}; stderr:\n{}",
                stderr_text(&host)
            )
        })??;
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
