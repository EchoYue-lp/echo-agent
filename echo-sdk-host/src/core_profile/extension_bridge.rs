//! Bidirectional `_echo_agent/extension/*` bridge (supreme plan 06).
//!
//! The bridge connects two directions over one ACP connection:
//!
//! - *Host → SDK reverse invocation*: framework trait proxies acquire a
//!   lease from the shared [`ExtensionInvocationAuthority`], send one typed
//!   [`ExtensionInvokeCall`] through the official `ConnectionTo` transport
//!   and settle exactly once. Deadline, cancellation and disconnect all
//!   settle locally with typed errors; a late response can only be
//!   discarded. There is never a built-in fallback implementation.
//! - *SDK → Host stream delivery*: streaming callbacks answer with the
//!   Host-minted stream handle and deliver chunks through
//!   `_echo_agent/extension/stream` notifications routed to a bounded sink
//!   with monotonic sequences and exactly one terminal.
//!
//! Trait proxies are thin by construction: they convert Rust trait
//! arguments to the canonical per-operation payload selected by
//! kind + operation and restore the result. Framework authorities
//! (ToolManager policy, run terminals, event envelopes, the session
//! registry) stay untouched.

use agent_client_protocol::{Client, ConnectionTo};
use echo_agent::agent::{Agent, AgentEvent, CancellationToken};
use echo_agent::error::ReactError;
use echo_agent::tools::{ToolContext, ToolParameters, ToolResult, ToolStreamEvent};
use futures::StreamExt as _;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::sync::mpsc;

use echo_sdk_protocol::error::{EchoSdkError, ExtensionErrorCode, Retryability};
use echo_sdk_protocol::handle::{HandleKind, WireHandle};
use echo_sdk_protocol::methods::{
    ExtensionDescriptor, ExtensionInvocationContext, ExtensionInvokeCall, ExtensionInvokeOutcome,
    ExtensionKind, ExtensionOperation, ExtensionStreamEvent,
};
use echo_sdk_protocol::scalar::WireValue;

use super::state::CoreProfileState;
use super::wire::sdk_error;

/// Bounded stream-channel capacity for one reverse callback stream. The
/// consumer (the Rust stream proxy) paces delivery; a full channel makes the
/// notification handler wait (never the reader loop — routing is spawned).
const STREAM_CHANNEL_CAPACITY: usize = 128;

// ── Shared bridge state ─────────────────────────────────────────────────────

/// Connection-captured state shared by every proxy of one connection.
pub(crate) struct ExtensionBridgeShared {
    /// The official transport handle, captured from the first extension
    /// handler invocation. Every proxy sends through this one connection.
    connection: OnceLock<ConnectionTo<Client>>,
    /// Live callback stream sinks by stream id.
    streams: Mutex<HashMap<String, Arc<ExtensionStreamSink>>>,
}

impl ExtensionBridgeShared {
    pub(crate) fn new() -> Self {
        Self {
            connection: OnceLock::new(),
            streams: Mutex::new(HashMap::new()),
        }
    }

    /// Capture the connection once. Later calls with the same connection are
    /// no-ops (the stdio Host serves exactly one connection per process).
    pub(crate) fn bind_connection(&self, connection: ConnectionTo<Client>) {
        let _ = self.connection.set(connection);
    }

    fn connection(&self) -> std::result::Result<ConnectionTo<Client>, EchoSdkError> {
        self.connection.get().cloned().ok_or_else(|| {
            sdk_error(
                ExtensionErrorCode::ExtensionDisconnected,
                "extension transport is not bound to a connection",
                Retryability::Never,
                "_echo_agent/extension/invoke",
            )
        })
    }

    fn register_stream(&self, sink: Arc<ExtensionStreamSink>) {
        self.streams
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(sink.stream_id().to_string(), sink);
    }

    fn remove_stream(&self, stream_id: &str) {
        self.streams
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(stream_id);
    }

    /// Route one `_echo_agent/extension/stream` notification. Unknown or
    /// already-terminal streams discard the event with a bounded diagnostic
    /// (late delivery never overrides settled state). Runs on a spawned
    /// task, so the bounded channel send may await consumer pace.
    pub(crate) async fn deliver_stream_event(
        &self,
        event: ExtensionStreamEvent,
    ) -> std::result::Result<(), String> {
        if let Err(reason) = event.validate() {
            return Err(format!("stream event violated the contract: {reason}"));
        }
        let sink = {
            let streams = self
                .streams
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let sink = streams.get(event.stream().id.as_str()).cloned();
            eprintln!(
                "[host] sink lookup {} -> {}",
                event.stream().id,
                sink.is_some()
            );
            sink
        };
        let Some(sink) = sink else {
            tracing::warn!(
                stream = event.stream().id,
                "discarded stream event for an unknown or released stream"
            );
            return Ok(());
        };
        let sequence = event.sequence().to_u64().unwrap_or_default();
        if !sink.admit(sequence, event.is_terminal()) {
            tracing::warn!(
                stream = sink.stream_id(),
                sequence,
                "discarded late or out-of-order extension stream event"
            );
            return Ok(());
        }
        let timeout = Duration::from_secs(30);
        match tokio::time::timeout(timeout, sink.sender().send(event)).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_)) => {
                // Receiver dropped: the consumer finished first. Terminal
                // state stays with the sink; later events are discarded.
                Ok(())
            }
            Err(_) => Err("extension stream consumer stalled past its bound".to_string()),
        }
    }
}

// ── Stream sink ─────────────────────────────────────────────────────────────

/// One live callback stream: bounded mailbox plus the monotonic-sequence and
/// exactly-one-terminal ledger.
pub(crate) struct ExtensionStreamSink {
    stream: WireHandle,
    sender: mpsc::Sender<ExtensionStreamEvent>,
    inner: Mutex<StreamSinkInner>,
}

struct StreamSinkInner {
    last_sequence: u64,
    terminal_seen: bool,
}

impl ExtensionStreamSink {
    fn new(stream: WireHandle) -> (Arc<Self>, mpsc::Receiver<ExtensionStreamEvent>) {
        let (sender, receiver) = mpsc::channel(STREAM_CHANNEL_CAPACITY);
        (
            Arc::new(Self {
                stream,
                sender,
                inner: Mutex::new(StreamSinkInner {
                    last_sequence: 0,
                    terminal_seen: false,
                }),
            }),
            receiver,
        )
    }

    fn stream_id(&self) -> &str {
        &self.stream.id
    }

    fn sender(&self) -> &mpsc::Sender<ExtensionStreamEvent> {
        &self.sender
    }

    /// Sequence/terminal admission. Returns false for events that must be
    /// discarded (non-monotonic, post-terminal, or duplicate terminal).
    fn admit(&self, sequence: u64, is_terminal: bool) -> bool {
        let Ok(mut inner) = self.inner.lock() else {
            return false;
        };
        if inner.terminal_seen || sequence <= inner.last_sequence {
            return false;
        }
        inner.last_sequence = sequence;
        inner.terminal_seen = is_terminal;
        true
    }
}

// ── Bridge handle ───────────────────────────────────────────────────────────

/// Per-call bridge over the shared state. Cheap to construct; proxies hold
/// one for their lifetime. The profile state binds itself after
/// construction (the bridge is created first because the default Agent
/// definition must capture it before any Session exists).
pub(crate) struct ExtensionBridge {
    state: OnceLock<Arc<CoreProfileState>>,
    shared: Arc<ExtensionBridgeShared>,
}

/// Kinds whose callbacks mutate the implementation exclusively: a second
/// in-flight invocation on the same registration is a typed conflict, never
/// a wait (design §12.3).
fn is_exclusive_kind(kind: ExtensionKind) -> bool {
    matches!(
        kind,
        ExtensionKind::HumanLoopProvider
            | ExtensionKind::AgentFactory
            | ExtensionKind::CustomAgent
            | ExtensionKind::Hook
    )
}

fn lease_error(error: echo_agent::acp::ExtensionLeaseError) -> EchoSdkError {
    match error {
        echo_agent::acp::ExtensionLeaseError::AdmissionClosed => sdk_error(
            ExtensionErrorCode::HostShuttingDown,
            "the Host is shutting down and refuses new extension invocations",
            Retryability::Never,
            "_echo_agent/extension/invoke",
        ),
        echo_agent::acp::ExtensionLeaseError::ConcurrencyLimit => sdk_error(
            ExtensionErrorCode::ExtensionRejected,
            "extension callback concurrency limit reached",
            Retryability::AfterDelay,
            "_echo_agent/extension/invoke",
        ),
        echo_agent::acp::ExtensionLeaseError::ExclusiveConflict => sdk_error(
            ExtensionErrorCode::ExtensionConflict,
            "extension is already executing an exclusive invocation",
            Retryability::Never,
            "_echo_agent/extension/invoke",
        ),
    }
}

fn transport_error(error: &agent_client_protocol::Error) -> EchoSdkError {
    if agent_client_protocol::is_incoming_transport_closed(error) {
        sdk_error(
            ExtensionErrorCode::ExtensionDisconnected,
            "the SDK connection closed before the callback answered",
            Retryability::Never,
            "_echo_agent/extension/invoke",
        )
    } else if let Ok(decoded) = EchoSdkError::from_jsonrpc_data(error.data.as_ref()) {
        decoded
    } else {
        sdk_error(
            ExtensionErrorCode::ExtensionFailed,
            format!("reverse invocation failed: {error}"),
            Retryability::Never,
            "_echo_agent/extension/invoke",
        )
    }
}

impl ExtensionBridge {
    pub(crate) fn unbound(shared: Arc<ExtensionBridgeShared>) -> Self {
        Self {
            state: OnceLock::new(),
            shared,
        }
    }

    /// Bind the profile state once; every proxy resolves it per invocation.
    pub(crate) fn bind_state(&self, state: Arc<CoreProfileState>) {
        let _ = self.state.set(state);
    }

    fn state(&self) -> std::result::Result<Arc<CoreProfileState>, EchoSdkError> {
        self.state.get().cloned().ok_or_else(|| {
            sdk_error(
                ExtensionErrorCode::HostShuttingDown,
                "extension bridge is not bound to the profile state",
                Retryability::Never,
                "_echo_agent/extension/invoke",
            )
        })
    }

    fn deadline_of(&self, record: &super::handles::ExtensionRecord) -> Duration {
        record
            .timeout
            .as_ref()
            .and_then(|timeout| {
                timeout.seconds.to_u64().and_then(|seconds| {
                    u32::try_from(seconds)
                        .ok()
                        .map(|seconds| Duration::from_secs(u64::from(seconds)))
                })
            })
            .unwrap_or_else(|| {
                Duration::from_secs(
                    self.state()
                        .map(|state| state.limits.callback_timeout_secs)
                        .unwrap_or_default()
                        .max(1),
                )
            })
    }

    /// Acquire a lease and resolve the extension record through the fixed
    /// admission ladder. Shared by both invoke paths.
    #[allow(clippy::type_complexity)]
    fn prepare(
        &self,
        extension: &WireHandle,
        operation: ExtensionOperation,
        framework_cancellation: CancellationToken,
    ) -> std::result::Result<
        (
            echo_agent::acp::ExtensionInvocationLease,
            Arc<super::handles::ExtensionRecord>,
            Arc<echo_agent::acp::ExtensionInvocationAuthority>,
        ),
        EchoSdkError,
    > {
        const OPERATION: &str = "_echo_agent/extension/invoke";
        let state = self.state()?;
        state
            .handles
            .check_shape_and_generation(extension, HandleKind::Extension, OPERATION)?;
        if let Err(reason) = extension.validate() {
            return Err(sdk_error(
                ExtensionErrorCode::InvalidValue,
                reason,
                Retryability::Never,
                OPERATION,
            ));
        }
        let record = state.handles.extension(extension)?;
        if record.kind != operation.kind() {
            return Err(sdk_error(
                ExtensionErrorCode::InvalidValue,
                format!(
                    "operation {} does not address extension kind {}",
                    operation.as_str(),
                    record.kind.as_str()
                ),
                Retryability::Never,
                OPERATION,
            )
            .with_handle(extension.clone()));
        }
        let services = state.services().map_err(|error| {
            sdk_error(
                ExtensionErrorCode::HostShuttingDown,
                error.to_string(),
                Retryability::Never,
                OPERATION,
            )
        })?;
        let authority = services.extensions().clone();
        let exclusive_key = is_exclusive_kind(record.kind)
            .then(|| format!("{}/{}", extension.id, record.kind.as_str()));
        let lease = authority
            .lease(exclusive_key.as_deref(), framework_cancellation)
            .map_err(lease_error)?;
        Ok((lease, record, authority))
    }

    /// Drive one non-streaming reverse invocation to its single settlement.
    pub(crate) async fn invoke_once(
        &self,
        extension: &WireHandle,
        operation: ExtensionOperation,
        context: Option<ExtensionInvocationContext>,
        input: WireValue,
        framework_cancellation: CancellationToken,
    ) -> std::result::Result<WireValue, EchoSdkError> {
        let connection = self.shared.connection()?;
        let (mut lease, record, _authority) =
            self.prepare(extension, operation, framework_cancellation)?;
        let deadline = self.deadline_of(&record);
        let call = ExtensionInvokeCall {
            extension: extension.clone(),
            invocation_id: lease.identity().to_string(),
            operation,
            context,
            input,
            deadline: wire_duration(deadline),
            stream: None,
        };
        if let Err(reason) = call.validate() {
            return Err(sdk_error(
                ExtensionErrorCode::SerializationViolation,
                reason,
                Retryability::Never,
                "_echo_agent/extension/invoke",
            ));
        }
        let sent = connection.send_request(call);
        let cancellation = lease.cancellation();
        tokio::select! {
            answer = sent.block_task() => {
                match answer {
                    Ok(outcome) => self.settle_outcome(&mut lease, outcome, operation, None),
                    Err(error) => {
                        let typed = transport_error(&error);
                        if typed.code == ExtensionErrorCode::ExtensionDisconnected {
                            lease.settle(echo_agent::acp::ExtensionSettlement::Disconnected);
                        } else {
                            lease.settle(echo_agent::acp::ExtensionSettlement::Answered);
                        }
                        Err(typed)
                    }
                }
            }
            () = cancellation.cancelled() => {
                let _ = self.send_cancel_notice(lease.identity(), "cancelled");
                lease.settle(echo_agent::acp::ExtensionSettlement::Cancelled);
                Err(sdk_error(
                    ExtensionErrorCode::Cancelled,
                    "extension invocation was cancelled",
                    Retryability::Never,
                    "_echo_agent/extension/invoke",
                ))
            }
            _ = tokio::time::sleep(deadline) => {
                let _ = self.send_cancel_notice(lease.identity(), "timeout");
                lease.settle_timeout();
                Err(sdk_error(
                    ExtensionErrorCode::ExtensionTimeout,
                    "extension invocation exceeded its deadline",
                    Retryability::Never,
                    "_echo_agent/extension/invoke",
                ))
            }
        }
    }

    /// Drive one streaming reverse invocation: mint the stream handle,
    /// subscribe the sink, return the bounded receiver the Rust stream
    /// proxy consumes.
    pub(crate) async fn invoke_stream(
        &self,
        extension: &WireHandle,
        operation: ExtensionOperation,
        context: Option<ExtensionInvocationContext>,
        input: WireValue,
        framework_cancellation: CancellationToken,
    ) -> std::result::Result<(mpsc::Receiver<ExtensionStreamEvent>, WireHandle), EchoSdkError> {
        const OPERATION_STR: &str = "_echo_agent/extension/invoke";
        debug_assert!(operation.is_streaming());
        let connection = self.shared.connection()?;
        let (mut lease, record, _authority) =
            self.prepare(extension, operation, framework_cancellation)?;
        let deadline = self.deadline_of(&record);
        let stream = self
            .state()?
            .handles
            .register_extension_stream(&extension.id)?;
        let (sink, receiver) = ExtensionStreamSink::new(stream.clone());
        self.shared.register_stream(sink);
        let call = ExtensionInvokeCall {
            extension: extension.clone(),
            invocation_id: lease.identity().to_string(),
            operation,
            context,
            input,
            deadline: wire_duration(deadline),
            stream: Some(stream.clone()),
        };
        if let Err(reason) = call.validate() {
            self.release_stream(&stream);
            return Err(sdk_error(
                ExtensionErrorCode::SerializationViolation,
                reason,
                Retryability::Never,
                OPERATION_STR,
            ));
        }
        let sent = connection.send_request(call);
        let cancellation = lease.cancellation();
        let outcome = tokio::select! {
            answer = sent.block_task() => {
                match answer {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        let typed = transport_error(&error);
                        if typed.code == ExtensionErrorCode::ExtensionDisconnected {
                            lease.settle(echo_agent::acp::ExtensionSettlement::Disconnected);
                        } else {
                            lease.settle(echo_agent::acp::ExtensionSettlement::Answered);
                        }
                        self.release_stream(&stream);
                        return Err(typed);
                    }
                }
            }
            () = cancellation.cancelled() => {
                let _ = self.send_cancel_notice(lease.identity(), "cancelled");
                lease.settle(echo_agent::acp::ExtensionSettlement::Cancelled);
                self.release_stream(&stream);
                return Err(sdk_error(
                    ExtensionErrorCode::Cancelled,
                    "extension stream invocation was cancelled",
                    Retryability::Never,
                    OPERATION_STR,
                ));
            }
            _ = tokio::time::sleep(deadline) => {
                let _ = self.send_cancel_notice(lease.identity(), "timeout");
                lease.settle_timeout();
                self.release_stream(&stream);
                return Err(sdk_error(
                    ExtensionErrorCode::ExtensionTimeout,
                    "extension stream invocation exceeded its deadline",
                    Retryability::Never,
                    OPERATION_STR,
                ));
            }
        };
        match self.settle_outcome(&mut lease, outcome, operation, Some(&stream)) {
            Ok(_value) => Ok((receiver, stream)),
            Err(error) => {
                self.release_stream(&stream);
                Err(error)
            }
        }
    }

    /// Validate and settle one answer. `expected_stream` enforces the
    /// echoed stream handle for streaming operations.
    fn settle_outcome(
        &self,
        lease: &mut echo_agent::acp::ExtensionInvocationLease,
        outcome: ExtensionInvokeOutcome,
        operation: ExtensionOperation,
        expected_stream: Option<&WireHandle>,
    ) -> std::result::Result<WireValue, EchoSdkError> {
        lease.settle(echo_agent::acp::ExtensionSettlement::Answered);
        if let Err(reason) = outcome.validate() {
            return Err(sdk_error(
                ExtensionErrorCode::SerializationViolation,
                reason,
                Retryability::Never,
                "_echo_agent/extension/invoke",
            ));
        }
        match outcome {
            ExtensionInvokeOutcome::Result { value } => {
                if operation.is_streaming() {
                    return Err(sdk_error(
                        ExtensionErrorCode::ExtensionFailed,
                        "streaming operation answered with a single result",
                        Retryability::Never,
                        "_echo_agent/extension/invoke",
                    ));
                }
                Ok(value)
            }
            ExtensionInvokeOutcome::Stream { stream } => {
                let Some(expected) = expected_stream else {
                    return Err(sdk_error(
                        ExtensionErrorCode::ExtensionFailed,
                        "non-streaming operation answered with a stream",
                        Retryability::Never,
                        "_echo_agent/extension/invoke",
                    ));
                };
                if stream.id != expected.id {
                    return Err(sdk_error(
                        ExtensionErrorCode::ExtensionFailed,
                        "stream outcome did not echo the Host-minted stream handle",
                        Retryability::Never,
                        "_echo_agent/extension/invoke",
                    ));
                }
                Ok(WireValue::Null)
            }
            ExtensionInvokeOutcome::Error { error } => Err(error),
        }
    }

    fn release_stream(&self, stream: &WireHandle) {
        self.shared.remove_stream(&stream.id);
        if let Ok(state) = self.state() {
            state.handles.remove_extension_stream(&stream.id);
        }
    }

    /// Best-effort typed cancel notice; the official `$/cancel_request` is
    /// sent automatically when the `SentRequest` handle drops.
    fn send_cancel_notice(&self, invocation_id: &str, reason: &str) -> Result<(), String> {
        let notice = echo_sdk_protocol::methods::ExtensionCancelNotice {
            invocation_id: invocation_id.to_string(),
            reason: reason.to_string(),
        };
        if let Err(reason) = notice.validate() {
            return Err(reason.to_string());
        }
        let connection = self.shared.connection().map_err(|error| error.message)?;
        connection
            .send_notification(notice)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

fn wire_duration(duration: Duration) -> echo_sdk_protocol::scalar::WireDuration {
    echo_sdk_protocol::scalar::WireDuration::from_nanos(
        duration.as_nanos().try_into().unwrap_or(u64::MAX),
    )
}

// ── Wire value conversions ──────────────────────────────────────────────────

fn to_wire(value: impl serde::Serialize) -> std::result::Result<WireValue, String> {
    let json = serde_json::to_value(value).map_err(|error| error.to_string())?;
    WireValue::from_json(json).map_err(|error| error.to_string())
}

fn from_wire<T: serde::de::DeserializeOwned>(value: WireValue) -> std::result::Result<T, String> {
    let json = value.into_json().map_err(|error| error.to_string())?;
    serde_json::from_value(json).map_err(|error| error.to_string())
}

/// Build the callback-event stream that ends exactly at the terminal.
///
/// The sink keeps its bounded `Sender` registered for late-delivery
/// admission, so the raw receiver would never close on its own: ending at
/// the terminal (and releasing the stream resources then) is what makes the
/// exactly-one-terminal contract observable to the Rust consumer.
fn extension_event_stream(
    bridge: Arc<ExtensionBridge>,
    stream: WireHandle,
    receiver: mpsc::Receiver<ExtensionStreamEvent>,
) -> impl futures::Stream<Item = ExtensionStreamEvent> {
    futures::stream::unfold(Some((bridge, stream, receiver)), |state| async move {
        let (bridge, stream, mut receiver) = state?;
        match receiver.recv().await {
            Some(event) => {
                if event.is_terminal() {
                    bridge.release_stream(&stream);
                    Some((event, None))
                } else {
                    Some((event, Some((bridge, stream, receiver))))
                }
            }
            None => {
                bridge.release_stream(&stream);
                None
            }
        }
    })
}

fn react_error(error: EchoSdkError) -> ReactError {
    ReactError::Other(format!(
        "extension bridge failure {}: {}",
        error.code.as_str(),
        error.message
    ))
}

fn cancelled_token() -> CancellationToken {
    CancellationToken::new()
}

fn cancelled_token_arc() -> Arc<CancellationToken> {
    Arc::new(CancellationToken::new())
}

// ── Tool proxy ──────────────────────────────────────────────────────────────

/// Serializable projection of [`ToolContext`]: the identity facts a callback
/// needs. Closures, sinks and guards never cross the wire; cancellation is
/// carried by the invocation envelope, not the context.
#[derive(serde::Serialize, serde::Deserialize)]
struct ToolContextWire {
    #[serde(skip_serializing_if = "Option::is_none")]
    working_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    conversation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    turn_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    execution_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    call_id: Option<String>,
}

impl ToolContextWire {
    fn of(context: &ToolContext) -> Self {
        Self {
            working_dir: context
                .working_dir
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            conversation_id: context.conversation_id.clone(),
            run_id: context.run_id.clone(),
            turn_id: context.turn_id.clone(),
            message_id: context.message_id.clone(),
            execution_id: context.execution_id.clone(),
            call_id: context.call_id.clone(),
        }
    }
}

/// Thin `Tool` proxy: descriptor facts answered locally, execution and
/// streaming delegated to the registered implementation. The ToolManager
/// keeps owning permissions, retry and sandbox policy.
pub(crate) struct ExtensionToolProxy {
    bridge: Arc<ExtensionBridge>,
    extension: WireHandle,
    descriptor: ExtensionDescriptor,
}

impl ExtensionToolProxy {
    pub(crate) fn new(bridge: Arc<ExtensionBridge>, extension: WireHandle) -> Option<Self> {
        let descriptor = bridge
            .state()
            .ok()?
            .handles
            .extension(&extension)
            .ok()?
            .descriptor
            .clone();
        matches!(descriptor, ExtensionDescriptor::Tool { .. }).then(|| Self {
            bridge,
            extension,
            descriptor,
        })
    }

    fn as_tool(&self) -> (String, String, serde_json::Value, u64, bool) {
        let ExtensionDescriptor::Tool {
            name,
            description,
            parameters,
            schema_revision,
            supports_streaming,
            ..
        } = &self.descriptor
        else {
            return (
                String::new(),
                String::new(),
                serde_json::Value::Null,
                0,
                false,
            );
        };
        let parameters = parameters
            .clone()
            .into_json()
            .unwrap_or(serde_json::Value::Null);
        (
            name.clone(),
            description.clone(),
            parameters,
            schema_revision.to_u64().unwrap_or_default(),
            *supports_streaming,
        )
    }

    async fn execute_over(
        &self,
        operation: ExtensionOperation,
        parameters: ToolParameters,
        context: Option<&ToolContext>,
    ) -> echo_agent::error::Result<ToolResult> {
        let input = serde_json::json!({
            "parameters": parameters,
            "context": context.map(ToolContextWire::of),
        });
        let value = self
            .bridge
            .invoke_once(
                &self.extension,
                operation,
                None,
                to_wire(input).map_err(ReactError::Other)?,
                context
                    .and_then(|context| context.cancel.clone())
                    .unwrap_or_else(cancelled_token_arc)
                    .as_ref()
                    .clone(),
            )
            .await
            .map_err(react_error)?;
        from_wire(value).map_err(ReactError::Other)
    }
}

impl echo_agent::tools::Tool for ExtensionToolProxy {
    fn name(&self) -> &str {
        match &self.descriptor {
            ExtensionDescriptor::Tool { name, .. } => name,
            _ => "",
        }
    }

    fn description(&self) -> &str {
        match &self.descriptor {
            ExtensionDescriptor::Tool { description, .. } => description,
            _ => "",
        }
    }

    fn parameters(&self) -> serde_json::Value {
        self.as_tool().2
    }

    fn schema_revision(&self) -> u64 {
        self.as_tool().3
    }

    fn supports_streaming(&self) -> bool {
        self.as_tool().4
    }

    fn execute_with_context<'a>(
        &'a self,
        parameters: ToolParameters,
        context: &'a ToolContext,
    ) -> futures::future::BoxFuture<'a, echo_agent::error::Result<ToolResult>> {
        Box::pin(async move {
            self.execute_over(ExtensionOperation::ToolExecute, parameters, Some(context))
                .await
        })
    }

    fn execute_stream_with_context<'a>(
        &'a self,
        parameters: ToolParameters,
        context: &ToolContext,
    ) -> futures::future::BoxFuture<
        'a,
        echo_agent::error::Result<
            std::pin::Pin<Box<dyn futures::Stream<Item = ToolStreamEvent> + Send + 'a>>,
        >,
    > {
        let context = context.clone();
        Box::pin(async move {
            let input = serde_json::json!({
                "parameters": parameters,
                "context": ToolContextWire::of(&context),
            });
            let cancellation = context
                .cancel
                .clone()
                .unwrap_or_else(cancelled_token_arc)
                .as_ref()
                .clone();
            let (receiver, stream) = self
                .bridge
                .invoke_stream(
                    &self.extension,
                    ExtensionOperation::ToolExecuteStream,
                    None,
                    to_wire(input).map_err(ReactError::Other)?,
                    cancellation,
                )
                .await
                .map_err(react_error)?;
            let stream_clone = stream.clone();
            let _ = stream;
            Ok(Box::pin(futures::StreamExt::map(
                extension_event_stream(self.bridge.clone(), stream_clone, receiver),
                |event: ExtensionStreamEvent| match event {
                    ExtensionStreamEvent::Chunk { value, .. }
                    | ExtensionStreamEvent::Complete { value, .. } => {
                        from_wire(value).unwrap_or(ToolStreamEvent::Complete(ToolResult::error(
                            "stream chunk could not be decoded",
                        )))
                    }
                    ExtensionStreamEvent::Failed { error, .. } => {
                        ToolStreamEvent::Complete(ToolResult::error(&error.message))
                    }
                    ExtensionStreamEvent::Cancelled { .. } => {
                        ToolStreamEvent::Complete(ToolResult::error("stream was cancelled"))
                    }
                },
            ))
                as std::pin::Pin<
                    Box<dyn futures::Stream<Item = ToolStreamEvent> + Send>,
                >)
        })
    }

    fn validate_parameters<'a>(
        &'a self,
        parameters: &'a ToolParameters,
    ) -> futures::future::BoxFuture<'a, echo_agent::error::Result<()>> {
        Box::pin(async move {
            let input = serde_json::json!({"parameters": parameters});
            let value = self
                .bridge
                .invoke_once(
                    &self.extension,
                    ExtensionOperation::ToolValidateParameters,
                    None,
                    to_wire(input).map_err(ReactError::Other)?,
                    cancelled_token(),
                )
                .await
                .map_err(react_error)?;
            // The validation contract is `null` when the parameters are
            // valid and a bounded rejection reason otherwise.
            match from_wire::<Option<String>>(value).map_err(ReactError::Other)? {
                Some(reason) => Err(ReactError::Other(reason)),
                None => Ok(()),
            }
        })
    }
}

// ── LlmClient proxy ─────────────────────────────────────────────────────────

/// Wire projection of a chat request: everything except the cancellation
/// token and timeout handle, which live in the invocation envelope.
#[derive(serde::Serialize, serde::Deserialize)]
struct ChatRequestWire {
    messages: Vec<echo_agent::llm::types::Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<echo_agent::llm::types::ToolDefinition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    user_id: Option<String>,
}

/// Wire projection of one stream chunk. `DeltaMessage` is deserialize-only
/// in the framework, so the chunk is projected field-wise.
#[derive(serde::Serialize, serde::Deserialize, Default)]
struct ChatChunkWire {
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<DeltaToolCallWire>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    finish_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    usage: Option<echo_agent::llm::types::Usage>,
}

/// Wire projection of one streaming tool-call fragment. The framework type
/// is deserialize-only, so the projection is field-wise.
#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct DeltaToolCallWire {
    index: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    call_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    function: Option<DeltaFunctionCallWire>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Default)]
struct DeltaFunctionCallWire {
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    arguments: Option<String>,
}

impl DeltaToolCallWire {
    #[cfg(test)]
    fn of(delta: &echo_agent::llm::types::DeltaToolCall) -> Self {
        Self {
            index: delta.index,
            id: delta.id.clone(),
            call_type: delta.call_type.clone(),
            function: delta
                .function
                .as_ref()
                .map(|function| DeltaFunctionCallWire {
                    name: function.name.clone(),
                    arguments: function.arguments.clone(),
                }),
        }
    }

    fn into_delta(self) -> echo_agent::llm::types::DeltaToolCall {
        echo_agent::llm::types::DeltaToolCall {
            index: self.index,
            id: self.id,
            call_type: self.call_type,
            function: self
                .function
                .map(|function| echo_agent::llm::types::DeltaFunctionCall {
                    name: function.name,
                    arguments: function.arguments,
                }),
        }
    }
}

impl ChatChunkWire {
    #[cfg(test)]
    fn from_chunk(chunk: &echo_agent::llm::ChatChunk) -> Self {
        Self {
            role: chunk.delta.role.clone(),
            content: chunk.delta.content.clone(),
            reasoning_content: chunk.delta.reasoning_content.clone(),
            tool_calls: chunk
                .delta
                .tool_calls
                .as_ref()
                .map(|calls| calls.iter().map(DeltaToolCallWire::of).collect()),
            finish_reason: chunk.finish_reason.clone(),
            usage: chunk.usage.clone(),
        }
    }

    fn into_chunk(self) -> echo_agent::llm::ChatChunk {
        let delta = echo_agent::llm::types::DeltaMessage {
            role: self.role,
            content: self.content,
            reasoning_content: self.reasoning_content,
            reasoning_blocks: None,
            tool_calls: self
                .tool_calls
                .map(|calls| calls.into_iter().map(|wire| wire.into_delta()).collect()),
        };
        echo_agent::llm::ChatChunk {
            delta,
            finish_reason: self.finish_reason,
            usage: self.usage,
        }
    }
}

/// Wire projection of a chat response. `ChatResponse` itself carries no
/// serde derives, so the projection is field-wise over serde-able members.
#[derive(serde::Serialize, serde::Deserialize)]
struct ChatResponseWire {
    message: echo_agent::llm::types::Message,
    #[serde(skip_serializing_if = "Option::is_none")]
    finish_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    usage: Option<echo_agent::llm::types::Usage>,
    #[serde(default)]
    raw: echo_agent::llm::types::ChatCompletionResponse,
}

impl ChatResponseWire {
    fn into_response(self) -> echo_agent::llm::ChatResponse {
        echo_agent::llm::ChatResponse {
            message: self.message,
            finish_reason: self.finish_reason,
            usage: self.usage,
            raw: self.raw,
        }
    }
}

/// Thin `LlmClient` proxy: chat and chat_stream delegate to the registered
/// implementation; the descriptor answers model identity locally. The
/// AgentTurnDriver keeps deciding run terminals from the returned chunks.
pub(crate) struct ExtensionLlmClientProxy {
    bridge: Arc<ExtensionBridge>,
    extension: WireHandle,
    model_name: String,
}

impl ExtensionLlmClientProxy {
    pub(crate) fn new(bridge: Arc<ExtensionBridge>, extension: WireHandle) -> Option<Self> {
        let record = bridge.state().ok()?.handles.extension(&extension).ok()?;
        let ExtensionDescriptor::LlmClient { model_name, .. } = &record.descriptor else {
            return None;
        };
        Some(Self {
            bridge,
            extension,
            model_name: model_name.clone(),
        })
    }

    fn request_wire(request: &echo_agent::llm::ChatRequest) -> ChatRequestWire {
        ChatRequestWire {
            messages: request.messages.clone(),
            temperature: request.temperature.map(f64::from),
            max_tokens: request.max_tokens,
            tools: request.tools.clone(),
            tool_choice: request.tool_choice.clone(),
            user_id: request.user_id.clone(),
        }
    }
}

impl echo_agent::llm::LlmClient for ExtensionLlmClientProxy {
    fn chat(
        &self,
        request: echo_agent::llm::ChatRequest,
    ) -> futures::future::BoxFuture<'_, echo_agent::error::Result<echo_agent::llm::ChatResponse>>
    {
        Box::pin(async move {
            let cancellation = request.cancel_token.clone().unwrap_or_else(cancelled_token);
            let value = self
                .bridge
                .invoke_once(
                    &self.extension,
                    ExtensionOperation::LlmChat,
                    None,
                    to_wire(Self::request_wire(&request)).map_err(ReactError::Other)?,
                    cancellation,
                )
                .await
                .map_err(react_error)?;
            let wire: ChatResponseWire = from_wire(value).map_err(ReactError::Other)?;
            Ok(wire.into_response())
        })
    }

    fn chat_stream(
        &self,
        request: echo_agent::llm::ChatRequest,
    ) -> futures::future::BoxFuture<
        '_,
        echo_agent::error::Result<
            futures::stream::BoxStream<
                'static,
                echo_agent::error::Result<echo_agent::llm::ChatChunk>,
            >,
        >,
    > {
        Box::pin(async move {
            let cancellation = request.cancel_token.clone().unwrap_or_else(cancelled_token);
            let (receiver, stream) = self
                .bridge
                .invoke_stream(
                    &self.extension,
                    ExtensionOperation::LlmChatStream,
                    None,
                    to_wire(Self::request_wire(&request)).map_err(ReactError::Other)?,
                    cancellation,
                )
                .await
                .map_err(react_error)?;
            Ok(futures::StreamExt::map(
                extension_event_stream(self.bridge.clone(), stream, receiver),
                |event: ExtensionStreamEvent| {
                    let result: echo_agent::error::Result<echo_agent::llm::ChatChunk> = match event
                    {
                        ExtensionStreamEvent::Chunk { value, .. }
                        | ExtensionStreamEvent::Complete { value, .. } => {
                            from_wire::<ChatChunkWire>(value)
                                .map(|chunk| chunk.into_chunk())
                                .map_err(ReactError::Other)
                        }
                        ExtensionStreamEvent::Failed { error, .. } => Err(react_error(error)),
                        ExtensionStreamEvent::Cancelled { .. } => {
                            Err(ReactError::Other("llm stream was cancelled".to_string()))
                        }
                    };
                    result
                },
            )
            .boxed())
        })
    }

    fn model_name(&self) -> &str {
        &self.model_name
    }
}

// ── Store proxy ─────────────────────────────────────────────────────────────

/// Wire projection of a search query; search modes stay explicit so
/// semantic/hybrid search never silently degrades to keyword.
#[derive(serde::Serialize, serde::Deserialize)]
struct SearchQueryWire {
    text: String,
    limit: usize,
    mode: SearchModeWireKind,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum SearchModeWireKind {
    Keyword,
    Semantic,
    Hybrid {
        #[serde(skip_serializing_if = "Option::is_none")]
        vector_weight: Option<f32>,
    },
}

/// Thin `Store` proxy over the six store operations. Semantic or hybrid
/// searches against an implementation that did not declare them fail before
/// any callback leaves the process.
pub(crate) struct ExtensionStoreProxy {
    bridge: Arc<ExtensionBridge>,
    extension: WireHandle,
    declared_modes: Vec<echo_sdk_protocol::methods::SearchModeWire>,
}

impl ExtensionStoreProxy {
    pub(crate) fn new(bridge: Arc<ExtensionBridge>, extension: WireHandle) -> Option<Self> {
        let record = bridge.state().ok()?.handles.extension(&extension).ok()?;
        let ExtensionDescriptor::Store { search_modes, .. } = &record.descriptor else {
            return None;
        };
        Some(Self {
            bridge,
            extension,
            declared_modes: search_modes.clone(),
        })
    }

    fn declares(&self, mode: echo_sdk_protocol::methods::SearchModeWire) -> bool {
        self.declared_modes.is_empty() || self.declared_modes.contains(&mode)
    }

    async fn call_op(
        &self,
        operation: ExtensionOperation,
        payload: impl serde::Serialize,
    ) -> echo_agent::error::Result<WireValue> {
        self.bridge
            .invoke_once(
                &self.extension,
                operation,
                None,
                to_wire(payload).map_err(ReactError::Other)?,
                cancelled_token(),
            )
            .await
            .map_err(react_error)
    }
}

impl echo_agent::memory::Store for ExtensionStoreProxy {
    fn put<'a>(
        &'a self,
        namespace: &'a [&'a str],
        key: &'a str,
        value: serde_json::Value,
    ) -> futures::future::BoxFuture<'a, echo_agent::error::Result<()>> {
        Box::pin(async move {
            let value = self
                .call_op(
                    ExtensionOperation::StorePut,
                    serde_json::json!({"namespace": namespace, "key": key, "value": value}),
                )
                .await?;
            let _ = value;
            Ok(())
        })
    }

    fn get<'a>(
        &'a self,
        namespace: &'a [&'a str],
        key: &'a str,
    ) -> futures::future::BoxFuture<
        'a,
        echo_agent::error::Result<Option<echo_agent::memory::StoreItem>>,
    > {
        Box::pin(async move {
            let value = self
                .call_op(
                    ExtensionOperation::StoreGet,
                    serde_json::json!({"namespace": namespace, "key": key}),
                )
                .await?;
            from_wire(value).map_err(ReactError::Other)
        })
    }

    fn search<'a>(
        &'a self,
        namespace: &'a [&'a str],
        query: &'a str,
        limit: usize,
    ) -> futures::future::BoxFuture<'a, echo_agent::error::Result<Vec<echo_agent::memory::StoreItem>>>
    {
        Box::pin(async move {
            let value = self
                .call_op(
                    ExtensionOperation::StoreSearch,
                    serde_json::json!({"namespace": namespace, "query": query, "limit": limit}),
                )
                .await?;
            from_wire(value).map_err(ReactError::Other)
        })
    }

    fn search_with<'a>(
        &'a self,
        namespace: &'a [&'a str],
        query: echo_agent::memory::SearchQuery<'a>,
    ) -> futures::future::BoxFuture<'a, echo_agent::error::Result<Vec<echo_agent::memory::StoreItem>>>
    {
        Box::pin(async move {
            let (mode, wire_mode) = match &query.mode {
                echo_agent::memory::SearchMode::Keyword => (
                    echo_sdk_protocol::methods::SearchModeWire::Keyword,
                    SearchModeWireKind::Keyword,
                ),
                echo_agent::memory::SearchMode::Semantic => (
                    echo_sdk_protocol::methods::SearchModeWire::Semantic,
                    SearchModeWireKind::Semantic,
                ),
                echo_agent::memory::SearchMode::Hybrid { vector_weight } => (
                    echo_sdk_protocol::methods::SearchModeWire::Hybrid,
                    SearchModeWireKind::Hybrid {
                        vector_weight: Some(*vector_weight),
                    },
                ),
            };
            if !self.declares(mode) {
                return Err(ReactError::Other(format!(
                    "store extension does not declare {:?} search",
                    mode
                )));
            }
            let payload = SearchQueryWire {
                text: query.text.to_string(),
                limit: query.limit,
                mode: wire_mode,
            };
            let wire = serde_json::json!({
                "namespace": namespace,
                "query": payload,
            });
            let value = self
                .call_op(ExtensionOperation::StoreSearchWith, wire)
                .await?;
            from_wire(value).map_err(ReactError::Other)
        })
    }

    fn delete<'a>(
        &'a self,
        namespace: &'a [&'a str],
        key: &'a str,
    ) -> futures::future::BoxFuture<'a, echo_agent::error::Result<bool>> {
        Box::pin(async move {
            let value = self
                .call_op(
                    ExtensionOperation::StoreDelete,
                    serde_json::json!({"namespace": namespace, "key": key}),
                )
                .await?;
            from_wire(value).map_err(ReactError::Other)
        })
    }

    fn list_namespaces<'a>(
        &'a self,
        prefix: Option<&'a [&'a str]>,
    ) -> futures::future::BoxFuture<'a, echo_agent::error::Result<Vec<Vec<String>>>> {
        Box::pin(async move {
            let value = self
                .call_op(
                    ExtensionOperation::StoreListNamespaces,
                    serde_json::json!({"prefix": prefix}),
                )
                .await?;
            from_wire(value).map_err(ReactError::Other)
        })
    }

    fn list<'a>(
        &'a self,
        namespace: &'a [&'a str],
    ) -> futures::future::BoxFuture<'a, echo_agent::error::Result<Vec<echo_agent::memory::StoreItem>>>
    {
        Box::pin(async move {
            let value = self
                .call_op(
                    ExtensionOperation::StoreList,
                    serde_json::json!({"namespace": namespace}),
                )
                .await?;
            from_wire(value).map_err(ReactError::Other)
        })
    }
}

// ── HumanLoopProvider proxy ─────────────────────────────────────────────────

/// Wire projection of a human-loop request.
#[derive(serde::Serialize, serde::Deserialize)]
struct HumanLoopRequestWire {
    #[serde(skip_serializing_if = "Option::is_none")]
    request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent_name: Option<String>,
    kind: String,
    prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    args: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    timeout_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    task_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    context: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    phase: Option<String>,
}

/// Wire projection of the one-shot response.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(tag = "variant", rename_all = "snake_case")]
enum HumanLoopResponseWire {
    Approved,
    ApprovedWithScope {
        scope: String,
    },
    ModifiedArgs {
        args: serde_json::Value,
        scope: String,
    },
    Rejected {
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    Text(String),
    Timeout,
    Deferred,
    Selection {
        selection: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        instructions: Option<String>,
    },
}

impl TryFrom<&echo_agent::human_loop::HumanLoopResponse> for HumanLoopResponseWire {
    type Error = String;

    fn try_from(
        response: &echo_agent::human_loop::HumanLoopResponse,
    ) -> std::result::Result<Self, String> {
        use echo_agent::human_loop::HumanLoopResponse as Framework;
        Ok(match response {
            Framework::Approved => Self::Approved,
            Framework::ApprovedWithScope { scope } => Self::ApprovedWithScope {
                scope: format!("{scope:?}"),
            },
            Framework::ModifiedArgs { args, scope } => Self::ModifiedArgs {
                args: args.clone(),
                scope: format!("{scope:?}"),
            },
            Framework::Rejected { reason } => Self::Rejected {
                reason: reason.clone(),
            },
            Framework::Text(text) => Self::Text(text.clone()),
            Framework::Timeout => Self::Timeout,
            Framework::Deferred => Self::Deferred,
            Framework::Selection {
                selection,
                instructions,
            } => Self::Selection {
                selection: selection.clone(),
                instructions: instructions.clone(),
            },
        })
    }
}

impl TryFrom<HumanLoopResponseWire> for echo_agent::human_loop::HumanLoopResponse {
    type Error = String;

    fn try_from(wire: HumanLoopResponseWire) -> std::result::Result<Self, String> {
        use echo_agent::human_loop::HumanLoopResponse as Framework;
        Ok(match wire {
            HumanLoopResponseWire::Approved => Framework::Approved,
            HumanLoopResponseWire::ApprovedWithScope { scope } => Framework::ApprovedWithScope {
                scope: parse_scope(&scope)?,
            },
            HumanLoopResponseWire::ModifiedArgs { args, scope } => Framework::ModifiedArgs {
                args,
                scope: parse_scope(&scope)?,
            },
            HumanLoopResponseWire::Rejected { reason } => Framework::Rejected { reason },
            HumanLoopResponseWire::Text(text) => Framework::Text(text),
            HumanLoopResponseWire::Timeout => Framework::Timeout,
            HumanLoopResponseWire::Deferred => Framework::Deferred,
            HumanLoopResponseWire::Selection {
                selection,
                instructions,
            } => Framework::Selection {
                selection,
                instructions,
            },
        })
    }
}

fn parse_scope(scope: &str) -> std::result::Result<echo_agent::human_loop::ApprovalScope, String> {
    match scope.trim_matches('"') {
        "Once" => Ok(echo_agent::human_loop::ApprovalScope::Once),
        "Session" => Ok(echo_agent::human_loop::ApprovalScope::Session),
        "SessionTool" => Ok(echo_agent::human_loop::ApprovalScope::SessionTool),
        other => Err(format!("unknown approval scope {other:?}")),
    }
}

/// Thin `HumanLoopProvider` proxy: request identity is preserved, the
/// response settles exactly once per invocation, and disconnect or timeout
/// becomes an explicit framework error instead of a fallback provider.
pub(crate) struct ExtensionHumanLoopProxy {
    bridge: Arc<ExtensionBridge>,
    extension: WireHandle,
}

impl ExtensionHumanLoopProxy {
    pub(crate) fn new(bridge: Arc<ExtensionBridge>, extension: WireHandle) -> Option<Self> {
        let record = bridge.state().ok()?.handles.extension(&extension).ok()?;
        matches!(
            record.descriptor,
            ExtensionDescriptor::HumanLoopProvider { .. }
        )
        .then(|| Self { bridge, extension })
    }
}

impl echo_agent::human_loop::HumanLoopProvider for ExtensionHumanLoopProxy {
    fn request(
        &self,
        request: echo_agent::human_loop::HumanLoopRequest,
    ) -> futures::future::BoxFuture<
        '_,
        echo_agent::error::Result<echo_agent::human_loop::HumanLoopResponse>,
    > {
        Box::pin(async move {
            let payload = HumanLoopRequestWire {
                request_id: request.request_id.clone(),
                session_id: request.session_id.clone(),
                agent_name: request.agent_name.clone(),
                kind: format!("{:?}", request.kind).to_snake(),
                prompt: request.prompt.clone(),
                tool_name: request.tool_name.clone(),
                args: request.args.clone(),
                timeout_ms: request.timeout.map(|duration| duration.as_millis() as u64),
                task_id: request.task_id.clone(),
                options: request.options.clone(),
                context: request.context.clone(),
                phase: request.phase.clone(),
            };
            let value = self
                .bridge
                .invoke_once(
                    &self.extension,
                    ExtensionOperation::HumanLoopRequest,
                    None,
                    to_wire(payload).map_err(ReactError::Other)?,
                    cancelled_token(),
                )
                .await
                .map_err(react_error)?;
            let wire: HumanLoopResponseWire = from_wire(value).map_err(ReactError::Other)?;
            wire.try_into().map_err(ReactError::Other)
        })
    }
}

/// Small snake_case helper for framework enum debug names.
trait ToSnake {
    fn to_snake(self) -> String;
}

impl ToSnake for String {
    fn to_snake(self) -> String {
        let mut out = String::with_capacity(self.len());
        for (index, character) in self.chars().enumerate() {
            if character.is_ascii_uppercase() {
                if index > 0 {
                    out.push('_');
                }
                out.push(character.to_ascii_lowercase());
            } else {
                out.push(character);
            }
        }
        out
    }
}

// ── Hook bridge ─────────────────────────────────────────────────────────────

/// Wire projection of `HookResult`: the framework type carries no serde
/// derives, so block/mutation/propagation facts are projected field-wise.
/// Permission decisions ride as tagged values so deny reasons and
/// suggestions survive the round trip.
#[derive(serde::Serialize, serde::Deserialize, Default)]
struct HookResultWire {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    block: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    block_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    updated_input: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    messages: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    stop_propagation: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    permission_decision: Option<PermissionDecisionWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    permission_mode_override: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    continue_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    injected_context: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    retry: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    activate_skill: Option<(String, String)>,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
enum PermissionDecisionWire {
    Allow,
    Deny { reason: String },
    RequireApproval,
    Ask { suggestions: Vec<String> },
}

impl HookResultWire {
    fn into_result(self) -> echo_agent::hooks::HookResult {
        echo_agent::hooks::HookResult {
            block: self.block,
            block_reason: self.block_reason,
            updated_input: self.updated_input,
            messages: self.messages,
            stop_propagation: self.stop_propagation,
            permission_decision: self.permission_decision.map(|decision| match decision {
                PermissionDecisionWire::Allow => {
                    echo_agent::tools::permission::PermissionDecision::Allow
                }
                PermissionDecisionWire::Deny { reason } => {
                    echo_agent::tools::permission::PermissionDecision::Deny { reason }
                }
                PermissionDecisionWire::RequireApproval => {
                    echo_agent::tools::permission::PermissionDecision::RequireApproval
                }
                PermissionDecisionWire::Ask { suggestions } => {
                    echo_agent::tools::permission::PermissionDecision::Ask { suggestions }
                }
            }),
            permission_mode_override: self
                .permission_mode_override
                .and_then(|mode| serde_json::from_value(serde_json::Value::String(mode)).ok()),
            continue_reason: self.continue_reason,
            injected_context: self.injected_context,
            retry: self.retry,
            metadata: self.metadata,
            activate_skill: self.activate_skill,
        }
    }
}

/// Build the framework programmatic-hook executor for one Hook extension.
/// The closure preserves hook semantics: the wire result maps back to
/// `HookResult` (block/mutation/propagation) unchanged.
pub(crate) fn hook_executor(
    bridge: Arc<ExtensionBridge>,
    extension: WireHandle,
) -> echo_agent::skills::hooks::ProgrammaticHookFn {
    Arc::new(move |context: echo_agent::hooks::HookContext| {
        let bridge = bridge.clone();
        let extension = extension.clone();
        Box::pin(async move {
            let input = match to_wire(&context) {
                Ok(input) => input,
                Err(error) => {
                    tracing::warn!("hook context could not be projected: {error}");
                    return echo_agent::hooks::HookResult::default();
                }
            };
            match bridge
                .invoke_once(
                    &extension,
                    ExtensionOperation::HookRun,
                    None,
                    input,
                    cancelled_token(),
                )
                .await
            {
                Ok(value) => from_wire::<HookResultWire>(value)
                    .map(HookResultWire::into_result)
                    .unwrap_or_default(),
                Err(error) => {
                    tracing::warn!("hook extension {} failed: {}", extension.id, error.message);
                    echo_agent::hooks::HookResult::default()
                }
            }
        })
    })
}

// ── AgentCallback proxy ─────────────────────────────────────────────────────

/// Observational callback proxy. Callback failures are logged with bounded
/// diagnostics and never fail the run — matching the Rust trait contract
/// (`BoxFuture<'a, ()>` cannot propagate errors).
pub(crate) struct ExtensionAgentCallbackProxy {
    bridge: Arc<ExtensionBridge>,
    extension: WireHandle,
}

impl ExtensionAgentCallbackProxy {
    pub(crate) fn new(bridge: Arc<ExtensionBridge>, extension: WireHandle) -> Option<Self> {
        let record = bridge.state().ok()?.handles.extension(&extension).ok()?;
        matches!(record.descriptor, ExtensionDescriptor::AgentCallback { .. })
            .then(|| Self { bridge, extension })
    }

    fn observe(
        &self,
        operation: ExtensionOperation,
        payload: impl serde::Serialize,
    ) -> futures::future::BoxFuture<'_, ()> {
        let bridge = self.bridge.clone();
        let extension = self.extension.clone();
        let input = to_wire(payload).ok();
        Box::pin(async move {
            let Some(input) = input else {
                return;
            };
            if let Err(error) = bridge
                .invoke_once(&extension, operation, None, input, cancelled_token())
                .await
            {
                tracing::warn!(
                    "agent callback extension {} failed: {}",
                    extension.id,
                    error.message
                );
            }
        })
    }
}

impl echo_agent::agent::AgentCallback for ExtensionAgentCallbackProxy {
    fn on_think_start<'a>(
        &'a self,
        agent: &'a str,
        messages: &'a [echo_agent::llm::types::Message],
    ) -> futures::future::BoxFuture<'a, ()> {
        self.observe(
            ExtensionOperation::CallbackOnThinkStart,
            serde_json::json!({"agent": agent, "message_count": messages.len()}),
        )
    }

    fn on_think_end<'a>(
        &'a self,
        agent: &'a str,
        steps: &'a [echo_agent::agent::StepType],
        prompt_tokens: usize,
        completion_tokens: usize,
    ) -> futures::future::BoxFuture<'a, ()> {
        self.observe(
            ExtensionOperation::CallbackOnThinkEnd,
            serde_json::json!({
                "agent": agent,
                "steps": steps.iter().map(|step| format!("{step:?}")).collect::<Vec<_>>(),
                "prompt_tokens": prompt_tokens,
                "completion_tokens": completion_tokens,
            }),
        )
    }

    fn on_tool_start<'a>(
        &'a self,
        agent: &'a str,
        tool: &'a str,
        args: &'a serde_json::Value,
    ) -> futures::future::BoxFuture<'a, ()> {
        self.observe(
            ExtensionOperation::CallbackOnToolStart,
            serde_json::json!({"agent": agent, "tool": tool, "args": args}),
        )
    }

    fn on_tool_end<'a>(
        &'a self,
        agent: &'a str,
        tool: &'a str,
        result: &'a str,
    ) -> futures::future::BoxFuture<'a, ()> {
        self.observe(
            ExtensionOperation::CallbackOnToolEnd,
            serde_json::json!({"agent": agent, "tool": tool, "result": result}),
        )
    }

    fn on_tool_error<'a>(
        &'a self,
        agent: &'a str,
        tool: &'a str,
        error: &'a ReactError,
    ) -> futures::future::BoxFuture<'a, ()> {
        self.observe(
            ExtensionOperation::CallbackOnToolError,
            serde_json::json!({"agent": agent, "tool": tool, "error": error.to_string()}),
        )
    }

    fn on_final_answer<'a>(
        &'a self,
        agent: &'a str,
        answer: &'a str,
    ) -> futures::future::BoxFuture<'a, ()> {
        self.observe(
            ExtensionOperation::CallbackOnFinalAnswer,
            serde_json::json!({"agent": agent, "answer": answer}),
        )
    }

    fn on_iteration<'a>(
        &'a self,
        agent: &'a str,
        iteration: usize,
    ) -> futures::future::BoxFuture<'a, ()> {
        self.observe(
            ExtensionOperation::CallbackOnIteration,
            serde_json::json!({"agent": agent, "iteration": iteration}),
        )
    }
}

// ── InterventionCallback proxy ──────────────────────────────────────────────

/// Wire projection of `InterventionResult`: the constructor-based Rust API
/// becomes an explicit field set so every behavior (block/inject/redirect/
/// cancel/modify) stays observable and composable.
#[derive(serde::Serialize, serde::Deserialize, Default)]
struct InterventionResultWire {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    block: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    block_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    injected_context: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    redirect_to: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    cancel: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    modified_args: Option<serde_json::Value>,
}

impl From<InterventionResultWire> for echo_agent::agent::InterventionResult {
    fn from(wire: InterventionResultWire) -> Self {
        Self {
            block: wire.block,
            block_reason: wire.block_reason,
            injected_context: wire.injected_context,
            redirect_to: wire.redirect_to,
            cancel: wire.cancel,
            modified_args: wire.modified_args,
        }
    }
}

/// Behavior-controlling intervention proxy. It is its own extension kind —
/// never an alias of the observational AgentCallback — and failures map to
/// `allow` only through the framework's explicit default, never a fallback
/// implementation.
pub(crate) struct ExtensionInterventionProxy {
    bridge: Arc<ExtensionBridge>,
    extension: WireHandle,
}

impl ExtensionInterventionProxy {
    pub(crate) fn new(bridge: Arc<ExtensionBridge>, extension: WireHandle) -> Option<Self> {
        let record = bridge.state().ok()?.handles.extension(&extension).ok()?;
        matches!(
            record.descriptor,
            ExtensionDescriptor::InterventionCallback { .. }
        )
        .then(|| Self { bridge, extension })
    }

    fn decide(
        &self,
        operation: ExtensionOperation,
        payload: impl serde::Serialize,
    ) -> futures::future::BoxFuture<'_, echo_agent::agent::InterventionResult> {
        let bridge = self.bridge.clone();
        let extension = self.extension.clone();
        let input = to_wire(payload).ok();
        Box::pin(async move {
            let Some(input) = input else {
                return echo_agent::agent::InterventionResult::allow();
            };
            match bridge
                .invoke_once(&extension, operation, None, input, cancelled_token())
                .await
            {
                Ok(value) => from_wire::<InterventionResultWire>(value)
                    .map(echo_agent::agent::InterventionResult::from)
                    .unwrap_or_default(),
                Err(error) => {
                    tracing::warn!(
                        "intervention extension {} failed: {}",
                        extension.id,
                        error.message
                    );
                    echo_agent::agent::InterventionResult::allow()
                }
            }
        })
    }
}

impl echo_agent::agent::InterventionCallback for ExtensionInterventionProxy {
    fn on_tool_call<'a>(
        &'a self,
        agent: &'a str,
        tool: &'a str,
        args: &'a serde_json::Value,
    ) -> futures::future::BoxFuture<'a, echo_agent::agent::InterventionResult> {
        self.decide(
            ExtensionOperation::InterventionOnToolCall,
            serde_json::json!({"agent": agent, "tool": tool, "args": args}),
        )
    }

    fn on_think_start<'a>(
        &'a self,
        agent: &'a str,
        messages: &'a [echo_agent::llm::types::Message],
    ) -> futures::future::BoxFuture<'a, echo_agent::agent::InterventionResult> {
        self.decide(
            ExtensionOperation::InterventionOnThinkStart,
            serde_json::json!({"agent": agent, "message_count": messages.len()}),
        )
    }

    fn on_final_answer<'a>(
        &'a self,
        agent: &'a str,
        answer: &'a str,
    ) -> futures::future::BoxFuture<'a, echo_agent::agent::InterventionResult> {
        self.decide(
            ExtensionOperation::InterventionOnFinalAnswer,
            serde_json::json!({"agent": agent, "answer": answer}),
        )
    }
}

// ── AgentFactory + custom Agent proxies ─────────────────────────────────────

/// Wire projection of an `AgentFactoryConfig`: Rust tool objects cannot
/// cross the wire, so the factory receives the construction facts only.
#[derive(serde::Serialize, serde::Deserialize)]
struct AgentFactoryConfigWire {
    model: String,
    name: String,
    system_prompt: String,
    tool_count: usize,
}

/// Wire projection of a created custom agent's identity facts.
#[derive(serde::Serialize, serde::Deserialize)]
struct CustomAgentDescriptorWire {
    name: String,
    model_name: String,
    #[serde(default)]
    system_prompt: String,
    #[serde(default)]
    tool_names: Vec<String>,
}

/// Async subagent-factory adapter: the framework's lazy subagent
/// construction calls `create()`, which forwards to the same factory
/// operation with a minimal construction config.
pub(crate) struct SubagentFactoryAdapter {
    bridge: Arc<ExtensionBridge>,
    extension: WireHandle,
    subagent_name: String,
}

impl echo_agent::agent::subagent::AgentFactory for SubagentFactoryAdapter {
    fn create(
        &self,
    ) -> futures::future::BoxFuture<'static, echo_agent::error::Result<Box<dyn Agent>>> {
        let bridge = self.bridge.clone();
        let extension = self.extension.clone();
        let subagent_name = self.subagent_name.clone();
        Box::pin(async move {
            let payload = AgentFactoryConfigWire {
                model: String::new(),
                name: subagent_name,
                system_prompt: String::new(),
                tool_count: 0,
            };
            let input = to_wire(payload).map_err(ReactError::Other)?;
            let value = bridge
                .invoke_once(
                    &extension,
                    ExtensionOperation::FactoryCreateAgent,
                    None,
                    input,
                    cancelled_token(),
                )
                .await
                .map_err(react_error)?;
            let descriptor: CustomAgentDescriptorWire =
                from_wire(value).map_err(ReactError::Other)?;
            Ok(Box::new(ExtensionCustomAgentProxy {
                bridge,
                extension,
                descriptor,
            }) as Box<dyn Agent>)
        })
    }
}

/// Thin custom `Agent` proxy: execute/chat (and their streaming forms) run
/// through the bridge; events the SDK emits are plain `AgentEvent` values
/// the existing run observers already project — the proxy never fabricates
/// sequence numbers or terminals.
pub(crate) struct ExtensionCustomAgentProxy {
    bridge: Arc<ExtensionBridge>,
    extension: WireHandle,
    descriptor: CustomAgentDescriptorWire,
}

impl ExtensionCustomAgentProxy {
    async fn run_once(
        &self,
        operation: ExtensionOperation,
        payload: impl serde::Serialize,
    ) -> echo_agent::error::Result<String> {
        let value = self
            .bridge
            .invoke_once(
                &self.extension,
                operation,
                None,
                to_wire(payload).map_err(ReactError::Other)?,
                cancelled_token(),
            )
            .await
            .map_err(react_error)?;
        from_wire(value).map_err(ReactError::Other)
    }

    async fn run_stream(
        &self,
        operation: ExtensionOperation,
        payload: impl serde::Serialize,
    ) -> echo_agent::error::Result<
        futures::stream::BoxStream<'static, echo_agent::error::Result<AgentEvent>>,
    > {
        let (receiver, stream) = self
            .bridge
            .invoke_stream(
                &self.extension,
                operation,
                None,
                to_wire(payload).map_err(ReactError::Other)?,
                cancelled_token(),
            )
            .await
            .map_err(react_error)?;
        Ok(futures::StreamExt::map(
            extension_event_stream(self.bridge.clone(), stream, receiver),
            |event: ExtensionStreamEvent| {
                let result: echo_agent::error::Result<AgentEvent> = match event {
                    ExtensionStreamEvent::Chunk { value, .. }
                    | ExtensionStreamEvent::Complete { value, .. } => {
                        from_wire(value).map_err(ReactError::Other)
                    }
                    ExtensionStreamEvent::Failed { error, .. } => Err(react_error(error)),
                    ExtensionStreamEvent::Cancelled { .. } => Err(ReactError::Other(
                        "custom agent stream was cancelled".to_string(),
                    )),
                };
                result
            },
        )
        .boxed())
    }
}

impl Agent for ExtensionCustomAgentProxy {
    fn name(&self) -> &str {
        &self.descriptor.name
    }

    fn model_name(&self) -> &str {
        &self.descriptor.model_name
    }

    fn system_prompt(&self) -> &str {
        &self.descriptor.system_prompt
    }

    fn tool_names(&self) -> Vec<String> {
        self.descriptor.tool_names.clone()
    }

    fn close<'a>(&'a self) -> futures::future::BoxFuture<'a, echo_agent::error::Result<()>> {
        Box::pin(async move {
            let _ = self
                .run_once(ExtensionOperation::AgentClose, serde_json::json!({}))
                .await;
            Ok(())
        })
    }

    fn execute<'a>(
        &'a self,
        task: &'a str,
    ) -> futures::future::BoxFuture<'a, echo_agent::error::Result<String>> {
        Box::pin(async move {
            self.run_once(
                ExtensionOperation::AgentExecute,
                serde_json::json!({"task": task}),
            )
            .await
        })
    }

    fn execute_stream<'a>(
        &'a self,
        task: &'a str,
    ) -> futures::future::BoxFuture<
        'a,
        echo_agent::error::Result<
            futures::stream::BoxStream<'a, echo_agent::error::Result<AgentEvent>>,
        >,
    > {
        Box::pin(async move {
            let stream = self
                .run_stream(
                    ExtensionOperation::AgentExecuteStream,
                    serde_json::json!({"task": task}),
                )
                .await?;
            Ok(stream)
        })
    }

    fn chat<'a>(
        &'a self,
        message: &'a str,
    ) -> futures::future::BoxFuture<'a, echo_agent::error::Result<String>> {
        Box::pin(async move {
            self.run_once(
                ExtensionOperation::AgentChat,
                serde_json::json!({"message": message}),
            )
            .await
        })
    }

    fn chat_stream<'a>(
        &'a self,
        message: &'a str,
    ) -> futures::future::BoxFuture<
        'a,
        echo_agent::error::Result<
            futures::stream::BoxStream<'a, echo_agent::error::Result<AgentEvent>>,
        >,
    > {
        Box::pin(async move {
            let stream = self
                .run_stream(
                    ExtensionOperation::AgentChatStream,
                    serde_json::json!({"message": message}),
                )
                .await?;
            Ok(stream)
        })
    }
}

// ── Session Agent construction integration ──────────────────────────────────

/// Inject every currently registered extension into one Session Agent at
/// construction. Called by `PreparedAgentDefinition::create_agent` so all
/// Sessions — standard `session/new` and `_echo_agent/session/create`
/// alike — share one bridge wiring path. Sessions created before a
/// registration simply never see it: registration is connection-owned and
/// takes effect for Agents constructed afterwards.
pub(crate) async fn apply_extensions_to_agent(
    bridge: &Arc<ExtensionBridge>,
    agent: &mut echo_agent::agent::ReactAgent,
) -> std::result::Result<(), String> {
    let state = bridge.state().map_err(|error| error.message)?;
    // LlmClient: the most recently registered implementation becomes the
    // Session's model client.
    if let Some((extension, _)) = state
        .handles
        .extensions_of_kind(ExtensionKind::LlmClient)
        .pop()
        && let Some(proxy) = ExtensionLlmClientProxy::new(bridge.clone(), extension)
    {
        agent.set_llm_client(Arc::new(proxy));
    }
    // Tools.
    for (extension, _) in state.handles.extensions_of_kind(ExtensionKind::Tool) {
        if let Some(proxy) = ExtensionToolProxy::new(bridge.clone(), extension) {
            agent.add_tool(Box::new(proxy));
        }
    }
    // Memory store: set_memory_store re-registers the remember/recall/forget
    // tools against the extension-backed store, so model-driven memory calls
    // become real reverse invocations.
    if let Some((extension, _)) = state
        .handles
        .extensions_of_kind(ExtensionKind::Store)
        .first()
        .cloned()
        && let Some(proxy) = ExtensionStoreProxy::new(bridge.clone(), extension)
    {
        agent.set_memory_store(Arc::new(proxy));
    }
    // Human-in-the-loop provider: swap the approval channel and register the
    // appeal tool so model-initiated approvals also reach the extension.
    if let Some((extension, _)) = state
        .handles
        .extensions_of_kind(ExtensionKind::HumanLoopProvider)
        .first()
        .cloned()
        && let Some(proxy) = ExtensionHumanLoopProxy::new(bridge.clone(), extension)
    {
        let shared = Arc::new(proxy);
        agent.set_approval_provider(shared.clone());
        agent.add_need_appeal_tool(Box::new(
            echo_agent::tools::builtin::human_in_loop::HumanInLoop::new(shared),
        ));
    }
    // Hooks: programmatic sources in the agent's hook registry.
    {
        let registry = agent.hook_registry().clone();
        let mut registry = registry.write().await;
        for (extension, record) in state.handles.extensions_of_kind(ExtensionKind::Hook) {
            let executor = hook_executor(bridge.clone(), extension);
            registry.set_programmatic_hook(&record.implementation_id, &[], executor);
        }
    }
    // Observational callbacks.
    for (extension, _) in state
        .handles
        .extensions_of_kind(ExtensionKind::AgentCallback)
    {
        if let Some(proxy) = ExtensionAgentCallbackProxy::new(bridge.clone(), extension) {
            agent.add_callback(Arc::new(proxy));
        }
    }
    // Intervention callbacks.
    for (extension, _) in state
        .handles
        .extensions_of_kind(ExtensionKind::InterventionCallback)
    {
        if let Some(proxy) = ExtensionInterventionProxy::new(bridge.clone(), extension) {
            agent.add_intervention_callback(Arc::new(proxy));
        }
    }
    // Custom agents register for subagent dispatch by name.
    for (extension, _) in state.handles.extensions_of_kind(ExtensionKind::CustomAgent) {
        if let Some(proxy) = ExtensionCustomAgentProxy::for_registration(bridge.clone(), extension)
        {
            agent.register_agent(Box::new(proxy));
        }
    }
    // Agent factories register lazily-constructed subagents by name.
    for (extension, record) in state
        .handles
        .extensions_of_kind(ExtensionKind::AgentFactory)
    {
        agent.register_subagent_factory(
            echo_agent::agent::subagent::SubagentDefinition::simple_sync(&record.implementation_id),
            Arc::new(SubagentFactoryAdapter {
                bridge: bridge.clone(),
                extension,
                subagent_name: record.implementation_id.clone(),
            }),
        );
    }
    Ok(())
}

impl ExtensionCustomAgentProxy {
    /// Build a proxy for a directly registered custom agent (no factory
    /// round-trip): identity facts come from the registration descriptor.
    pub(crate) fn for_registration(
        bridge: Arc<ExtensionBridge>,
        extension: WireHandle,
    ) -> Option<Self> {
        let record = bridge.state().ok()?.handles.extension(&extension).ok()?;
        let ExtensionDescriptor::CustomAgent {
            name,
            model_name,
            system_prompt,
            tool_names,
            ..
        } = &record.descriptor
        else {
            return None;
        };
        Some(Self {
            bridge,
            extension,
            descriptor: CustomAgentDescriptorWire {
                name: name.clone(),
                model_name: model_name.clone(),
                system_prompt: system_prompt.clone(),
                tool_names: tool_names.clone(),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snake_case_projection_is_stable() {
        assert_eq!("Approval".to_string().to_snake(), "approval");
        assert_eq!(
            "ApprovedWithScope".to_string().to_snake(),
            "approved_with_scope"
        );
    }

    #[test]
    fn chat_chunk_wire_round_trips_fields() {
        let chunk = echo_agent::llm::ChatChunk {
            delta: echo_agent::llm::types::DeltaMessage {
                role: Some("assistant".to_string()),
                content: Some("hello".to_string()),
                reasoning_content: None,
                reasoning_blocks: None,
                tool_calls: None,
            },
            finish_reason: Some("stop".to_string()),
            usage: None,
        };
        let wire = to_wire(ChatChunkWire::from_chunk(&chunk)).expect("wire");
        let back: ChatChunkWire = from_wire(wire).expect("decode");
        let restored = back.into_chunk();
        assert_eq!(restored.delta.content.as_deref(), Some("hello"));
        assert_eq!(restored.finish_reason.as_deref(), Some("stop"));
    }

    #[test]
    fn approval_scope_round_trips() {
        for scope in ["Once", "Session", "SessionTool"] {
            assert!(parse_scope(scope).is_ok());
        }
        assert!(parse_scope("Forever").is_err());
    }
}
