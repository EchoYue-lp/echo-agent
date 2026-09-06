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
use base64::Engine as _;
use echo_agent::agent::{Agent, AgentEvent, CancellationToken};
use echo_agent::error::ReactError;
use echo_agent::tools::{ToolContext, ToolParameters, ToolResult, ToolStreamEvent};
use futures::{Stream, StreamExt as _};
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context, Poll};
use std::time::Duration;
use std::time::Instant;
use tokio::sync::{Notify, mpsc};

use echo_sdk_protocol::error::{EchoSdkError, ExtensionErrorCode, Retryability};
use echo_sdk_protocol::handle::{HandleKind, WireHandle};
use echo_sdk_protocol::methods::{
    ActivateSkillWire, AgentEventWire, AgentFactoryConfigWire, AgentMessageInput,
    AgentStreamChunkWire, AgentStreamTerminalWire, AgentTaskInput, ApprovalScopeWire,
    CallbackFinalAnswerInput, CallbackIterationInput, CallbackThinkEndInput,
    CallbackThinkStartInput, CallbackToolEndInput, CallbackToolErrorInput, CallbackToolStartInput,
    CustomAgentDescriptorWire, ExtensionDescriptor, ExtensionInvocation,
    ExtensionInvocationContext, ExtensionInvokeCall, ExtensionInvokeOutcome, ExtensionKind,
    ExtensionOperation, ExtensionResult, ExtensionStreamChunkValue, ExtensionStreamCompleteValue,
    ExtensionStreamEvent, ExtensionUnit, HookResultWire, HookRunInput, HumanLoopKindWire,
    HumanLoopRequestWire, HumanLoopResponseWire, HumanRiskLevelWire, InterventionResultWire,
    LlmChatChunkWire, LlmChatRequestWire, LlmChatResponseWire, LlmDeltaToolCallWire,
    LlmMessageWire, LlmReasoningBlockWire, LlmStreamChunkWire, LlmStreamCompleteWire,
    LlmToolCallWire, LlmToolDefinitionWire, PermissionDecisionWire, StepTypeWire, StoreItemWire,
    StoreKeyInput, StoreListNamespacesInput, StoreNamespaceInput, StorePutInput, StoreSearchInput,
    StoreSearchModeWire, StoreSearchQueryWire, StoreSearchWithInput, ToolContextWire,
    ToolExecuteInput, ToolFailureWire, ToolOutputArtifactConfigWire, ToolOutputArtifactRefWire,
    ToolResultContentWire, ToolResultKindWire, ToolResultWire, ToolStreamChunkWire,
    ToolStreamEventWire, ToolValidateInput,
};
#[cfg(test)]
use echo_sdk_protocol::methods::{LlmDeltaFunctionWire, LlmUsageWire};
use echo_sdk_protocol::scalar::{WirePath, WireU64, WireValue};

use super::state::CoreProfileState;
use super::wire::sdk_error;

/// Bounded stream-channel capacity for one reverse callback stream. The
/// consumer (the Rust stream proxy) paces delivery; a full channel makes the
/// notification handler wait (never the reader loop — routing is spawned).
#[cfg(test)]
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
        if let Some(sink) = self
            .streams
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(stream_id)
        {
            sink.close();
        }
    }

    pub(crate) fn remove_streams_for_extension(&self, extension_id: &str) {
        let sinks = {
            let mut streams = self
                .streams
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let ids = streams
                .iter()
                .filter(|(_, sink)| sink.extension_id() == extension_id)
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>();
            ids.into_iter()
                .filter_map(|id| streams.remove(&id))
                .collect::<Vec<_>>()
        };
        for sink in sinks {
            sink.close();
        }
    }

    pub(crate) fn close_all_streams(&self) {
        let sinks = self
            .streams
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .drain()
            .map(|(_, sink)| sink)
            .collect::<Vec<_>>();
        for sink in sinks {
            sink.close();
        }
    }

    fn stream_sink(&self, stream_id: &str) -> Option<Arc<ExtensionStreamSink>> {
        self.streams
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(stream_id)
            .cloned()
    }

    /// Route one `_echo_agent/extension/stream` notification. Unknown or
    /// already-terminal streams discard the event with a bounded diagnostic
    /// (late delivery never overrides settled state). Delivery is
    /// non-blocking: a full bounded mailbox closes the stream and reports the
    /// backpressure failure to the notification handler.
    pub(crate) fn deliver_stream_event(
        &self,
        event: ExtensionStreamEvent,
    ) -> std::result::Result<(), String> {
        if let Err(reason) = event.validate() {
            self.fail_stream(event.stream(), reason);
            return Err(format!("stream event violated the contract: {reason}"));
        }
        let sink = {
            let streams = self
                .streams
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            streams.get(event.stream().id.as_str()).cloned()
        };
        let Some(sink) = sink else {
            tracing::warn!(
                stream = event.stream().id,
                "discarded stream event for an unknown or released stream"
            );
            return Ok(());
        };
        let payload_kind = match &event {
            ExtensionStreamEvent::Chunk { value, .. } => Some(value.operation_kind()),
            ExtensionStreamEvent::Complete { value, .. } => Some(value.operation_kind()),
            ExtensionStreamEvent::Failed { .. } | ExtensionStreamEvent::Cancelled { .. } => None,
        };
        if payload_kind.is_some_and(|kind| kind != sink.value_kind()) {
            self.fail_stream(
                sink.stream_handle(),
                "extension stream payload kind does not match the invocation",
            );
            return Err("extension stream payload kind does not match the invocation".to_string());
        }
        let sequence = event.sequence().to_u64().unwrap_or_default();
        if !sink.admit_event(&event) {
            let reason = format!(
                "extension stream sequence {sequence} is not the next contiguous value or follows a terminal"
            );
            self.fail_stream(sink.stream_handle(), &reason);
            return Err(reason);
        }
        let is_terminal = event.is_terminal();
        match sink.sender().try_send(event) {
            Ok(()) => {
                if is_terminal {
                    sink.clear_terminal();
                }
                Ok(())
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                self.remove_stream(sink.stream_id());
                Ok(())
            }
            Err(mpsc::error::TrySendError::Full(event)) => {
                let reason = "extension stream consumer exceeded its bounded mailbox";
                sink.fail_undelivered(event.sequence().clone(), reason);
                self.remove_stream(sink.stream_id());
                Err(reason.to_string())
            }
        }
    }

    pub(crate) fn fail_stream(&self, stream: &WireHandle, reason: &str) {
        let sink = self.stream_sink(&stream.id);
        if let Some(sink) = sink {
            let message = reason.to_string();
            sink.terminate(|sequence| ExtensionStreamEvent::Failed {
                stream: stream.clone(),
                sequence,
                error: sdk_error(
                    ExtensionErrorCode::SerializationViolation,
                    message,
                    Retryability::Never,
                    "_echo_agent/extension/stream",
                ),
            });
            self.remove_stream(sink.stream_id());
        }
    }
}

// ── Stream sink ─────────────────────────────────────────────────────────────

/// One live callback stream: bounded mailbox plus the monotonic-sequence and
/// exactly-one-terminal ledger.
pub(crate) struct ExtensionStreamSink {
    stream: WireHandle,
    extension_id: String,
    value_kind: ExtensionKind,
    sender: mpsc::Sender<ExtensionStreamEvent>,
    inner: Mutex<StreamSinkInner>,
    closed: AtomicBool,
    closed_notify: Notify,
}

struct StreamSinkInner {
    last_sequence: u64,
    terminal_seen: bool,
    terminal: Option<ExtensionStreamEvent>,
}

impl ExtensionStreamSink {
    fn new(
        stream: WireHandle,
        extension_id: String,
        value_kind: ExtensionKind,
        capacity: usize,
    ) -> (Arc<Self>, mpsc::Receiver<ExtensionStreamEvent>) {
        let (sender, receiver) = mpsc::channel(capacity.max(1));
        (
            Arc::new(Self {
                stream,
                extension_id,
                value_kind,
                sender,
                inner: Mutex::new(StreamSinkInner {
                    last_sequence: 0,
                    terminal_seen: false,
                    terminal: None,
                }),
                closed: AtomicBool::new(false),
                closed_notify: Notify::new(),
            }),
            receiver,
        )
    }

    fn stream_id(&self) -> &str {
        &self.stream.id
    }

    fn stream_handle(&self) -> &WireHandle {
        &self.stream
    }

    fn extension_id(&self) -> &str {
        &self.extension_id
    }

    fn value_kind(&self) -> ExtensionKind {
        self.value_kind
    }

    fn sender(&self) -> &mpsc::Sender<ExtensionStreamEvent> {
        &self.sender
    }

    fn close(&self) {
        self.terminate(|sequence| ExtensionStreamEvent::Cancelled {
            stream: self.stream.clone(),
            sequence,
        });
        if !self.closed.swap(true, Ordering::AcqRel) {
            self.closed_notify.notify_waiters();
        }
    }

    async fn wait_closed(&self) {
        loop {
            let notified = self.closed_notify.notified();
            if self.closed.load(Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }

    /// Sequence/terminal admission. Returns false for events that must be
    /// discarded (non-contiguous, post-terminal, or duplicate terminal).
    fn admit_event(&self, event: &ExtensionStreamEvent) -> bool {
        let sequence = event.sequence().to_u64().unwrap_or_default();
        let is_terminal = event.is_terminal();
        let Ok(mut inner) = self.inner.lock() else {
            return false;
        };
        let Some(expected) = inner.last_sequence.checked_add(1) else {
            return false;
        };
        if inner.terminal_seen || sequence != expected {
            return false;
        }
        inner.last_sequence = sequence;
        inner.terminal_seen = is_terminal;
        if is_terminal {
            inner.terminal = Some(event.clone());
        }
        true
    }

    /// Replace an admitted event that could not enter the bounded mailbox
    /// with a failed terminal at the same sequence. Keeping that terminal in
    /// the sink lets the consumer observe the failure after it drains the
    /// already-buffered prefix, without fabricating a sequence gap.
    fn fail_undelivered(&self, sequence: echo_sdk_protocol::scalar::WireNonZeroU64, reason: &str) {
        let event = ExtensionStreamEvent::Failed {
            stream: self.stream.clone(),
            sequence,
            error: sdk_error(
                ExtensionErrorCode::ExtensionFailed,
                reason,
                Retryability::Never,
                "_echo_agent/extension/stream",
            ),
        };
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        inner.terminal_seen = true;
        inner.terminal = Some(event.clone());
        drop(inner);
        if self.sender.try_send(event).is_ok() {
            self.clear_terminal();
        }
    }

    fn terminate(
        &self,
        build: impl FnOnce(echo_sdk_protocol::scalar::WireNonZeroU64) -> ExtensionStreamEvent,
    ) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        if inner.terminal_seen {
            return;
        }
        let Some(sequence) = inner.last_sequence.checked_add(1).and_then(|value| {
            echo_sdk_protocol::scalar::WireNonZeroU64::try_from(value.to_string()).ok()
        }) else {
            return;
        };
        inner.last_sequence = sequence.to_u64().unwrap_or_default();
        inner.terminal_seen = true;
        let event = build(sequence);
        inner.terminal = Some(event.clone());
        drop(inner);
        if self.sender.try_send(event).is_ok() {
            self.clear_terminal();
        }
    }

    fn take_terminal(&self) -> Option<ExtensionStreamEvent> {
        self.inner.lock().ok()?.terminal.take()
    }

    fn clear_terminal(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.terminal = None;
        }
    }
}

// ── Bridge handle ───────────────────────────────────────────────────────────

/// Per-call bridge over the shared state. Cheap to construct; proxies hold
/// one for their lifetime. The profile state binds itself after
/// construction (the bridge is created first because the default Agent
/// definition must capture it before any Session exists).
#[derive(Clone)]
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

    fn connection_cancellation(&self) -> CancellationToken {
        self.state()
            .ok()
            .and_then(|state| state.services().ok())
            .map(|services| services.extensions().connection_cancellation())
            .unwrap_or_default()
    }

    async fn cancellation_for_context(
        &self,
        context: Option<&ExtensionInvocationContext>,
        fallback: CancellationToken,
    ) -> CancellationToken {
        let Some(state) = self.state().ok() else {
            return fallback;
        };
        let Ok(services) = state.services() else {
            return fallback;
        };
        if let Some(run_id) = context.and_then(|context| context.run_id.as_deref())
            && let Some(run) = services.run(run_id).await
        {
            return run.cancellation();
        }
        if let Some(session_id) = context.and_then(|context| context.session_id.as_deref())
            && let Some(cancel) = services.active_run_cancellation(session_id).await
        {
            return cancel;
        }
        fallback
    }

    fn deadline_of(&self, record: &super::handles::ExtensionRecord) -> Duration {
        record
            .timeout
            .as_ref()
            .and_then(|timeout| {
                timeout.validate().ok().and_then(|_| {
                    timeout
                        .seconds
                        .to_u64()
                        .map(|seconds| Duration::new(seconds, timeout.nanos))
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
        context: Option<ExtensionInvocationContext>,
        invocation: ExtensionInvocation,
        framework_cancellation: CancellationToken,
    ) -> std::result::Result<ExtensionResult, EchoSdkError> {
        self.invoke_once_with_scope(extension, context, invocation, framework_cancellation, true)
            .await
    }

    async fn invoke_once_connection_scoped(
        &self,
        extension: &WireHandle,
        context: Option<ExtensionInvocationContext>,
        invocation: ExtensionInvocation,
    ) -> std::result::Result<ExtensionResult, EchoSdkError> {
        self.invoke_once_with_scope(
            extension,
            context,
            invocation,
            self.connection_cancellation(),
            false,
        )
        .await
    }

    async fn invoke_once_with_scope(
        &self,
        extension: &WireHandle,
        context: Option<ExtensionInvocationContext>,
        invocation: ExtensionInvocation,
        framework_cancellation: CancellationToken,
        resolve_run_cancellation: bool,
    ) -> std::result::Result<ExtensionResult, EchoSdkError> {
        let operation = invocation.operation();
        let framework_cancellation = if resolve_run_cancellation {
            self.cancellation_for_context(context.as_ref(), framework_cancellation)
                .await
        } else {
            framework_cancellation
        };
        let connection = self.shared.connection()?;
        let (mut lease, record, _authority) =
            self.prepare(extension, operation, framework_cancellation)?;
        self.ensure_payload_bound(&invocation, false)?;
        let deadline = self.deadline_of(&record);
        let call = ExtensionInvokeCall {
            extension: extension.clone(),
            invocation_id: lease.identity().to_string(),
            context,
            invocation,
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
        let mut drop_notice = InvocationDropNotice::new(self.clone(), lease.identity());
        let result = tokio::select! {
            answer = sent.block_task() => {
                match answer {
                    Ok(outcome) => self
                        .settle_outcome(&mut lease, outcome, operation, None)
                        .and_then(|result| result.ok_or_else(|| sdk_error(
                            ExtensionErrorCode::ExtensionFailed,
                            "non-streaming invocation returned no result",
                            Retryability::Never,
                            "_echo_agent/extension/invoke",
                        ))),
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
                drop_notice.disarm();
                if !lease.settle(echo_agent::acp::ExtensionSettlement::Cancelled) {
                    return Err(settlement_error(lease.settlement()));
                }
                Err(sdk_error(
                    ExtensionErrorCode::Cancelled,
                    "extension invocation was cancelled",
                    Retryability::Never,
                    "_echo_agent/extension/invoke",
                ))
            }
            _ = tokio::time::sleep(deadline) => {
                let _ = self.send_cancel_notice(lease.identity(), "timeout");
                drop_notice.disarm();
                if !lease.settle_timeout() {
                    return Err(settlement_error(lease.settlement()));
                }
                Err(sdk_error(
                    ExtensionErrorCode::ExtensionTimeout,
                    "extension invocation exceeded its deadline",
                    Retryability::Never,
                    "_echo_agent/extension/invoke",
                ))
            }
        };
        drop_notice.disarm();
        result
    }

    /// Drive one streaming reverse invocation: mint the stream handle,
    /// subscribe the sink, return the bounded receiver the Rust stream
    /// proxy consumes.
    pub(crate) async fn invoke_stream(
        &self,
        extension: &WireHandle,
        context: Option<ExtensionInvocationContext>,
        invocation: ExtensionInvocation,
        framework_cancellation: CancellationToken,
    ) -> std::result::Result<
        (
            mpsc::Receiver<ExtensionStreamEvent>,
            WireHandle,
            String,
            CancellationToken,
            Arc<Mutex<Option<echo_agent::acp::ExtensionInvocationLease>>>,
            Arc<ExtensionStreamSink>,
        ),
        EchoSdkError,
    > {
        const OPERATION_STR: &str = "_echo_agent/extension/invoke";
        let operation = invocation.operation();
        debug_assert!(operation.is_streaming());
        let started_at = Instant::now();
        let framework_cancellation = self
            .cancellation_for_context(context.as_ref(), framework_cancellation)
            .await;
        let connection = self.shared.connection()?;
        let (mut lease, record, _authority) =
            self.prepare(extension, operation, framework_cancellation)?;
        self.ensure_payload_bound(&invocation, false)?;
        let deadline = self.deadline_of(&record);
        let state = self.state()?;
        let stream = state.handles.register_extension_stream(&extension.id)?;
        let (sink, receiver) = ExtensionStreamSink::new(
            stream.clone(),
            extension.id.clone(),
            operation.kind(),
            state.limits.max_outstanding_live_events,
        );
        self.shared.register_stream(sink.clone());
        let call = ExtensionInvokeCall {
            extension: extension.clone(),
            invocation_id: lease.identity().to_string(),
            context,
            invocation,
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
        let mut drop_notice = InvocationDropNotice::new(self.clone(), lease.identity());
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
                        drop_notice.disarm();
                        self.release_stream(&stream);
                        return Err(typed);
                    }
                }
            }
            () = cancellation.cancelled() => {
                let _ = self.send_cancel_notice(lease.identity(), "cancelled");
                drop_notice.disarm();
                if !lease.settle(echo_agent::acp::ExtensionSettlement::Cancelled) {
                    self.release_stream(&stream);
                    return Err(settlement_error(lease.settlement()));
                }
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
                drop_notice.disarm();
                if !lease.settle_timeout() {
                    self.release_stream(&stream);
                    return Err(settlement_error(lease.settlement()));
                }
                self.release_stream(&stream);
                return Err(sdk_error(
                    ExtensionErrorCode::ExtensionTimeout,
                    "extension stream invocation exceeded its deadline",
                    Retryability::Never,
                    OPERATION_STR,
                ));
            }
        };
        drop_notice.disarm();
        let invocation_id = lease.identity().to_string();
        match self.settle_outcome(&mut lease, outcome, operation, Some(&stream)) {
            Ok(None) => {
                let lease = Arc::new(Mutex::new(Some(lease)));
                self.watch_stream(
                    stream.clone(),
                    invocation_id.clone(),
                    cancellation.clone(),
                    deadline.saturating_sub(started_at.elapsed()),
                    lease.clone(),
                );
                Ok((receiver, stream, invocation_id, cancellation, lease, sink))
            }
            Ok(Some(_)) => {
                self.release_stream(&stream);
                Err(sdk_error(
                    ExtensionErrorCode::ExtensionFailed,
                    "streaming invocation returned a single result",
                    Retryability::Never,
                    OPERATION_STR,
                ))
            }
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
    ) -> std::result::Result<Option<ExtensionResult>, EchoSdkError> {
        if !lease.settle(echo_agent::acp::ExtensionSettlement::Answered) {
            return Err(settlement_error(lease.settlement()));
        }
        if let Err(reason) = outcome.validate() {
            return Err(sdk_error(
                ExtensionErrorCode::SerializationViolation,
                reason,
                Retryability::Never,
                "_echo_agent/extension/invoke",
            ));
        }
        match outcome {
            ExtensionInvokeOutcome::Result { result } => {
                self.ensure_payload_bound(&result, false)?;
                if operation.is_streaming() {
                    return Err(sdk_error(
                        ExtensionErrorCode::ExtensionFailed,
                        "streaming operation answered with a single result",
                        Retryability::Never,
                        "_echo_agent/extension/invoke",
                    ));
                }
                if result.operation() != operation {
                    return Err(sdk_error(
                        ExtensionErrorCode::SerializationViolation,
                        "extension result operation does not match the invocation",
                        Retryability::Never,
                        "_echo_agent/extension/invoke",
                    ));
                }
                Ok(Some(result))
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
                if stream != *expected {
                    return Err(sdk_error(
                        ExtensionErrorCode::ExtensionFailed,
                        "stream outcome did not echo the Host-minted stream handle",
                        Retryability::Never,
                        "_echo_agent/extension/invoke",
                    ));
                }
                Ok(None)
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

    fn register_factory_instance(
        &self,
        factory: &WireHandle,
        descriptor: &CustomAgentDescriptorWire,
    ) -> std::result::Result<WireHandle, EchoSdkError> {
        let state = self.state()?;
        let factory_record = state.handles.extension(factory)?;
        if factory_record.kind != ExtensionKind::AgentFactory {
            return Err(sdk_error(
                ExtensionErrorCode::InvalidValue,
                "factory instance creation requires an AgentFactory handle",
                Retryability::Never,
                "_echo_agent/extension/invoke",
            ));
        }
        let custom_descriptor = ExtensionDescriptor::CustomAgent {
            descriptor_version: 1,
            name: descriptor.name.clone(),
            model_name: descriptor.model_name.clone(),
            system_prompt: descriptor.system_prompt.clone(),
            tool_names: descriptor.tool_names.clone(),
        };
        let implementation_id = format!(
            "{}::instance-{}",
            factory_record.implementation_id,
            uuid::Uuid::new_v4()
        );
        state
            .handles
            .register_extension(
                super::handles::ExtensionRegistrationLimits {
                    max_extensions: state.limits.max_registered_extensions,
                    max_descriptor_bytes: state.limits.max_extension_descriptor_bytes,
                },
                ExtensionKind::CustomAgent,
                &implementation_id,
                custom_descriptor,
                factory_record.timeout.clone(),
                true,
            )
            .map(|(handle, _)| handle)
    }

    fn release_extension(&self, extension: &WireHandle) {
        if let Ok(state) = self.state()
            && let Ok(true) = state.handles.close_extension(extension)
        {
            self.shared.remove_streams_for_extension(&extension.id);
        }
    }

    fn ensure_payload_bound<T: serde::Serialize>(
        &self,
        value: &T,
        stream_chunk: bool,
    ) -> std::result::Result<(), EchoSdkError> {
        let encoded = serde_json::to_vec(value).map_err(|error| {
            sdk_error(
                ExtensionErrorCode::SerializationViolation,
                format!("extension payload is not encodable: {error}"),
                Retryability::Never,
                "_echo_agent/extension/invoke",
            )
        })?;
        let state = self.state()?;
        let limit = if stream_chunk {
            state.limits.max_extension_stream_bytes
        } else {
            state.limits.max_extension_payload_bytes
        };
        if encoded.len() > limit {
            return Err(sdk_error(
                ExtensionErrorCode::PayloadTooLarge,
                "extension payload exceeds the configured byte bound",
                Retryability::Never,
                "_echo_agent/extension/invoke",
            ));
        }
        Ok(())
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

    fn watch_stream(
        &self,
        stream: WireHandle,
        invocation_id: String,
        cancellation: CancellationToken,
        deadline: Duration,
        lease: Arc<Mutex<Option<echo_agent::acp::ExtensionInvocationLease>>>,
    ) {
        let bridge = self.clone();
        let sink = self.shared.stream_sink(&stream.id);
        tokio::spawn(async move {
            let Some(sink) = sink else {
                return;
            };
            tokio::select! {
                _ = sink.wait_closed() => {
                    drop_stream_lease(&lease);
                }
                _ = cancellation.cancelled() => {
                    sink.terminate(|sequence| ExtensionStreamEvent::Cancelled {
                        stream: stream.clone(),
                        sequence,
                    });
                    let _ = bridge.send_cancel_notice(&invocation_id, "cancelled");
                    drop_stream_lease(&lease);
                    bridge.release_stream(&stream);
                }
                _ = tokio::time::sleep(deadline) => {
                    cancellation.cancel();
                    sink.terminate(|sequence| ExtensionStreamEvent::Failed {
                        stream: stream.clone(),
                        sequence,
                        error: sdk_error(
                            ExtensionErrorCode::ExtensionTimeout,
                            "extension stream invocation exceeded its deadline",
                            Retryability::Never,
                            "_echo_agent/extension/stream",
                        ),
                    });
                    let _ = bridge.send_cancel_notice(&invocation_id, "timeout");
                    drop_stream_lease(&lease);
                    bridge.release_stream(&stream);
                }
            }
        });
    }
}

fn wire_duration(duration: Duration) -> echo_sdk_protocol::scalar::WireDuration {
    echo_sdk_protocol::scalar::WireDuration {
        seconds: WireU64::from_u64(duration.as_secs()),
        nanos: duration.subsec_nanos(),
    }
}

/// Sends the extension-level cancellation notice when an invocation future is
/// dropped by its owning run before the normal select branches can settle.
struct InvocationDropNotice {
    bridge: ExtensionBridge,
    invocation_id: String,
    armed: bool,
}

impl InvocationDropNotice {
    fn new(bridge: ExtensionBridge, invocation_id: &str) -> Self {
        Self {
            bridge,
            invocation_id: invocation_id.to_string(),
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for InvocationDropNotice {
    fn drop(&mut self) {
        if self.armed {
            let _ = self
                .bridge
                .send_cancel_notice(&self.invocation_id, "cancelled");
        }
    }
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
    invocation_id: String,
    cancellation: CancellationToken,
    lease: Arc<Mutex<Option<echo_agent::acp::ExtensionInvocationLease>>>,
    sink: Arc<ExtensionStreamSink>,
    receiver: mpsc::Receiver<ExtensionStreamEvent>,
) -> ExtensionEventStream {
    ExtensionEventStream {
        bridge,
        stream,
        invocation_id,
        cancellation,
        lease,
        sink,
        receiver,
        finished: false,
    }
}

struct ExtensionEventStream {
    bridge: Arc<ExtensionBridge>,
    stream: WireHandle,
    invocation_id: String,
    cancellation: CancellationToken,
    lease: Arc<Mutex<Option<echo_agent::acp::ExtensionInvocationLease>>>,
    sink: Arc<ExtensionStreamSink>,
    receiver: mpsc::Receiver<ExtensionStreamEvent>,
    finished: bool,
}

impl Stream for ExtensionEventStream {
    type Item = ExtensionStreamEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.finished {
            return Poll::Ready(None);
        }
        if self.receiver.is_empty()
            && let Some(event) = self.sink.take_terminal()
        {
            self.finished = true;
            drop_stream_lease(&self.lease);
            self.bridge.release_stream(&self.stream);
            return Poll::Ready(Some(event));
        }
        match Pin::new(&mut self.receiver).poll_recv(cx) {
            Poll::Ready(Some(event)) => {
                if event.is_terminal() {
                    self.finished = true;
                    self.sink.clear_terminal();
                    drop_stream_lease(&self.lease);
                    self.bridge.release_stream(&self.stream);
                }
                Poll::Ready(Some(event))
            }
            Poll::Ready(None) => {
                self.finished = true;
                drop_stream_lease(&self.lease);
                self.bridge.release_stream(&self.stream);
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for ExtensionEventStream {
    fn drop(&mut self) {
        if !self.finished {
            self.cancellation.cancel();
            let _ = self
                .bridge
                .send_cancel_notice(&self.invocation_id, "cancelled");
            drop_stream_lease(&self.lease);
            self.bridge.release_stream(&self.stream);
        }
    }
}

fn drop_stream_lease(lease: &Arc<Mutex<Option<echo_agent::acp::ExtensionInvocationLease>>>) {
    if let Ok(mut lease) = lease.lock() {
        lease.take();
    }
}

fn react_error(error: EchoSdkError) -> ReactError {
    ReactError::Other(format!(
        "extension bridge failure {}: {}",
        error.code.as_str(),
        error.message
    ))
}

fn settlement_error(settlement: Option<echo_agent::acp::ExtensionSettlement>) -> EchoSdkError {
    match settlement {
        Some(echo_agent::acp::ExtensionSettlement::Cancelled) => sdk_error(
            ExtensionErrorCode::Cancelled,
            "extension invocation was cancelled before its response was applied",
            Retryability::Never,
            "_echo_agent/extension/invoke",
        ),
        Some(echo_agent::acp::ExtensionSettlement::TimedOut) => sdk_error(
            ExtensionErrorCode::ExtensionTimeout,
            "extension invocation exceeded its deadline before its response was applied",
            Retryability::Never,
            "_echo_agent/extension/invoke",
        ),
        Some(echo_agent::acp::ExtensionSettlement::Disconnected) => sdk_error(
            ExtensionErrorCode::ExtensionDisconnected,
            "extension connection closed before its response was applied",
            Retryability::Never,
            "_echo_agent/extension/invoke",
        ),
        Some(echo_agent::acp::ExtensionSettlement::Answered) | None => sdk_error(
            ExtensionErrorCode::ExtensionFailed,
            "extension invocation settled concurrently without a usable response",
            Retryability::Never,
            "_echo_agent/extension/invoke",
        ),
    }
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
fn tool_context_wire(context: &ToolContext) -> std::result::Result<ToolContextWire, String> {
    Ok(ToolContextWire {
        working_dir: context.working_dir.as_deref().map(wire_path),
        conversation_id: context.conversation_id.clone(),
        run_id: context.run_id.clone(),
        turn_id: context.turn_id.clone(),
        message_id: context.message_id.clone(),
        execution_id: context.execution_id.clone(),
        call_id: context.call_id.clone(),
        active_message: context
            .active_message
            .as_ref()
            .map(message_wire)
            .transpose()?,
        output_artifacts: context.output_artifacts.as_ref().map(|config| {
            ToolOutputArtifactConfigWire {
                root_dir: wire_path(&config.root_dir),
                retention: config.retention.clone(),
                threshold_bytes: WireU64::from_u64(
                    u64::try_from(config.threshold_bytes).unwrap_or(u64::MAX),
                ),
                max_age_secs: config.max_age_secs.map(WireU64::from_u64),
            }
        }),
    })
}

fn wire_path(path: &std::path::Path) -> WirePath {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt as _;
        let bytes = path.as_os_str().as_bytes();
        WirePath::Unix {
            bytes_base64: base64::engine::general_purpose::STANDARD_NO_PAD.encode(bytes),
            display: path.to_str().map(str::to_string),
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt as _;
        let units = path
            .as_os_str()
            .encode_wide()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        WirePath::Windows {
            utf16_base64: base64::engine::general_purpose::STANDARD_NO_PAD.encode(units),
            display: path.to_str().map(str::to_string),
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        WirePath::Utf8 {
            path: path.to_string_lossy().into_owned(),
        }
    }
}

fn invocation_context(context: &ToolContext) -> Option<ExtensionInvocationContext> {
    let value = ExtensionInvocationContext {
        session_id: context.conversation_id.clone(),
        run_id: context.run_id.clone(),
        stream_id: None,
        turn_id: context.turn_id.clone(),
        message_id: context.message_id.clone(),
        execution_id: context.execution_id.clone(),
        call_id: context.call_id.clone(),
    };
    (value.session_id.is_some()
        || value.run_id.is_some()
        || value.turn_id.is_some()
        || value.message_id.is_some()
        || value.execution_id.is_some()
        || value.call_id.is_some())
    .then_some(value)
}

fn session_invocation_context(session_id: &str) -> ExtensionInvocationContext {
    ExtensionInvocationContext {
        session_id: Some(session_id.to_string()),
        run_id: None,
        stream_id: None,
        turn_id: None,
        message_id: None,
        execution_id: None,
        call_id: None,
    }
}

fn wire_path_to_path(path: WirePath) -> std::result::Result<std::path::PathBuf, String> {
    match path {
        WirePath::Utf8 { path } => Ok(path.into()),
        WirePath::Unix { bytes_base64, .. } => {
            #[cfg(unix)]
            {
                use std::os::unix::ffi::OsStringExt as _;
                let bytes = base64::engine::general_purpose::STANDARD_NO_PAD
                    .decode(bytes_base64)
                    .map_err(|error| error.to_string())?;
                Ok(std::ffi::OsString::from_vec(bytes).into())
            }
            #[cfg(not(unix))]
            {
                let _ = bytes_base64;
                Err("a Unix artifact path cannot be restored on this platform".to_string())
            }
        }
        WirePath::Windows { utf16_base64, .. } => {
            #[cfg(windows)]
            {
                use std::os::windows::ffi::OsStringExt as _;
                let bytes = base64::engine::general_purpose::STANDARD_NO_PAD
                    .decode(utf16_base64)
                    .map_err(|error| error.to_string())?;
                if bytes.len() % 2 != 0 {
                    return Err("Windows artifact path has an odd UTF-16 byte count".to_string());
                }
                let units = bytes
                    .chunks_exact(2)
                    .filter_map(|pair| Some(u16::from_le_bytes([*pair.first()?, *pair.get(1)?])))
                    .collect::<Vec<_>>();
                Ok(std::ffi::OsString::from_wide(&units).into())
            }
            #[cfg(not(windows))]
            {
                let _ = utf16_base64;
                Err("a Windows artifact path cannot be restored on this platform".to_string())
            }
        }
    }
}

fn tool_result_from_wire(wire: ToolResultWire) -> std::result::Result<ToolResult, String> {
    use echo_agent::tools::{
        ToolFailure, ToolFailureCategory, ToolRecoveryAction, ToolResultContent, ToolResultKind,
        ToolSideEffect,
    };
    let kind = match wire.kind {
        ToolResultKindWire::Text => ToolResultKind::Text,
        ToolResultKindWire::Json => ToolResultKind::Json,
        ToolResultKindWire::Image { mime_type } => ToolResultKind::Image { mime_type },
        ToolResultKindWire::Table { columns, rows } => ToolResultKind::Table { columns, rows },
        ToolResultKindWire::Diff { unified_diff } => ToolResultKind::Diff { unified_diff },
        ToolResultKindWire::FileReference { path } => ToolResultKind::FileReference { path },
        ToolResultKindWire::CommandOutput { exit_code } => {
            ToolResultKind::CommandOutput { exit_code }
        }
        ToolResultKindWire::SkillActivation { name } => ToolResultKind::SkillActivation { name },
        ToolResultKindWire::StructuredError { error_code } => {
            ToolResultKind::StructuredError { error_code }
        }
    };
    let failure = wire
        .failure
        .map(|failure| -> std::result::Result<ToolFailure, String> {
            let category = match failure.category.as_str() {
                "invalid_arguments" => Ok(ToolFailureCategory::InvalidArguments),
                "unavailable" => Ok(ToolFailureCategory::Unavailable),
                "timeout" => Ok(ToolFailureCategory::Timeout),
                "cancelled" => Ok(ToolFailureCategory::Cancelled),
                "transient" => Ok(ToolFailureCategory::Transient),
                "permanent" => Ok(ToolFailureCategory::Permanent),
                "partial_side_effect" => Ok(ToolFailureCategory::PartialSideEffect),
                _ => Err("unknown tool failure category".to_string()),
            }?;
            let recovery = match failure.recovery.as_str() {
                "correct_arguments" => Ok(ToolRecoveryAction::CorrectArguments),
                "retry" => Ok(ToolRecoveryAction::Retry),
                "restore_then_retry" => Ok(ToolRecoveryAction::RestoreThenRetry),
                "verify_then_retry" => Ok(ToolRecoveryAction::VerifyThenRetry),
                "stop" => Ok(ToolRecoveryAction::Stop),
                _ => Err("unknown tool recovery action".to_string()),
            }?;
            let side_effect = match failure.side_effect.as_str() {
                "none" => Ok(ToolSideEffect::None),
                "possible" => Ok(ToolSideEffect::Possible),
                "confirmed" => Ok(ToolSideEffect::Confirmed),
                _ => Err("unknown tool side-effect status".to_string()),
            }?;
            Ok(ToolFailure {
                category,
                recovery,
                side_effect,
                retry_after_ms: failure.retry_after_ms.and_then(|value| value.to_u64()),
                idempotency_key: failure.idempotency_key,
                postcondition: failure.postcondition,
            })
        })
        .transpose()?;
    let artifact =
        wire.artifact
            .map(
                |artifact| -> std::result::Result<
                    echo_agent::tools::artifact::ToolOutputArtifactRef,
                    String,
                > {
                    Ok(echo_agent::tools::artifact::ToolOutputArtifactRef {
                        path: wire_path_to_path(artifact.path)?,
                        artifact_bytes: artifact
                            .artifact_bytes
                            .to_u64()
                            .ok_or_else(|| "invalid artifact byte count".to_string())?,
                        payload_bytes: artifact
                            .payload_bytes
                            .to_u64()
                            .ok_or_else(|| "invalid payload byte count".to_string())?,
                        sha256: artifact.sha256,
                        retention: artifact.retention,
                    })
                },
            )
            .transpose()?;
    Ok(ToolResult {
        kind,
        success: wire.success,
        output: wire.output,
        error: wire.error,
        failure,
        data: wire
            .data
            .map(|value| value.into_json().map_err(|error| error.to_string()))
            .transpose()?,
        truncated: wire.truncated,
        mime_type: wire.mime_type,
        artifact,
        metadata: wire.metadata.into_iter().collect(),
        model_content: wire
            .model_content
            .into_iter()
            .map(|content| match content {
                ToolResultContentWire::ImageUrl { url, detail } => {
                    ToolResultContent::ImageUrl { url, detail }
                }
            })
            .collect(),
    })
}

fn tool_stream_event_from_wire(
    wire: ToolStreamEventWire,
) -> std::result::Result<ToolStreamEvent, String> {
    Ok(match wire {
        ToolStreamEventWire::Progress { message, percent } => {
            ToolStreamEvent::Progress { message, percent }
        }
        ToolStreamEventWire::Output { channel, chunk } => ToolStreamEvent::Output {
            channel: match channel.as_str() {
                "stdout" => echo_agent::tools::ToolOutputChannel::Stdout,
                "stderr" => echo_agent::tools::ToolOutputChannel::Stderr,
                "log" => echo_agent::tools::ToolOutputChannel::Log,
                _ => return Err("unknown tool output channel".to_string()),
            },
            chunk,
        },
        ToolStreamEventWire::Complete { result } => {
            ToolStreamEvent::Complete(tool_result_from_wire(result)?)
        }
    })
}

fn tool_stream_chunk_from_wire(
    wire: ToolStreamChunkWire,
) -> std::result::Result<ToolStreamEvent, String> {
    match wire {
        ToolStreamChunkWire::Progress { message, percent } => {
            Ok(ToolStreamEvent::Progress { message, percent })
        }
        ToolStreamChunkWire::Output { channel, chunk } => {
            tool_stream_event_from_wire(ToolStreamEventWire::Output { channel, chunk })
        }
    }
}

/// Reverse projections are retained as round-trip helpers for future Host-
/// originated extension events; current bridge direction only consumes SDK
/// results.
#[allow(dead_code)]
fn tool_result_wire_from_framework(
    result: &ToolResult,
) -> std::result::Result<ToolResultWire, String> {
    use echo_agent::tools::ToolResultKind;
    let kind = match &result.kind {
        ToolResultKind::Text => ToolResultKindWire::Text,
        ToolResultKind::Json => ToolResultKindWire::Json,
        ToolResultKind::Image { mime_type } => ToolResultKindWire::Image {
            mime_type: mime_type.clone(),
        },
        ToolResultKind::Table { columns, rows } => ToolResultKindWire::Table {
            columns: columns.clone(),
            rows: rows.clone(),
        },
        ToolResultKind::Diff { unified_diff } => ToolResultKindWire::Diff {
            unified_diff: unified_diff.clone(),
        },
        ToolResultKind::FileReference { path } => {
            ToolResultKindWire::FileReference { path: path.clone() }
        }
        ToolResultKind::CommandOutput { exit_code } => ToolResultKindWire::CommandOutput {
            exit_code: *exit_code,
        },
        ToolResultKind::SkillActivation { name } => {
            ToolResultKindWire::SkillActivation { name: name.clone() }
        }
        ToolResultKind::StructuredError { error_code } => ToolResultKindWire::StructuredError {
            error_code: error_code.clone(),
        },
    };
    let failure = result.failure.as_ref().map(|failure| ToolFailureWire {
        category: failure.category.as_str().to_string(),
        recovery: failure.recovery.as_str().to_string(),
        side_effect: match failure.side_effect {
            echo_agent::tools::ToolSideEffect::None => "none",
            echo_agent::tools::ToolSideEffect::Possible => "possible",
            echo_agent::tools::ToolSideEffect::Confirmed => "confirmed",
        }
        .to_string(),
        retry_after_ms: failure.retry_after_ms.map(WireU64::from_u64),
        idempotency_key: failure.idempotency_key.clone(),
        postcondition: failure.postcondition.clone(),
    });
    let artifact = result
        .artifact
        .as_ref()
        .map(|artifact| ToolOutputArtifactRefWire {
            path: wire_path(&artifact.path),
            artifact_bytes: WireU64::from_u64(artifact.artifact_bytes),
            payload_bytes: WireU64::from_u64(artifact.payload_bytes),
            sha256: artifact.sha256.clone(),
            retention: artifact.retention.clone(),
        });
    let model_content = result
        .model_content
        .iter()
        .map(|content| match content {
            echo_agent::tools::ToolResultContent::ImageUrl { url, detail } => {
                ToolResultContentWire::ImageUrl {
                    url: url.clone(),
                    detail: detail.clone(),
                }
            }
        })
        .collect();
    Ok(ToolResultWire {
        kind,
        success: result.success,
        output: result.output.clone(),
        error: result.error.clone(),
        failure,
        data: result.data.as_ref().map(to_wire).transpose()?,
        truncated: result.truncated,
        mime_type: result.mime_type.clone(),
        artifact,
        metadata: result
            .metadata
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        model_content,
    })
}

#[allow(dead_code)]
fn tool_stream_event_wire_from_framework(
    event: &ToolStreamEvent,
) -> std::result::Result<ToolStreamEventWire, String> {
    Ok(match event {
        ToolStreamEvent::Progress { message, percent } => ToolStreamEventWire::Progress {
            message: message.clone(),
            percent: *percent,
        },
        ToolStreamEvent::Output { channel, chunk } => ToolStreamEventWire::Output {
            channel: match channel {
                echo_agent::tools::ToolOutputChannel::Stdout => "stdout",
                echo_agent::tools::ToolOutputChannel::Stderr => "stderr",
                echo_agent::tools::ToolOutputChannel::Log => "log",
            }
            .to_string(),
            chunk: chunk.clone(),
        },
        ToolStreamEvent::Complete(result) => ToolStreamEventWire::Complete {
            result: tool_result_wire_from_framework(result)?,
        },
    })
}

#[allow(dead_code)]
fn agent_event_wire_from_framework(
    event: &AgentEvent,
) -> std::result::Result<AgentEventWire, String> {
    Ok(match event {
        AgentEvent::Token(text) => AgentEventWire::Token { text: text.clone() },
        AgentEvent::ThinkStart => AgentEventWire::ThinkStart,
        AgentEvent::ThinkEnd {
            prompt_tokens,
            completion_tokens,
        } => AgentEventWire::ThinkEnd {
            prompt_tokens: WireU64::from_u64(u64::try_from(*prompt_tokens).unwrap_or(u64::MAX)),
            completion_tokens: WireU64::from_u64(
                u64::try_from(*completion_tokens).unwrap_or(u64::MAX),
            ),
        },
        AgentEvent::LlmUsage {
            model,
            prompt_tokens,
            completion_tokens,
            total_tokens,
            cached_prompt_tokens,
            cache_creation_prompt_tokens,
            usage_reported,
        } => AgentEventWire::LlmUsage {
            model: model.clone(),
            prompt_tokens: WireU64::from_u64(u64::try_from(*prompt_tokens).unwrap_or(u64::MAX)),
            completion_tokens: WireU64::from_u64(
                u64::try_from(*completion_tokens).unwrap_or(u64::MAX),
            ),
            total_tokens: WireU64::from_u64(u64::try_from(*total_tokens).unwrap_or(u64::MAX)),
            cached_prompt_tokens: WireU64::from_u64(
                u64::try_from(*cached_prompt_tokens).unwrap_or(u64::MAX),
            ),
            cache_creation_prompt_tokens: WireU64::from_u64(
                u64::try_from(*cache_creation_prompt_tokens).unwrap_or(u64::MAX),
            ),
            usage_reported: *usage_reported,
        },
        AgentEvent::BudgetDecision {
            decision,
            reason,
            iteration,
            reported_model_tokens,
            usage_complete,
        } => AgentEventWire::BudgetDecision {
            decision: to_wire(decision)?,
            reason: reason.clone(),
            iteration: WireU64::from_u64(u64::try_from(*iteration).unwrap_or(u64::MAX)),
            reported_model_tokens: WireU64::from_u64(
                u64::try_from(*reported_model_tokens).unwrap_or(u64::MAX),
            ),
            usage_complete: *usage_complete,
        },
        AgentEvent::ToolCall {
            call_id,
            invocation,
        } => AgentEventWire::ToolCall {
            call_id: call_id.clone(),
            invocation: to_wire(invocation)?,
        },
        AgentEvent::ToolResult {
            call_id,
            name,
            result,
        } => AgentEventWire::ToolResult {
            call_id: call_id.clone(),
            name: name.clone(),
            result: tool_result_wire_from_framework(result)?,
        },
        AgentEvent::ToolStream {
            call_id,
            name,
            event,
        } => AgentEventWire::ToolStream {
            call_id: call_id.clone(),
            name: name.clone(),
            event: tool_stream_event_wire_from_framework(event)?,
        },
        AgentEvent::ToolBatchStart { tool_count } => AgentEventWire::ToolBatchStart {
            tool_count: WireU64::from_u64(u64::try_from(*tool_count).unwrap_or(u64::MAX)),
        },
        AgentEvent::ToolBatchEnd => AgentEventWire::ToolBatchEnd,
        AgentEvent::GuardTriggered { guard, blocked } => AgentEventWire::GuardTriggered {
            guard: guard.clone(),
            blocked: *blocked,
        },
        AgentEvent::MemoryRecalled { count } => AgentEventWire::MemoryRecalled {
            count: WireU64::from_u64(u64::try_from(*count).unwrap_or(u64::MAX)),
        },
        AgentEvent::ContextCompressed {
            before_count,
            after_count,
            before_tokens,
            after_tokens,
        } => AgentEventWire::ContextCompressed {
            before_count: WireU64::from_u64(u64::try_from(*before_count).unwrap_or(u64::MAX)),
            after_count: WireU64::from_u64(u64::try_from(*after_count).unwrap_or(u64::MAX)),
            before_tokens: WireU64::from_u64(u64::try_from(*before_tokens).unwrap_or(u64::MAX)),
            after_tokens: WireU64::from_u64(u64::try_from(*after_tokens).unwrap_or(u64::MAX)),
        },
        AgentEvent::Chart { spec } => AgentEventWire::Chart {
            spec: to_wire(spec)?,
        },
        AgentEvent::Error {
            source,
            message,
            failure,
        } => AgentEventWire::Error {
            source: source.clone(),
            message: message.clone(),
            failure: to_wire(failure)?,
        },
        AgentEvent::SafetyNotice {
            action,
            reason,
            risk,
            permission,
        } => AgentEventWire::SafetyNotice {
            action: action.clone(),
            reason: reason.clone(),
            risk: risk.clone(),
            permission: permission.clone(),
        },
        AgentEvent::ParameterError {
            tool,
            parameter,
            expected,
            got,
        } => AgentEventWire::ParameterError {
            tool: tool.clone(),
            parameter: parameter.clone(),
            expected: expected.clone(),
            got: got.clone(),
        },
        AgentEvent::FinalAnswer(text) => AgentEventWire::FinalAnswer { text: text.clone() },
        AgentEvent::Cancelled => AgentEventWire::Cancelled,
        _ => return Err("unsupported framework AgentEvent variant".to_string()),
    })
}

fn agent_event_from_wire(wire: AgentEventWire) -> std::result::Result<AgentEvent, String> {
    Ok(match wire {
        AgentEventWire::Token { text } => AgentEvent::Token(text),
        AgentEventWire::ThinkStart => AgentEvent::ThinkStart,
        AgentEventWire::ThinkEnd {
            prompt_tokens,
            completion_tokens,
        } => AgentEvent::ThinkEnd {
            prompt_tokens: wire_usize(prompt_tokens, "prompt token count")?,
            completion_tokens: wire_usize(completion_tokens, "completion token count")?,
        },
        AgentEventWire::LlmUsage {
            model,
            prompt_tokens,
            completion_tokens,
            total_tokens,
            cached_prompt_tokens,
            cache_creation_prompt_tokens,
            usage_reported,
        } => AgentEvent::LlmUsage {
            model,
            prompt_tokens: wire_usize(prompt_tokens, "prompt token count")?,
            completion_tokens: wire_usize(completion_tokens, "completion token count")?,
            total_tokens: wire_usize(total_tokens, "total token count")?,
            cached_prompt_tokens: wire_usize(cached_prompt_tokens, "cached prompt token count")?,
            cache_creation_prompt_tokens: wire_usize(
                cache_creation_prompt_tokens,
                "cache creation prompt token count",
            )?,
            usage_reported,
        },
        AgentEventWire::BudgetDecision {
            decision,
            reason,
            iteration,
            reported_model_tokens,
            usage_complete,
        } => AgentEvent::BudgetDecision {
            decision: from_wire(decision)?,
            reason,
            iteration: wire_usize(iteration, "budget iteration")?,
            reported_model_tokens: wire_usize(reported_model_tokens, "reported model token count")?,
            usage_complete,
        },
        AgentEventWire::ToolCall {
            call_id,
            invocation,
        } => AgentEvent::ToolCall {
            call_id,
            invocation: from_wire(invocation)?,
        },
        AgentEventWire::ToolResult {
            call_id,
            name,
            result,
        } => AgentEvent::ToolResult {
            call_id,
            name,
            result: tool_result_from_wire(result)?,
        },
        AgentEventWire::ToolStream {
            call_id,
            name,
            event,
        } => AgentEvent::ToolStream {
            call_id,
            name,
            event: tool_stream_event_from_wire(event)?,
        },
        AgentEventWire::ToolBatchStart { tool_count } => AgentEvent::ToolBatchStart {
            tool_count: wire_usize(tool_count, "tool batch count")?,
        },
        AgentEventWire::ToolBatchEnd => AgentEvent::ToolBatchEnd,
        AgentEventWire::GuardTriggered { guard, blocked } => {
            AgentEvent::GuardTriggered { guard, blocked }
        }
        AgentEventWire::MemoryRecalled { count } => AgentEvent::MemoryRecalled {
            count: wire_usize(count, "recalled memory count")?,
        },
        AgentEventWire::ContextCompressed {
            before_count,
            after_count,
            before_tokens,
            after_tokens,
        } => AgentEvent::ContextCompressed {
            before_count: wire_usize(before_count, "pre-compression message count")?,
            after_count: wire_usize(after_count, "post-compression message count")?,
            before_tokens: wire_usize(before_tokens, "pre-compression token count")?,
            after_tokens: wire_usize(after_tokens, "post-compression token count")?,
        },
        AgentEventWire::Chart { spec } => AgentEvent::Chart {
            spec: from_wire(spec)?,
        },
        AgentEventWire::Error {
            source,
            message,
            failure,
        } => AgentEvent::Error {
            source,
            message,
            failure: from_wire(failure)?,
        },
        AgentEventWire::SafetyNotice {
            action,
            reason,
            risk,
            permission,
        } => AgentEvent::SafetyNotice {
            action,
            reason,
            risk,
            permission,
        },
        AgentEventWire::ParameterError {
            tool,
            parameter,
            expected,
            got,
        } => AgentEvent::ParameterError {
            tool,
            parameter,
            expected,
            got,
        },
        AgentEventWire::FinalAnswer { text } => AgentEvent::FinalAnswer(text),
        AgentEventWire::Cancelled => AgentEvent::Cancelled,
    })
}

fn agent_stream_chunk_from_wire(
    wire: AgentStreamChunkWire,
) -> std::result::Result<AgentEvent, String> {
    let value = serde_json::to_value(wire).map_err(|error| error.to_string())?;
    let event = serde_json::from_value(value).map_err(|error| error.to_string())?;
    agent_event_from_wire(event)
}

fn agent_stream_terminal_from_wire(
    wire: AgentStreamTerminalWire,
) -> std::result::Result<AgentEvent, String> {
    let value = serde_json::to_value(wire).map_err(|error| error.to_string())?;
    let event = serde_json::from_value(value).map_err(|error| error.to_string())?;
    agent_event_from_wire(event)
}

fn wire_usize(value: WireU64, field: &str) -> std::result::Result<usize, String> {
    let value = value.to_u64().ok_or_else(|| format!("invalid {field}"))?;
    usize::try_from(value).map_err(|_| format!("{field} exceeds this platform's usize range"))
}

/// Thin `Tool` proxy: descriptor facts answered locally, execution and
/// streaming delegated to the registered implementation. The ToolManager
/// keeps owning permissions, retry and sandbox policy.
pub(crate) struct ExtensionToolProxy {
    bridge: Arc<ExtensionBridge>,
    extension: WireHandle,
    descriptor: ExtensionDescriptor,
    session_id: String,
}

impl ExtensionToolProxy {
    pub(crate) fn new(
        bridge: Arc<ExtensionBridge>,
        extension: WireHandle,
        session_id: String,
    ) -> Option<Self> {
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
            session_id,
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
        let input = ToolExecuteInput {
            parameters: to_wire(parameters).map_err(ReactError::Other)?,
            context: context
                .map(tool_context_wire)
                .transpose()
                .map_err(ReactError::Other)?,
        };
        let invocation = match operation {
            ExtensionOperation::ToolExecute => ExtensionInvocation::ToolExecute(input),
            _ => {
                return Err(ReactError::Other(
                    "invalid non-streaming tool operation".to_string(),
                ));
            }
        };
        let value = self
            .bridge
            .invoke_once(
                &self.extension,
                context.and_then(invocation_context),
                invocation,
                context
                    .and_then(|context| context.cancel.clone())
                    .unwrap_or_else(cancelled_token_arc)
                    .as_ref()
                    .clone(),
            )
            .await
            .map_err(react_error)?;
        let ExtensionResult::ToolExecute(result) = value else {
            return Err(ReactError::Other(
                "tool extension returned the wrong result variant".to_string(),
            ));
        };
        tool_result_from_wire(result).map_err(ReactError::Other)
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

    fn permissions(&self) -> Vec<echo_agent::tools::permission::ToolPermission> {
        match &self.descriptor {
            ExtensionDescriptor::Tool {
                required_permissions,
                ..
            } => required_permissions
                .iter()
                .map(|permission| match permission {
                    echo_sdk_protocol::methods::ToolPermissionWire::Read => {
                        echo_agent::tools::permission::ToolPermission::Read
                    }
                    echo_sdk_protocol::methods::ToolPermissionWire::Write => {
                        echo_agent::tools::permission::ToolPermission::Write
                    }
                    echo_sdk_protocol::methods::ToolPermissionWire::Network => {
                        echo_agent::tools::permission::ToolPermission::Network
                    }
                    echo_sdk_protocol::methods::ToolPermissionWire::Execute => {
                        echo_agent::tools::permission::ToolPermission::Execute
                    }
                    echo_sdk_protocol::methods::ToolPermissionWire::Sensitive => {
                        echo_agent::tools::permission::ToolPermission::Sensitive
                    }
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    fn risk_level(&self) -> echo_agent::tools::ToolRiskLevel {
        match &self.descriptor {
            ExtensionDescriptor::Tool { risk_level, .. } => match risk_level {
                echo_sdk_protocol::methods::ToolRiskLevelWire::ReadOnly => {
                    echo_agent::tools::ToolRiskLevel::ReadOnly
                }
                echo_sdk_protocol::methods::ToolRiskLevelWire::Standard => {
                    echo_agent::tools::ToolRiskLevel::Standard
                }
                echo_sdk_protocol::methods::ToolRiskLevelWire::Dangerous => {
                    echo_agent::tools::ToolRiskLevel::Dangerous
                }
            },
            _ => echo_agent::tools::ToolRiskLevel::Standard,
        }
    }

    #[doc(hidden)]
    fn required_input_modalities_owned(&self) -> Vec<echo_agent::llm::ModelInputModality> {
        match &self.descriptor {
            ExtensionDescriptor::Tool {
                required_input_modalities,
                ..
            } => required_input_modalities
                .iter()
                .map(|modality| match modality {
                    echo_sdk_protocol::methods::ModelModalityWire::Text => {
                        echo_agent::llm::ModelInputModality::Text
                    }
                    echo_sdk_protocol::methods::ModelModalityWire::Image => {
                        echo_agent::llm::ModelInputModality::Image
                    }
                    echo_sdk_protocol::methods::ModelModalityWire::Audio => {
                        echo_agent::llm::ModelInputModality::Audio
                    }
                    echo_sdk_protocol::methods::ModelModalityWire::Video => {
                        echo_agent::llm::ModelInputModality::Video
                    }
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    fn exempt_from_batch_timeout(&self) -> bool {
        match &self.descriptor {
            ExtensionDescriptor::Tool {
                exempt_from_batch_timeout,
                ..
            } => *exempt_from_batch_timeout,
            _ => false,
        }
    }

    fn allows_parallel_batch_execution(&self) -> bool {
        match &self.descriptor {
            ExtensionDescriptor::Tool {
                allows_parallel_batch_execution,
                ..
            } => *allows_parallel_batch_execution,
            _ => true,
        }
    }

    fn manages_own_timeout(&self) -> bool {
        match &self.descriptor {
            ExtensionDescriptor::Tool {
                manages_own_timeout,
                ..
            } => *manages_own_timeout,
            _ => false,
        }
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
            let input = ToolExecuteInput {
                parameters: to_wire(parameters).map_err(ReactError::Other)?,
                context: Some(tool_context_wire(&context).map_err(ReactError::Other)?),
            };
            let cancellation = context
                .cancel
                .clone()
                .unwrap_or_else(cancelled_token_arc)
                .as_ref()
                .clone();
            let (receiver, stream, invocation_id, cancellation, lease, sink) = self
                .bridge
                .invoke_stream(
                    &self.extension,
                    Some(ExtensionInvocationContext {
                        session_id: context.conversation_id.clone(),
                        run_id: context.run_id.clone(),
                        stream_id: None,
                        turn_id: context.turn_id.clone(),
                        message_id: context.message_id.clone(),
                        execution_id: context.execution_id.clone(),
                        call_id: context.call_id.clone(),
                    })
                    .filter(|value| {
                        value.session_id.is_some()
                            || value.run_id.is_some()
                            || value.turn_id.is_some()
                            || value.message_id.is_some()
                            || value.execution_id.is_some()
                            || value.call_id.is_some()
                    }),
                    ExtensionInvocation::ToolExecuteStream(input),
                    cancellation,
                )
                .await
                .map_err(react_error)?;
            Ok(Box::pin(futures::StreamExt::map(
                extension_event_stream(
                    self.bridge.clone(),
                    stream,
                    invocation_id,
                    cancellation,
                    lease,
                    sink,
                    receiver,
                ),
                |event: ExtensionStreamEvent| match event {
                    ExtensionStreamEvent::Chunk { value, .. } => {
                        let ExtensionStreamChunkValue::Tool(event) = value else {
                            return ToolStreamEvent::Complete(ToolResult::error(
                                "tool stream received a non-tool payload",
                            ));
                        };
                        tool_stream_chunk_from_wire(event).unwrap_or_else(|error| {
                            ToolStreamEvent::Complete(ToolResult::error(error))
                        })
                    }
                    ExtensionStreamEvent::Complete { value, .. } => {
                        let ExtensionStreamCompleteValue::Tool(result) = value else {
                            return ToolStreamEvent::Complete(ToolResult::error(
                                "tool stream received a non-tool terminal",
                            ));
                        };
                        ToolStreamEvent::Complete(
                            tool_result_from_wire(result).unwrap_or_else(ToolResult::error),
                        )
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
            let input = ToolValidateInput {
                parameters: to_wire(parameters).map_err(ReactError::Other)?,
            };
            let value = self
                .bridge
                .invoke_once(
                    &self.extension,
                    Some(session_invocation_context(&self.session_id)),
                    ExtensionInvocation::ToolValidateParameters(input),
                    self.bridge.connection_cancellation(),
                )
                .await
                .map_err(react_error)?;
            // The validation contract is `null` when the parameters are
            // valid and a bounded rejection reason otherwise.
            let ExtensionResult::ToolValidateParameters(result) = value else {
                return Err(ReactError::Other(
                    "tool validation returned the wrong result variant".to_string(),
                ));
            };
            match result {
                Some(reason) => Err(ReactError::Other(reason)),
                None => Ok(()),
            }
        })
    }
}

// ── LlmClient proxy ─────────────────────────────────────────────────────────

fn cache_hints_json(hints: &echo_agent::llm::cache::CacheHints) -> serde_json::Value {
    let breakpoints = hints
        .breakpoints
        .iter()
        .map(|target| match target {
            echo_agent::llm::cache::BreakpointTarget::SystemLastBlock => {
                serde_json::json!({"kind": "system_last_block"})
            }
            echo_agent::llm::cache::BreakpointTarget::ToolsLastTool => {
                serde_json::json!({"kind": "tools_last_tool"})
            }
            echo_agent::llm::cache::BreakpointTarget::HistoryIndex(index) => {
                serde_json::json!({"kind": "history_index", "index": index})
            }
            echo_agent::llm::cache::BreakpointTarget::HistoryLastStable => {
                serde_json::json!({"kind": "history_last_stable"})
            }
        })
        .collect::<Vec<_>>();
    let range = |range: echo_agent::llm::cache::SegmentRange| serde_json::json!({"start": range.start, "end": range.end});
    serde_json::json!({
        "breakpoints": breakpoints,
        "stable_prefix_hash": hints.stable_prefix_hash,
        "segments": {
            "system": range(hints.segments.system),
            "canonical": range(hints.segments.canonical),
            "history": range(hints.segments.history),
            "runtime_context": range(hints.segments.runtime_context),
        }
    })
}

fn provider_capabilities(
    capabilities: &echo_sdk_protocol::methods::LlmCapabilitiesWire,
) -> echo_agent::llm::ProviderCapabilities {
    let tokenizer_name = match capabilities.tokenizer_name.as_deref() {
        Some("cl100k_base") => Some("cl100k_base"),
        Some("o200k_base") => Some("o200k_base"),
        Some("claude") => Some("claude"),
        _ => None,
    };
    echo_agent::llm::ProviderCapabilities {
        streaming_tool_calls: capabilities.streaming_tool_calls,
        named_sse_events: capabilities.named_sse_events,
        reasoning_content: capabilities.reasoning_content,
        image_input: capabilities.image_input,
        system_as_top_level: capabilities.system_as_top_level,
        ndjson_streaming: capabilities.ndjson_streaming,
        tool_support: capabilities.tool_support,
        structured_output: capabilities.structured_output,
        requires_version_header: capabilities.requires_version_header,
        supports_parallel_tool_calls: capabilities.supports_parallel_tool_calls,
        supports_tool_choice_none: capabilities.supports_tool_choice_none,
        tokenizer_name,
    }
}

fn reasoning_block_wire(block: &echo_agent::llm::types::ReasoningBlock) -> LlmReasoningBlockWire {
    match block {
        echo_agent::llm::types::ReasoningBlock::Signed {
            thinking,
            signature,
        } => LlmReasoningBlockWire::Signed {
            thinking: thinking.clone(),
            signature: signature.clone(),
        },
        echo_agent::llm::types::ReasoningBlock::Redacted { data } => {
            LlmReasoningBlockWire::Redacted { data: data.clone() }
        }
        echo_agent::llm::types::ReasoningBlock::Opaque {
            provider,
            id,
            data,
            summary,
        } => LlmReasoningBlockWire::Opaque {
            provider: provider.clone(),
            id: id.clone(),
            data: data.clone(),
            summary: summary.clone(),
        },
    }
}

fn reasoning_block_from_wire(
    block: LlmReasoningBlockWire,
) -> echo_agent::llm::types::ReasoningBlock {
    match block {
        LlmReasoningBlockWire::Signed {
            thinking,
            signature,
        } => echo_agent::llm::types::ReasoningBlock::Signed {
            thinking,
            signature,
        },
        LlmReasoningBlockWire::Redacted { data } => {
            echo_agent::llm::types::ReasoningBlock::Redacted { data }
        }
        LlmReasoningBlockWire::Opaque {
            provider,
            id,
            data,
            summary,
        } => echo_agent::llm::types::ReasoningBlock::Opaque {
            provider,
            id,
            data,
            summary,
        },
    }
}

fn message_wire(
    message: &echo_agent::llm::types::Message,
) -> std::result::Result<LlmMessageWire, String> {
    Ok(LlmMessageWire {
        role: message.role.as_str().to_string(),
        content: to_wire(&message.content)?,
        tool_calls: message.tool_calls.as_ref().map(|calls| {
            calls
                .iter()
                .map(|call| LlmToolCallWire {
                    id: call.id.clone(),
                    call_type: call.call_type.clone(),
                    function_name: call.function.name.clone(),
                    arguments: call.function.arguments.clone(),
                })
                .collect()
        }),
        name: message.name.clone(),
        tool_call_id: message.tool_call_id.clone(),
        reasoning_content: message.reasoning_content.clone(),
        reasoning_blocks: message
            .reasoning_blocks
            .as_ref()
            .map(|blocks| blocks.iter().map(reasoning_block_wire).collect()),
    })
}

fn message_from_wire(
    message: LlmMessageWire,
) -> std::result::Result<echo_agent::llm::types::Message, String> {
    Ok(echo_agent::llm::types::Message {
        role: message.role.as_str().into(),
        content: from_wire(message.content)?,
        tool_calls: message.tool_calls.map(|calls| {
            calls
                .into_iter()
                .map(|call| echo_agent::llm::types::ToolCall {
                    id: call.id,
                    call_type: call.call_type,
                    function: echo_agent::llm::types::FunctionCall {
                        name: call.function_name,
                        arguments: call.arguments,
                    },
                })
                .collect()
        }),
        name: message.name,
        tool_call_id: message.tool_call_id,
        reasoning_content: message.reasoning_content,
        reasoning_blocks: message
            .reasoning_blocks
            .map(|blocks| blocks.into_iter().map(reasoning_block_from_wire).collect()),
    })
}

#[cfg(test)]
fn delta_tool_call_wire(delta: &echo_agent::llm::types::DeltaToolCall) -> LlmDeltaToolCallWire {
    LlmDeltaToolCallWire {
        index: delta.index,
        id: delta.id.clone(),
        call_type: delta.call_type.clone(),
        function: delta
            .function
            .as_ref()
            .map(|function| LlmDeltaFunctionWire {
                name: function.name.clone(),
                arguments: function.arguments.clone(),
            }),
    }
}

fn delta_tool_call_from_wire(wire: LlmDeltaToolCallWire) -> echo_agent::llm::types::DeltaToolCall {
    echo_agent::llm::types::DeltaToolCall {
        index: wire.index,
        id: wire.id,
        call_type: wire.call_type,
        function: wire
            .function
            .map(|function| echo_agent::llm::types::DeltaFunctionCall {
                name: function.name,
                arguments: function.arguments,
            }),
    }
}

#[cfg(test)]
fn chat_chunk_wire(
    chunk: &echo_agent::llm::ChatChunk,
) -> std::result::Result<LlmChatChunkWire, String> {
    Ok(LlmChatChunkWire {
        role: chunk.delta.role.clone(),
        content: chunk.delta.content.clone(),
        reasoning_content: chunk.delta.reasoning_content.clone(),
        reasoning_blocks: chunk
            .delta
            .reasoning_blocks
            .as_ref()
            .map(|blocks| blocks.iter().map(reasoning_block_wire).collect()),
        tool_calls: chunk
            .delta
            .tool_calls
            .as_ref()
            .map(|calls| calls.iter().map(delta_tool_call_wire).collect()),
        finish_reason: chunk.finish_reason.clone(),
        usage: chunk
            .usage
            .as_ref()
            .map(|usage| to_wire(usage).map(|value| LlmUsageWire { value }))
            .transpose()?,
    })
}

fn chat_chunk_from_wire(
    wire: LlmChatChunkWire,
) -> std::result::Result<echo_agent::llm::ChatChunk, String> {
    let delta = echo_agent::llm::types::DeltaMessage {
        role: wire.role,
        content: wire.content,
        reasoning_content: wire.reasoning_content,
        reasoning_blocks: wire
            .reasoning_blocks
            .map(|blocks| blocks.into_iter().map(reasoning_block_from_wire).collect()),
        tool_calls: wire
            .tool_calls
            .map(|calls| calls.into_iter().map(delta_tool_call_from_wire).collect()),
    };
    Ok(echo_agent::llm::ChatChunk {
        delta,
        finish_reason: wire.finish_reason,
        usage: wire.usage.map(|usage| from_wire(usage.value)).transpose()?,
    })
}

fn chat_chunk_from_stream_chunk(
    wire: LlmStreamChunkWire,
) -> std::result::Result<echo_agent::llm::ChatChunk, String> {
    chat_chunk_from_wire(LlmChatChunkWire {
        role: wire.role,
        content: wire.content,
        reasoning_content: wire.reasoning_content,
        reasoning_blocks: wire.reasoning_blocks,
        tool_calls: wire.tool_calls,
        finish_reason: None,
        usage: wire.usage,
    })
}

fn chat_chunk_from_stream_complete(
    wire: LlmStreamCompleteWire,
) -> std::result::Result<echo_agent::llm::ChatChunk, String> {
    chat_chunk_from_wire(LlmChatChunkWire {
        role: wire.role,
        content: wire.content,
        reasoning_content: wire.reasoning_content,
        reasoning_blocks: wire.reasoning_blocks,
        tool_calls: wire.tool_calls,
        finish_reason: Some(wire.finish_reason),
        usage: wire.usage,
    })
}

fn chat_response_from_wire(
    wire: LlmChatResponseWire,
) -> std::result::Result<echo_agent::llm::ChatResponse, String> {
    Ok(echo_agent::llm::ChatResponse {
        message: message_from_wire(wire.message)?,
        finish_reason: wire.finish_reason,
        usage: wire.usage.map(|usage| from_wire(usage.value)).transpose()?,
        raw: from_wire(wire.raw)?,
    })
}

fn chat_response_into_chunk(
    response: echo_agent::llm::ChatResponse,
) -> std::result::Result<echo_agent::llm::ChatChunk, String> {
    let finish_reason = response
        .finish_reason
        .filter(|reason| !reason.trim().is_empty() && reason.chars().count() <= 256)
        .ok_or_else(|| {
            "non-streaming LLM response cannot be adapted without a bounded finish reason"
                .to_string()
        })?;
    let tool_calls = response
        .message
        .tool_calls
        .map(|calls| {
            calls
                .into_iter()
                .enumerate()
                .map(|(index, call)| {
                    Ok(echo_agent::llm::types::DeltaToolCall {
                        index: u32::try_from(index)
                            .map_err(|_| "LLM response has too many tool calls".to_string())?,
                        id: Some(call.id),
                        call_type: Some(call.call_type),
                        function: Some(echo_agent::llm::types::DeltaFunctionCall {
                            name: Some(call.function.name),
                            arguments: Some(call.function.arguments),
                        }),
                    })
                })
                .collect::<std::result::Result<Vec<_>, String>>()
        })
        .transpose()?;
    Ok(echo_agent::llm::ChatChunk {
        delta: echo_agent::llm::types::DeltaMessage {
            role: Some(response.message.role.as_str().to_string()),
            content: response.message.content.as_text(),
            reasoning_content: response.message.reasoning_content,
            reasoning_blocks: response.message.reasoning_blocks,
            tool_calls,
        },
        finish_reason: Some(finish_reason),
        usage: response.usage,
    })
}

/// Thin `LlmClient` proxy: chat and chat_stream delegate to the registered
/// implementation; the descriptor answers model identity locally. The
/// AgentTurnDriver keeps deciding run terminals from the returned chunks.
pub(crate) struct ExtensionLlmClientProxy {
    bridge: Arc<ExtensionBridge>,
    extension: WireHandle,
    model_name: String,
    capabilities: echo_agent::llm::ProviderCapabilities,
    supports_streaming: bool,
    session_id: String,
}

impl ExtensionLlmClientProxy {
    pub(crate) fn new(
        bridge: Arc<ExtensionBridge>,
        extension: WireHandle,
        session_id: String,
    ) -> Option<Self> {
        let record = bridge.state().ok()?.handles.extension(&extension).ok()?;
        let ExtensionDescriptor::LlmClient {
            model_name,
            supports_streaming,
            capabilities,
            ..
        } = &record.descriptor
        else {
            return None;
        };
        Some(Self {
            bridge,
            extension,
            model_name: model_name.clone(),
            capabilities: provider_capabilities(capabilities),
            supports_streaming: *supports_streaming,
            session_id,
        })
    }

    fn request_wire(
        request: &echo_agent::llm::ChatRequest,
    ) -> std::result::Result<LlmChatRequestWire, String> {
        Ok(LlmChatRequestWire {
            messages: request
                .messages
                .iter()
                .map(message_wire)
                .collect::<std::result::Result<Vec<_>, _>>()?,
            temperature: request.temperature.map(f64::from),
            max_tokens: request.max_tokens,
            tools: request
                .tools
                .as_ref()
                .map(|tools| {
                    tools
                        .iter()
                        .map(|tool| {
                            Ok(LlmToolDefinitionWire {
                                tool_type: tool.tool_type.clone(),
                                name: tool.function.name.clone(),
                                description: tool.function.description.clone(),
                                parameters: to_wire(&tool.function.parameters)?,
                            })
                        })
                        .collect::<std::result::Result<Vec<_>, String>>()
                })
                .transpose()?,
            tool_choice: request.tool_choice.clone(),
            user_id: request.user_id.clone(),
            response_format: request.response_format.as_ref().map(to_wire).transpose()?,
            thinking: request.thinking.as_ref().map(to_wire).transpose()?,
            timeouts: request.timeouts.as_ref().map(to_wire).transpose()?,
            cache_hints: request
                .cache_hints
                .as_ref()
                .map(cache_hints_json)
                .map(to_wire)
                .transpose()?,
        })
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
                    Some(session_invocation_context(&self.session_id)),
                    ExtensionInvocation::LlmChat(
                        Self::request_wire(&request).map_err(ReactError::Other)?,
                    ),
                    cancellation,
                )
                .await
                .map_err(react_error)?;
            let ExtensionResult::LlmChat(response) = value else {
                return Err(ReactError::Other(
                    "llm extension returned the wrong result variant".to_string(),
                ));
            };
            chat_response_from_wire(response).map_err(ReactError::Other)
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
            if !self.supports_streaming {
                let response = self.chat(request).await?;
                let chunk = chat_response_into_chunk(response).map_err(ReactError::Other)?;
                return Ok(futures::stream::once(async move { Ok(chunk) }).boxed());
            }
            let cancellation = request.cancel_token.clone().unwrap_or_else(cancelled_token);
            let (receiver, stream, invocation_id, cancellation, lease, sink) = self
                .bridge
                .invoke_stream(
                    &self.extension,
                    Some(session_invocation_context(&self.session_id)),
                    ExtensionInvocation::LlmChatStream(
                        Self::request_wire(&request).map_err(ReactError::Other)?,
                    ),
                    cancellation,
                )
                .await
                .map_err(react_error)?;
            Ok(futures::StreamExt::map(
                extension_event_stream(
                    self.bridge.clone(),
                    stream,
                    invocation_id,
                    cancellation,
                    lease,
                    sink,
                    receiver,
                ),
                |event: ExtensionStreamEvent| {
                    let result: echo_agent::error::Result<echo_agent::llm::ChatChunk> = match event
                    {
                        ExtensionStreamEvent::Chunk { value, .. } => {
                            let ExtensionStreamChunkValue::Llm(chunk) = value else {
                                return Err(ReactError::Other(
                                    "llm stream received a non-llm payload".to_string(),
                                ));
                            };
                            chat_chunk_from_stream_chunk(chunk).map_err(ReactError::Other)
                        }
                        ExtensionStreamEvent::Complete { value, .. } => {
                            let ExtensionStreamCompleteValue::Llm(chunk) = value else {
                                return Err(ReactError::Other(
                                    "llm stream received a non-llm terminal".to_string(),
                                ));
                            };
                            chat_chunk_from_stream_complete(chunk).map_err(ReactError::Other)
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

    fn capabilities(&self) -> echo_agent::llm::ProviderCapabilities {
        self.capabilities
    }
}

// ── Store proxy ─────────────────────────────────────────────────────────────

fn store_item_from_wire(
    item: StoreItemWire,
) -> std::result::Result<echo_agent::memory::StoreItem, String> {
    Ok(echo_agent::memory::StoreItem {
        namespace: item.namespace,
        key: item.key,
        value: item.value.into_json().map_err(|error| error.to_string())?,
        created_at: item
            .created_at
            .to_u64()
            .ok_or_else(|| "invalid created_at".to_string())?,
        updated_at: item
            .updated_at
            .to_u64()
            .ok_or_else(|| "invalid updated_at".to_string())?,
        score: item.score,
        importance: item.importance,
        last_accessed: item.last_accessed.and_then(|value| value.to_u64()),
        expires_at: item.expires_at.and_then(|value| value.to_u64()),
    })
}

/// Thin `Store` proxy over the six store operations. Semantic or hybrid
/// searches against an implementation that did not declare them fail before
/// any callback leaves the process.
pub(crate) struct ExtensionStoreProxy {
    bridge: Arc<ExtensionBridge>,
    extension: WireHandle,
    declared_modes: Vec<echo_sdk_protocol::methods::SearchModeWire>,
    session_id: String,
}

impl ExtensionStoreProxy {
    pub(crate) fn new(
        bridge: Arc<ExtensionBridge>,
        extension: WireHandle,
        session_id: String,
    ) -> Option<Self> {
        let record = bridge.state().ok()?.handles.extension(&extension).ok()?;
        let ExtensionDescriptor::Store { search_modes, .. } = &record.descriptor else {
            return None;
        };
        Some(Self {
            bridge,
            extension,
            declared_modes: search_modes.clone(),
            session_id,
        })
    }

    fn declares(&self, mode: echo_sdk_protocol::methods::SearchModeWire) -> bool {
        (self.declared_modes.is_empty()
            && mode == echo_sdk_protocol::methods::SearchModeWire::Keyword)
            || self.declared_modes.contains(&mode)
    }

    async fn call_op(
        &self,
        invocation: ExtensionInvocation,
    ) -> echo_agent::error::Result<ExtensionResult> {
        self.bridge
            .invoke_once(
                &self.extension,
                Some(session_invocation_context(&self.session_id)),
                invocation,
                self.bridge.connection_cancellation(),
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
            let result = self
                .call_op(ExtensionInvocation::StorePut(StorePutInput {
                    namespace: namespace.iter().map(|part| (*part).to_string()).collect(),
                    key: key.to_string(),
                    value: to_wire(value).map_err(ReactError::Other)?,
                }))
                .await?;
            matches!(result, ExtensionResult::StorePut(_))
                .then_some(())
                .ok_or_else(|| {
                    ReactError::Other("store put returned the wrong result variant".to_string())
                })
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
            let result = self
                .call_op(ExtensionInvocation::StoreGet(StoreKeyInput {
                    namespace: namespace.iter().map(|part| (*part).to_string()).collect(),
                    key: key.to_string(),
                }))
                .await?;
            let ExtensionResult::StoreGet(item) = result else {
                return Err(ReactError::Other(
                    "store get returned the wrong result variant".to_string(),
                ));
            };
            item.map(store_item_from_wire)
                .transpose()
                .map_err(ReactError::Other)
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
            let result = self
                .call_op(ExtensionInvocation::StoreSearch(StoreSearchInput {
                    namespace: namespace.iter().map(|part| (*part).to_string()).collect(),
                    query: query.to_string(),
                    limit: WireU64::from_u64(u64::try_from(limit).unwrap_or(u64::MAX)),
                }))
                .await?;
            let ExtensionResult::StoreSearch(items) = result else {
                return Err(ReactError::Other(
                    "store search returned the wrong result variant".to_string(),
                ));
            };
            items
                .into_iter()
                .map(store_item_from_wire)
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(ReactError::Other)
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
                    StoreSearchModeWire::Keyword,
                ),
                echo_agent::memory::SearchMode::Semantic => (
                    echo_sdk_protocol::methods::SearchModeWire::Semantic,
                    StoreSearchModeWire::Semantic,
                ),
                echo_agent::memory::SearchMode::Hybrid { vector_weight } => (
                    echo_sdk_protocol::methods::SearchModeWire::Hybrid,
                    StoreSearchModeWire::Hybrid {
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
            let payload = StoreSearchQueryWire {
                text: query.text.to_string(),
                limit: WireU64::from_u64(u64::try_from(query.limit).unwrap_or(u64::MAX)),
                mode: wire_mode,
            };
            let result = self
                .call_op(ExtensionInvocation::StoreSearchWith(StoreSearchWithInput {
                    namespace: namespace.iter().map(|part| (*part).to_string()).collect(),
                    query: payload,
                }))
                .await?;
            let ExtensionResult::StoreSearchWith(items) = result else {
                return Err(ReactError::Other(
                    "store search_with returned the wrong result variant".to_string(),
                ));
            };
            items
                .into_iter()
                .map(store_item_from_wire)
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(ReactError::Other)
        })
    }

    fn delete<'a>(
        &'a self,
        namespace: &'a [&'a str],
        key: &'a str,
    ) -> futures::future::BoxFuture<'a, echo_agent::error::Result<bool>> {
        Box::pin(async move {
            let result = self
                .call_op(ExtensionInvocation::StoreDelete(StoreKeyInput {
                    namespace: namespace.iter().map(|part| (*part).to_string()).collect(),
                    key: key.to_string(),
                }))
                .await?;
            match result {
                ExtensionResult::StoreDelete(deleted) => Ok(deleted),
                _ => Err(ReactError::Other(
                    "store delete returned the wrong result variant".to_string(),
                )),
            }
        })
    }

    fn list_namespaces<'a>(
        &'a self,
        prefix: Option<&'a [&'a str]>,
    ) -> futures::future::BoxFuture<'a, echo_agent::error::Result<Vec<Vec<String>>>> {
        Box::pin(async move {
            let result = self
                .call_op(ExtensionInvocation::StoreListNamespaces(
                    StoreListNamespacesInput {
                        prefix: prefix
                            .map(|parts| parts.iter().map(|part| (*part).to_string()).collect()),
                    },
                ))
                .await?;
            match result {
                ExtensionResult::StoreListNamespaces(namespaces) => Ok(namespaces),
                _ => Err(ReactError::Other(
                    "store list_namespaces returned the wrong result variant".to_string(),
                )),
            }
        })
    }

    fn list<'a>(
        &'a self,
        namespace: &'a [&'a str],
    ) -> futures::future::BoxFuture<'a, echo_agent::error::Result<Vec<echo_agent::memory::StoreItem>>>
    {
        Box::pin(async move {
            let result = self
                .call_op(ExtensionInvocation::StoreList(StoreNamespaceInput {
                    namespace: namespace.iter().map(|part| (*part).to_string()).collect(),
                }))
                .await?;
            let ExtensionResult::StoreList(items) = result else {
                return Err(ReactError::Other(
                    "store list returned the wrong result variant".to_string(),
                ));
            };
            items
                .into_iter()
                .map(store_item_from_wire)
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(ReactError::Other)
        })
    }

    fn prune_expired<'a>(
        &'a self,
        namespace: &'a [&'a str],
    ) -> futures::future::BoxFuture<'a, echo_agent::error::Result<u64>> {
        Box::pin(async move {
            let result = self
                .call_op(ExtensionInvocation::StorePruneExpired(
                    StoreNamespaceInput {
                        namespace: namespace.iter().map(|part| (*part).to_string()).collect(),
                    },
                ))
                .await?;
            match result {
                ExtensionResult::StorePruneExpired(count) => count
                    .to_u64()
                    .ok_or_else(|| ReactError::Other("invalid prune count".to_string())),
                _ => Err(ReactError::Other(
                    "store prune returned the wrong result variant".to_string(),
                )),
            }
        })
    }

    fn dedup_by_content<'a>(
        &'a self,
        namespace: &'a [&'a str],
    ) -> futures::future::BoxFuture<'a, echo_agent::error::Result<u64>> {
        Box::pin(async move {
            let result = self
                .call_op(ExtensionInvocation::StoreDedupByContent(
                    StoreNamespaceInput {
                        namespace: namespace.iter().map(|part| (*part).to_string()).collect(),
                    },
                ))
                .await?;
            match result {
                ExtensionResult::StoreDedupByContent(count) => count
                    .to_u64()
                    .ok_or_else(|| ReactError::Other("invalid dedup count".to_string())),
                _ => Err(ReactError::Other(
                    "store dedup returned the wrong result variant".to_string(),
                )),
            }
        })
    }
}

// ── HumanLoopProvider proxy ─────────────────────────────────────────────────

fn scope_from_wire(scope: ApprovalScopeWire) -> echo_agent::human_loop::ApprovalScope {
    match scope {
        ApprovalScopeWire::Once => echo_agent::human_loop::ApprovalScope::Once,
        ApprovalScopeWire::Session => echo_agent::human_loop::ApprovalScope::Session,
        ApprovalScopeWire::SessionTool => echo_agent::human_loop::ApprovalScope::SessionTool,
    }
}

fn human_response_from_wire(
    wire: HumanLoopResponseWire,
) -> std::result::Result<echo_agent::human_loop::HumanLoopResponse, String> {
    use echo_agent::human_loop::HumanLoopResponse as Framework;
    Ok(match wire {
        HumanLoopResponseWire::Approved => Framework::Approved,
        HumanLoopResponseWire::ApprovedWithScope { scope } => Framework::ApprovedWithScope {
            scope: scope_from_wire(scope),
        },
        HumanLoopResponseWire::ModifiedArgs { args, scope } => Framework::ModifiedArgs {
            args: args.into_json().map_err(|error| error.to_string())?,
            scope: scope_from_wire(scope),
        },
        HumanLoopResponseWire::Rejected { reason } => Framework::Rejected { reason },
        HumanLoopResponseWire::Text { text } => Framework::Text(text),
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

/// Thin `HumanLoopProvider` proxy: request identity is preserved, the
/// response settles exactly once per invocation, and disconnect or timeout
/// becomes an explicit framework error instead of a fallback provider.
pub(crate) struct ExtensionHumanLoopProxy {
    bridge: Arc<ExtensionBridge>,
    extension: WireHandle,
    session_id: String,
}

impl ExtensionHumanLoopProxy {
    pub(crate) fn new(
        bridge: Arc<ExtensionBridge>,
        extension: WireHandle,
        session_id: String,
    ) -> Option<Self> {
        let record = bridge.state().ok()?.handles.extension(&extension).ok()?;
        matches!(
            record.descriptor,
            ExtensionDescriptor::HumanLoopProvider { .. }
        )
        .then(|| Self {
            bridge,
            extension,
            session_id,
        })
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
                kind: match request.kind {
                    echo_agent::human_loop::HumanLoopKind::Approval => HumanLoopKindWire::Approval,
                    echo_agent::human_loop::HumanLoopKind::Input => HumanLoopKindWire::Input,
                    echo_agent::human_loop::HumanLoopKind::Selection => {
                        HumanLoopKindWire::Selection
                    }
                },
                prompt: request.prompt.clone(),
                tool_name: request.tool_name.clone(),
                args: request
                    .args
                    .as_ref()
                    .map(to_wire)
                    .transpose()
                    .map_err(ReactError::Other)?,
                risk_level: request.risk_level.map(|risk| match risk {
                    echo_agent::human_loop::RiskLevel::Low => HumanRiskLevelWire::Low,
                    echo_agent::human_loop::RiskLevel::Medium => HumanRiskLevelWire::Medium,
                    echo_agent::human_loop::RiskLevel::High => HumanRiskLevelWire::High,
                    echo_agent::human_loop::RiskLevel::Critical => HumanRiskLevelWire::Critical,
                }),
                approval_context: request
                    .approval_context
                    .as_ref()
                    .map(to_wire)
                    .transpose()
                    .map_err(ReactError::Other)?,
                suggestions: request
                    .suggestions
                    .iter()
                    .map(to_wire)
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(ReactError::Other)?,
                timeout: request.timeout.map(wire_duration),
                task_id: request.task_id.clone(),
                options: request.options.clone(),
                context: request
                    .context
                    .as_ref()
                    .map(to_wire)
                    .transpose()
                    .map_err(ReactError::Other)?,
                phase: request.phase.clone(),
            };
            let value = self
                .bridge
                .invoke_once(
                    &self.extension,
                    Some(session_invocation_context(&self.session_id)),
                    ExtensionInvocation::HumanLoopRequest(payload),
                    self.bridge.connection_cancellation(),
                )
                .await
                .map_err(react_error)?;
            let ExtensionResult::HumanLoopRequest(response) = value else {
                return Err(ReactError::Other(
                    "human-loop extension returned the wrong result variant".to_string(),
                ));
            };
            human_response_from_wire(response).map_err(ReactError::Other)
        })
    }
}

// ── Hook bridge ─────────────────────────────────────────────────────────────

fn hook_result_from_wire(
    wire: HookResultWire,
) -> std::result::Result<echo_agent::hooks::HookResult, String> {
    let permission_mode_override = wire
        .permission_mode_override
        .map(|mode| {
            serde_json::from_value(serde_json::Value::String(mode))
                .map_err(|error| format!("invalid permission mode override: {error}"))
        })
        .transpose()?;
    Ok(echo_agent::hooks::HookResult {
        block: wire.block,
        block_reason: wire.block_reason,
        updated_input: wire
            .updated_input
            .map(|value| value.into_json().map_err(|error| error.to_string()))
            .transpose()?,
        messages: wire.messages,
        stop_propagation: wire.stop_propagation,
        permission_decision: wire.permission_decision.map(|decision| match decision {
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
        permission_mode_override,
        continue_reason: wire.continue_reason,
        injected_context: wire.injected_context,
        retry: wire.retry,
        metadata: wire
            .metadata
            .map(|value| value.into_json().map_err(|error| error.to_string()))
            .transpose()?,
        activate_skill: wire
            .activate_skill
            .map(|ActivateSkillWire { name, reason }| (name, reason)),
    })
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
        let context_identity = ExtensionInvocationContext {
            session_id: Some(context.session_id.clone()),
            run_id: context.run_id.clone(),
            stream_id: None,
            turn_id: None,
            message_id: None,
            execution_id: None,
            call_id: None,
        };
        Box::pin(async move {
            let input = match to_wire(&context) {
                Ok(context) => HookRunInput { context },
                Err(error) => {
                    tracing::warn!("hook context could not be projected: {error}");
                    return echo_agent::hooks::HookResult::deny(format!(
                        "extension hook context projection failed: {error}"
                    ));
                }
            };
            match bridge
                .invoke_once(
                    &extension,
                    Some(context_identity),
                    ExtensionInvocation::HookRun(input),
                    bridge.connection_cancellation(),
                )
                .await
            {
                Ok(ExtensionResult::HookRun(value)) => {
                    hook_result_from_wire(value).unwrap_or_else(|error| {
                        echo_agent::hooks::HookResult::deny(format!(
                            "extension hook returned an invalid result: {error}"
                        ))
                    })
                }
                Ok(_) => echo_agent::hooks::HookResult::deny(
                    "extension hook returned the wrong result variant".to_string(),
                ),
                Err(error) => {
                    tracing::warn!("hook extension {} failed: {}", extension.id, error.message);
                    echo_agent::hooks::HookResult::deny(format!(
                        "extension hook failed: {}",
                        error.message
                    ))
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
    session_id: String,
}

impl ExtensionAgentCallbackProxy {
    pub(crate) fn new(
        bridge: Arc<ExtensionBridge>,
        extension: WireHandle,
        session_id: String,
    ) -> Option<Self> {
        let record = bridge.state().ok()?.handles.extension(&extension).ok()?;
        matches!(record.descriptor, ExtensionDescriptor::AgentCallback { .. }).then(|| Self {
            bridge,
            extension,
            session_id,
        })
    }

    fn observe(
        &self,
        invocation: std::result::Result<ExtensionInvocation, String>,
    ) -> futures::future::BoxFuture<'_, ()> {
        let bridge = self.bridge.clone();
        let extension = self.extension.clone();
        let session_id = self.session_id.clone();
        Box::pin(async move {
            let invocation = match invocation {
                Ok(invocation) => invocation,
                Err(error) => {
                    tracing::warn!("agent callback input projection failed: {error}");
                    return;
                }
            };
            if let Err(error) = bridge
                .invoke_once(
                    &extension,
                    Some(session_invocation_context(&session_id)),
                    invocation,
                    bridge.connection_cancellation(),
                )
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
    fn callback_kind(&self) -> Option<&'static str> {
        Some("sdk_extension")
    }

    fn callback_id(&self) -> Option<&str> {
        Some(&self.extension.id)
    }

    fn on_think_start<'a>(
        &'a self,
        agent: &'a str,
        messages: &'a [echo_agent::llm::types::Message],
    ) -> futures::future::BoxFuture<'a, ()> {
        self.observe(
            messages
                .iter()
                .map(message_wire)
                .collect::<std::result::Result<Vec<_>, _>>()
                .map(|messages| {
                    ExtensionInvocation::CallbackOnThinkStart(CallbackThinkStartInput {
                        agent: agent.to_string(),
                        messages,
                    })
                }),
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
            steps
                .iter()
                .map(step_type_wire)
                .collect::<std::result::Result<Vec<_>, _>>()
                .map(|steps| {
                    ExtensionInvocation::CallbackOnThinkEnd(CallbackThinkEndInput {
                        agent: agent.to_string(),
                        steps,
                        prompt_tokens: WireU64::from_u64(
                            u64::try_from(prompt_tokens).unwrap_or(u64::MAX),
                        ),
                        completion_tokens: WireU64::from_u64(
                            u64::try_from(completion_tokens).unwrap_or(u64::MAX),
                        ),
                    })
                }),
        )
    }

    fn on_tool_start<'a>(
        &'a self,
        agent: &'a str,
        tool: &'a str,
        args: &'a serde_json::Value,
    ) -> futures::future::BoxFuture<'a, ()> {
        self.observe(to_wire(args).map(|args| {
            ExtensionInvocation::CallbackOnToolStart(CallbackToolStartInput {
                agent: agent.to_string(),
                tool: tool.to_string(),
                args,
            })
        }))
    }

    fn on_tool_end<'a>(
        &'a self,
        agent: &'a str,
        tool: &'a str,
        result: &'a str,
    ) -> futures::future::BoxFuture<'a, ()> {
        self.observe(Ok(ExtensionInvocation::CallbackOnToolEnd(
            CallbackToolEndInput {
                agent: agent.to_string(),
                tool: tool.to_string(),
                result: result.to_string(),
            },
        )))
    }

    fn on_tool_error<'a>(
        &'a self,
        agent: &'a str,
        tool: &'a str,
        error: &'a ReactError,
    ) -> futures::future::BoxFuture<'a, ()> {
        self.observe(Ok(ExtensionInvocation::CallbackOnToolError(
            CallbackToolErrorInput {
                agent: agent.to_string(),
                tool: tool.to_string(),
                error: error.to_string(),
            },
        )))
    }

    fn on_final_answer<'a>(
        &'a self,
        agent: &'a str,
        answer: &'a str,
    ) -> futures::future::BoxFuture<'a, ()> {
        self.observe(Ok(ExtensionInvocation::CallbackOnFinalAnswer(
            CallbackFinalAnswerInput {
                agent: agent.to_string(),
                answer: answer.to_string(),
            },
        )))
    }

    fn on_iteration<'a>(
        &'a self,
        agent: &'a str,
        iteration: usize,
    ) -> futures::future::BoxFuture<'a, ()> {
        self.observe(Ok(ExtensionInvocation::CallbackOnIteration(
            CallbackIterationInput {
                agent: agent.to_string(),
                iteration: WireU64::from_u64(u64::try_from(iteration).unwrap_or(u64::MAX)),
            },
        )))
    }
}

fn step_type_wire(step: &echo_agent::agent::StepType) -> std::result::Result<StepTypeWire, String> {
    Ok(match step {
        echo_agent::agent::StepType::Thought(text) => StepTypeWire::Thought { text: text.clone() },
        echo_agent::agent::StepType::Call {
            tool_call_id,
            function_name,
            arguments,
        } => StepTypeWire::Call {
            tool_call_id: tool_call_id.clone(),
            function_name: function_name.clone(),
            arguments: to_wire(arguments)?,
        },
    })
}

// ── InterventionCallback proxy ──────────────────────────────────────────────

fn intervention_result_from_wire(
    wire: InterventionResultWire,
) -> std::result::Result<echo_agent::agent::InterventionResult, String> {
    Ok(echo_agent::agent::InterventionResult {
        block: wire.block,
        block_reason: wire.block_reason,
        injected_context: wire.injected_context,
        redirect_to: wire.redirect_to,
        cancel: wire.cancel,
        modified_args: wire
            .modified_args
            .map(|value| value.into_json().map_err(|error| error.to_string()))
            .transpose()?,
    })
}

/// Behavior-controlling intervention proxy. It is its own extension kind —
/// never an alias of the observational AgentCallback. Decode, transport and
/// deadline failures return `cancel` so a missing policy decision cannot
/// silently authorize a behavior-changing action.
pub(crate) struct ExtensionInterventionProxy {
    bridge: Arc<ExtensionBridge>,
    extension: WireHandle,
    session_id: String,
}

impl ExtensionInterventionProxy {
    pub(crate) fn new(
        bridge: Arc<ExtensionBridge>,
        extension: WireHandle,
        session_id: String,
    ) -> Option<Self> {
        let record = bridge.state().ok()?.handles.extension(&extension).ok()?;
        matches!(
            record.descriptor,
            ExtensionDescriptor::InterventionCallback { .. }
        )
        .then(|| Self {
            bridge,
            extension,
            session_id,
        })
    }

    fn decide(
        &self,
        invocation: std::result::Result<ExtensionInvocation, String>,
    ) -> futures::future::BoxFuture<'_, echo_agent::agent::InterventionResult> {
        let bridge = self.bridge.clone();
        let extension = self.extension.clone();
        let context = session_invocation_context(&self.session_id);
        Box::pin(async move {
            let Ok(invocation) = invocation else {
                return echo_agent::agent::InterventionResult::cancel();
            };
            match bridge
                .invoke_once(
                    &extension,
                    Some(context),
                    invocation,
                    bridge.connection_cancellation(),
                )
                .await
            {
                Ok(ExtensionResult::InterventionOnToolCall(value))
                | Ok(ExtensionResult::InterventionOnThinkStart(value))
                | Ok(ExtensionResult::InterventionOnFinalAnswer(value)) => {
                    intervention_result_from_wire(value).unwrap_or_else(|error| {
                        tracing::warn!(
                            "intervention extension {} returned an invalid result: {}",
                            extension.id,
                            error
                        );
                        echo_agent::agent::InterventionResult::cancel()
                    })
                }
                Ok(_) => echo_agent::agent::InterventionResult::cancel(),
                Err(error) => {
                    tracing::warn!(
                        "intervention extension {} failed: {}",
                        extension.id,
                        error.message
                    );
                    echo_agent::agent::InterventionResult::cancel()
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
        self.decide(to_wire(args).map(|args| {
            ExtensionInvocation::InterventionOnToolCall(CallbackToolStartInput {
                agent: agent.to_string(),
                tool: tool.to_string(),
                args,
            })
        }))
    }

    fn on_think_start<'a>(
        &'a self,
        agent: &'a str,
        messages: &'a [echo_agent::llm::types::Message],
    ) -> futures::future::BoxFuture<'a, echo_agent::agent::InterventionResult> {
        self.decide(
            messages
                .iter()
                .map(message_wire)
                .collect::<std::result::Result<Vec<_>, _>>()
                .map(|messages| {
                    ExtensionInvocation::InterventionOnThinkStart(CallbackThinkStartInput {
                        agent: agent.to_string(),
                        messages,
                    })
                }),
        )
    }

    fn on_final_answer<'a>(
        &'a self,
        agent: &'a str,
        answer: &'a str,
    ) -> futures::future::BoxFuture<'a, echo_agent::agent::InterventionResult> {
        self.decide(Ok(ExtensionInvocation::InterventionOnFinalAnswer(
            CallbackFinalAnswerInput {
                agent: agent.to_string(),
                answer: answer.to_string(),
            },
        )))
    }
}

// ── AgentFactory + custom Agent proxies ─────────────────────────────────────

/// Async subagent-factory adapter: the framework's lazy subagent
/// construction calls `create()`, which forwards to the same factory
/// operation with a minimal construction config.
pub(crate) struct SubagentFactoryAdapter {
    bridge: Arc<ExtensionBridge>,
    extension: WireHandle,
    subagent_name: String,
    session_id: String,
}

impl echo_agent::agent::subagent::AgentFactory for SubagentFactoryAdapter {
    fn create(
        &self,
    ) -> futures::future::BoxFuture<'static, echo_agent::error::Result<Box<dyn Agent>>> {
        let bridge = self.bridge.clone();
        let extension = self.extension.clone();
        let subagent_name = self.subagent_name.clone();
        let session_id = self.session_id.clone();
        Box::pin(async move {
            let payload = AgentFactoryConfigWire {
                model: String::new(),
                name: subagent_name,
                system_prompt: String::new(),
                tool_count: WireU64::from_u64(0),
            };
            let value = bridge
                .invoke_once(
                    &extension,
                    Some(session_invocation_context(&session_id)),
                    ExtensionInvocation::FactoryCreateAgent(payload),
                    bridge.connection_cancellation(),
                )
                .await
                .map_err(react_error)?;
            let ExtensionResult::FactoryCreateAgent(descriptor) = value else {
                return Err(ReactError::Other(
                    "agent factory returned the wrong result variant".to_string(),
                ));
            };
            let custom_extension = bridge
                .register_factory_instance(&extension, &descriptor)
                .map_err(react_error)?;
            let factory_lease = Arc::new(FactoryInstanceLease {
                bridge: bridge.clone(),
                extension: custom_extension.clone(),
                session_id: session_id.clone(),
                close_result: tokio::sync::OnceCell::new(),
            });
            Ok(Box::new(ExtensionCustomAgentProxy {
                bridge,
                extension: custom_extension,
                descriptor,
                session_id,
                factory_lease: Some(factory_lease),
            }) as Box<dyn Agent>)
        })
    }
}

struct FactoryInstanceLease {
    bridge: Arc<ExtensionBridge>,
    extension: WireHandle,
    session_id: String,
    close_result: tokio::sync::OnceCell<std::result::Result<(), String>>,
}

impl FactoryInstanceLease {
    async fn close(&self) -> echo_agent::error::Result<()> {
        self.close_result
            .get_or_init(|| async {
                close_factory_instance(
                    self.bridge.clone(),
                    self.extension.clone(),
                    self.session_id.clone(),
                )
                .await
                .map_err(|error| error.to_string())
            })
            .await
            .clone()
            .map_err(ReactError::Other)
    }
}

impl Drop for FactoryInstanceLease {
    fn drop(&mut self) {
        if self.close_result.get().is_some() {
            return;
        }
        let bridge = self.bridge.clone();
        let extension = self.extension.clone();
        let session_id = self.session_id.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                if let Err(error) = close_factory_instance(bridge, extension, session_id).await {
                    tracing::warn!("factory-created custom agent close failed: {error}");
                }
            });
        } else {
            self.bridge.release_extension(&self.extension);
        }
    }
}

async fn close_factory_instance(
    bridge: Arc<ExtensionBridge>,
    extension: WireHandle,
    session_id: String,
) -> echo_agent::error::Result<()> {
    let result = bridge
        .invoke_once_connection_scoped(
            &extension,
            Some(session_invocation_context(&session_id)),
            ExtensionInvocation::AgentClose(ExtensionUnit),
        )
        .await
        .map_err(react_error)
        .and_then(|result| {
            matches!(result, ExtensionResult::AgentClose(_))
                .then_some(())
                .ok_or_else(|| {
                    ReactError::Other(
                        "factory-created custom agent close returned the wrong result variant"
                            .to_string(),
                    )
                })
        });
    bridge.release_extension(&extension);
    result
}

/// Thin custom `Agent` proxy: execute/chat (and their streaming forms) run
/// through the bridge; events the SDK emits are plain `AgentEvent` values
/// the existing run observers already project — the proxy never fabricates
/// sequence numbers or terminals.
#[derive(Clone)]
pub(crate) struct ExtensionCustomAgentProxy {
    bridge: Arc<ExtensionBridge>,
    extension: WireHandle,
    descriptor: CustomAgentDescriptorWire,
    session_id: String,
    factory_lease: Option<Arc<FactoryInstanceLease>>,
}

impl ExtensionCustomAgentProxy {
    async fn run_once(
        &self,
        invocation: ExtensionInvocation,
    ) -> echo_agent::error::Result<ExtensionResult> {
        let value = self
            .bridge
            .invoke_once(
                &self.extension,
                Some(session_invocation_context(&self.session_id)),
                invocation,
                self.bridge.connection_cancellation(),
            )
            .await
            .map_err(react_error)?;
        Ok(value)
    }

    async fn run_stream(
        &self,
        invocation: ExtensionInvocation,
    ) -> echo_agent::error::Result<
        futures::stream::BoxStream<'static, echo_agent::error::Result<AgentEvent>>,
    > {
        let (receiver, stream, invocation_id, cancellation, lease, sink) = self
            .bridge
            .invoke_stream(
                &self.extension,
                Some(session_invocation_context(&self.session_id)),
                invocation,
                self.bridge.connection_cancellation(),
            )
            .await
            .map_err(react_error)?;
        Ok(futures::StreamExt::map(
            extension_event_stream(
                self.bridge.clone(),
                stream,
                invocation_id,
                cancellation,
                lease,
                sink,
                receiver,
            ),
            |event: ExtensionStreamEvent| {
                let result: echo_agent::error::Result<AgentEvent> = match event {
                    ExtensionStreamEvent::Chunk { value, .. } => {
                        let ExtensionStreamChunkValue::Agent(value) = value else {
                            return Err(ReactError::Other(
                                "custom agent stream received a non-agent payload".to_string(),
                            ));
                        };
                        agent_stream_chunk_from_wire(value).map_err(ReactError::Other)
                    }
                    ExtensionStreamEvent::Complete { value, .. } => {
                        let ExtensionStreamCompleteValue::Agent(value) = value else {
                            return Err(ReactError::Other(
                                "custom agent stream received a non-agent terminal".to_string(),
                            ));
                        };
                        agent_stream_terminal_from_wire(value).map_err(ReactError::Other)
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

    async fn finish_factory_result<T>(
        &self,
        result: echo_agent::error::Result<T>,
    ) -> echo_agent::error::Result<T> {
        let Some(lease) = &self.factory_lease else {
            return result;
        };
        let close = lease.close().await;
        match (result, close) {
            (Err(error), _) => Err(error),
            (Ok(value), Ok(())) => Ok(value),
            (Ok(_), Err(error)) => Err(error),
        }
    }
}

fn close_factory_stream(
    stream: futures::stream::BoxStream<'static, echo_agent::error::Result<AgentEvent>>,
    lease: Arc<FactoryInstanceLease>,
) -> futures::stream::BoxStream<'static, echo_agent::error::Result<AgentEvent>> {
    futures::stream::unfold(
        (stream, Some(lease)),
        |(mut stream, mut lease)| async move {
            match stream.next().await {
                Some(item) => {
                    let terminal = item.as_ref().is_err()
                        || matches!(
                            item.as_ref(),
                            Ok(AgentEvent::FinalAnswer(_)
                                | AgentEvent::Error { .. }
                                | AgentEvent::Cancelled)
                        );
                    if terminal
                        && let Some(owner) = lease.take()
                        && let Err(close_error) = owner.close().await
                    {
                        return Some((Err(close_error), (stream, lease)));
                    }
                    Some((item, (stream, lease)))
                }
                None => {
                    if let Some(owner) = lease.take()
                        && let Err(close_error) = owner.close().await
                    {
                        return Some((Err(close_error), (stream, lease)));
                    }
                    None
                }
            }
        },
    )
    .boxed()
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
            if let Some(lease) = &self.factory_lease {
                return lease.close().await;
            }
            let result = self
                .run_once(ExtensionInvocation::AgentClose(ExtensionUnit))
                .await
                .and_then(|result| {
                    matches!(result, ExtensionResult::AgentClose(_))
                        .then_some(())
                        .ok_or_else(|| {
                            ReactError::Other(
                                "custom agent close returned the wrong result variant".to_string(),
                            )
                        })
                });
            self.bridge.release_extension(&self.extension);
            result
        })
    }

    fn execute<'a>(
        &'a self,
        task: &'a str,
    ) -> futures::future::BoxFuture<'a, echo_agent::error::Result<String>> {
        Box::pin(async move {
            let result = match self
                .run_once(ExtensionInvocation::AgentExecute(AgentTaskInput {
                    task: task.to_string(),
                }))
                .await
            {
                Ok(ExtensionResult::AgentExecute(output)) => Ok(output),
                Ok(_) => Err(ReactError::Other(
                    "custom agent execute returned the wrong result variant".to_string(),
                )),
                Err(error) => Err(error),
            };
            self.finish_factory_result(result).await
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
            let stream = match self
                .run_stream(ExtensionInvocation::AgentExecuteStream(AgentTaskInput {
                    task: task.to_string(),
                }))
                .await
            {
                Ok(stream) => stream,
                Err(error) => return self.finish_factory_result(Err(error)).await,
            };
            Ok(match &self.factory_lease {
                Some(lease) => close_factory_stream(stream, lease.clone()),
                None => stream,
            })
        })
    }

    fn chat<'a>(
        &'a self,
        message: &'a str,
    ) -> futures::future::BoxFuture<'a, echo_agent::error::Result<String>> {
        Box::pin(async move {
            let result = match self
                .run_once(ExtensionInvocation::AgentChat(AgentMessageInput {
                    message: message.to_string(),
                }))
                .await
            {
                Ok(ExtensionResult::AgentChat(output)) => Ok(output),
                Ok(_) => Err(ReactError::Other(
                    "custom agent chat returned the wrong result variant".to_string(),
                )),
                Err(error) => Err(error),
            };
            self.finish_factory_result(result).await
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
            let stream = match self
                .run_stream(ExtensionInvocation::AgentChatStream(AgentMessageInput {
                    message: message.to_string(),
                }))
                .await
            {
                Ok(stream) => stream,
                Err(error) => return self.finish_factory_result(Err(error)).await,
            };
            Ok(match &self.factory_lease {
                Some(lease) => close_factory_stream(stream, lease.clone()),
                None => stream,
            })
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
    session_id: &str,
) -> std::result::Result<(), String> {
    let state = bridge.state().map_err(|error| error.message)?;
    // LlmClient: the most recently registered implementation becomes the
    // Session's model client.
    if let Some((extension, _)) = state
        .handles
        .extensions_of_kind(ExtensionKind::LlmClient)
        .into_iter()
        .max_by_key(|(_, record)| record.registration_order)
        && let Some(proxy) =
            ExtensionLlmClientProxy::new(bridge.clone(), extension, session_id.to_string())
    {
        agent.set_llm_client(Arc::new(proxy));
    }
    // Tools.
    for (extension, _) in state.handles.extensions_of_kind(ExtensionKind::Tool) {
        if let Some(proxy) =
            ExtensionToolProxy::new(bridge.clone(), extension, session_id.to_string())
        {
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
        && let Some(proxy) =
            ExtensionStoreProxy::new(bridge.clone(), extension, session_id.to_string())
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
        && let Some(proxy) =
            ExtensionHumanLoopProxy::new(bridge.clone(), extension, session_id.to_string())
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
            let events = match &record.descriptor {
                ExtensionDescriptor::Hook { events, .. } => events
                    .iter()
                    .filter_map(|event| echo_agent::hooks::HookEvent::from_name(event))
                    .collect::<Vec<_>>(),
                _ => Vec::new(),
            };
            registry.set_programmatic_hook(&record.implementation_id, &events, executor);
        }
    }
    // Observational callbacks.
    for (extension, _) in state
        .handles
        .extensions_of_kind(ExtensionKind::AgentCallback)
    {
        if let Some(proxy) =
            ExtensionAgentCallbackProxy::new(bridge.clone(), extension, session_id.to_string())
        {
            agent.add_callback(Arc::new(proxy));
        }
    }
    // Intervention callbacks.
    for (extension, _) in state
        .handles
        .extensions_of_kind(ExtensionKind::InterventionCallback)
    {
        if let Some(proxy) =
            ExtensionInterventionProxy::new(bridge.clone(), extension, session_id.to_string())
        {
            agent.add_intervention_callback(Arc::new(proxy));
        }
    }
    // Custom agents register for subagent dispatch by name.
    for (extension, _) in state
        .handles
        .session_extensions_of_kind(ExtensionKind::CustomAgent)
    {
        if let Some(proxy) = ExtensionCustomAgentProxy::for_registration(
            bridge.clone(),
            extension,
            session_id.to_string(),
        ) {
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
                session_id: session_id.to_string(),
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
        session_id: String,
    ) -> Option<Self> {
        let record = bridge.state().ok()?.handles.extension(&extension).ok()?;
        if record.factory_instance {
            return None;
        }
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
            session_id,
            factory_lease: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_stream_handle(id: &str) -> WireHandle {
        WireHandle {
            id: id.to_string(),
            generation: WireU64::from_u64(1),
            kind: HandleKind::Stream,
        }
    }

    fn test_sequence(
        value: u64,
    ) -> std::result::Result<echo_sdk_protocol::scalar::WireNonZeroU64, String> {
        echo_sdk_protocol::scalar::WireNonZeroU64::try_from(value.to_string())
            .map_err(|error| error.to_string())
    }

    fn tool_progress(
        stream: &WireHandle,
        sequence: u64,
    ) -> std::result::Result<ExtensionStreamEvent, String> {
        Ok(ExtensionStreamEvent::Chunk {
            stream: stream.clone(),
            sequence: test_sequence(sequence)?,
            value: ExtensionStreamChunkValue::Tool(ToolStreamChunkWire::Progress {
                message: format!("chunk {sequence}"),
                percent: None,
            }),
        })
    }

    #[test]
    fn chat_chunk_wire_round_trips_fields() -> Result<(), String> {
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
        let wire = chat_chunk_wire(&chunk)?;
        let restored = chat_chunk_from_wire(wire)?;
        assert_eq!(restored.delta.content.as_deref(), Some("hello"));
        assert_eq!(restored.finish_reason.as_deref(), Some("stop"));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn tool_context_keeps_non_utf8_working_directory_lossless() -> Result<(), String> {
        use std::os::unix::ffi::OsStringExt as _;

        let path =
            std::path::PathBuf::from(std::ffi::OsString::from_vec(b"/tmp/bridge-\xff".to_vec()));
        let context = ToolContext {
            working_dir: Some(path.clone()),
            ..ToolContext::default()
        };
        let wire = tool_context_wire(&context)?;
        let Some(WirePath::Unix { bytes_base64, .. }) = wire.working_dir else {
            return Err("unix paths must use the unix wire encoding".to_string());
        };
        let decoded = base64::engine::general_purpose::STANDARD_NO_PAD
            .decode(bytes_base64)
            .map_err(|error| error.to_string())?;
        use std::os::unix::ffi::OsStrExt as _;
        assert_eq!(decoded, path.as_os_str().as_bytes());
        Ok(())
    }

    #[test]
    fn approval_scope_maps_all_closed_variants() {
        assert_eq!(
            scope_from_wire(ApprovalScopeWire::Once),
            echo_agent::human_loop::ApprovalScope::Once
        );
        assert_eq!(
            scope_from_wire(ApprovalScopeWire::Session),
            echo_agent::human_loop::ApprovalScope::Session
        );
        assert_eq!(
            scope_from_wire(ApprovalScopeWire::SessionTool),
            echo_agent::human_loop::ApprovalScope::SessionTool
        );
    }

    #[tokio::test]
    async fn duplicate_stream_sequence_settles_a_failed_terminal() -> Result<(), String> {
        let shared = ExtensionBridgeShared::new();
        let stream = test_stream_handle("stream-duplicate");
        let (sink, mut receiver) = ExtensionStreamSink::new(
            stream.clone(),
            "extension-1".to_string(),
            ExtensionKind::Tool,
            STREAM_CHANNEL_CAPACITY,
        );
        shared.register_stream(sink);

        shared.deliver_stream_event(tool_progress(&stream, 1)?)?;
        assert!(
            shared
                .deliver_stream_event(tool_progress(&stream, 1)?)
                .is_err()
        );
        assert!(matches!(
            receiver.recv().await,
            Some(ExtensionStreamEvent::Chunk { .. })
        ));
        assert!(matches!(
            receiver.recv().await,
            Some(ExtensionStreamEvent::Failed { .. })
        ));
        assert!(shared.stream_sink(&stream.id).is_none());
        Ok(())
    }

    #[test]
    fn full_stream_mailbox_retains_a_failed_terminal_without_a_gap() -> Result<(), String> {
        let shared = ExtensionBridgeShared::new();
        let stream = test_stream_handle("stream-backpressure");
        let (sink, _receiver) = ExtensionStreamSink::new(
            stream.clone(),
            "extension-1".to_string(),
            ExtensionKind::Tool,
            STREAM_CHANNEL_CAPACITY,
        );
        shared.register_stream(sink.clone());

        for sequence in 1..=STREAM_CHANNEL_CAPACITY {
            let sequence = u64::try_from(sequence).map_err(|error| error.to_string())?;
            shared.deliver_stream_event(tool_progress(&stream, sequence)?)?;
        }
        let failed_sequence = u64::try_from(STREAM_CHANNEL_CAPACITY)
            .map_err(|error| error.to_string())?
            .saturating_add(1);
        assert!(
            shared
                .deliver_stream_event(tool_progress(&stream, failed_sequence)?)
                .is_err()
        );
        assert!(matches!(
            sink.take_terminal(),
            Some(ExtensionStreamEvent::Failed { sequence, .. })
                if sequence.to_u64() == Some(failed_sequence)
        ));
        assert!(shared.stream_sink(&stream.id).is_none());
        Ok(())
    }

    #[test]
    fn every_public_agent_event_variant_round_trips() -> Result<(), String> {
        let events = vec![
            AgentEvent::Token("token".to_string()),
            AgentEvent::ThinkStart,
            AgentEvent::ThinkEnd {
                prompt_tokens: 2,
                completion_tokens: 3,
            },
            AgentEvent::LlmUsage {
                model: "fixture-model".to_string(),
                prompt_tokens: 2,
                completion_tokens: 3,
                total_tokens: 5,
                cached_prompt_tokens: 1,
                cache_creation_prompt_tokens: 1,
                usage_reported: true,
            },
            AgentEvent::BudgetDecision {
                decision: echo_agent::agent::BudgetDecision::WindDown,
                reason: "near_limit".to_string(),
                iteration: 4,
                reported_model_tokens: 5,
                usage_complete: true,
            },
            AgentEvent::ToolCall {
                call_id: "call-1".to_string(),
                invocation: echo_agent::agent::ToolInvocation {
                    requested_name: "search".to_string(),
                    requested_args: serde_json::json!({"query": "bridge"}),
                    name: "search".to_string(),
                    args: serde_json::json!({"query": "bridge"}),
                    rewrites: Vec::new(),
                },
            },
            AgentEvent::ToolResult {
                call_id: "call-1".to_string(),
                name: "search".to_string(),
                result: ToolResult::success("found"),
            },
            AgentEvent::ToolStream {
                call_id: "call-1".to_string(),
                name: "search".to_string(),
                event: ToolStreamEvent::Progress {
                    message: "halfway".to_string(),
                    percent: Some(50),
                },
            },
            AgentEvent::ToolBatchStart { tool_count: 1 },
            AgentEvent::ToolBatchEnd,
            AgentEvent::GuardTriggered {
                guard: "fixture".to_string(),
                blocked: true,
            },
            AgentEvent::MemoryRecalled { count: 2 },
            AgentEvent::ContextCompressed {
                before_count: 4,
                after_count: 2,
                before_tokens: 40,
                after_tokens: 20,
            },
            AgentEvent::Chart {
                spec: serde_json::json!({"mark": "bar"}),
            },
            AgentEvent::Error {
                source: "fixture".to_string(),
                message: "failed".to_string(),
                failure: echo_agent::error::AgentFailure {
                    category: echo_agent::error::AgentFailureCategory::Other,
                    terminal_kind: echo_agent::error::AgentTerminalKind::Failed,
                    retryable: false,
                    code: "fixture".to_string(),
                    http_status: None,
                    message: "failed".to_string(),
                },
            },
            AgentEvent::SafetyNotice {
                action: "write".to_string(),
                reason: "fixture".to_string(),
                risk: "low".to_string(),
                permission: "default".to_string(),
            },
            AgentEvent::ParameterError {
                tool: "search".to_string(),
                parameter: "query".to_string(),
                expected: "string".to_string(),
                got: "number".to_string(),
            },
            AgentEvent::FinalAnswer("done".to_string()),
            AgentEvent::Cancelled,
        ];

        for event in events {
            let expected = serde_json::to_value(&event).map_err(|error| error.to_string())?;
            let wire = agent_event_wire_from_framework(&event)?;
            let restored = agent_event_from_wire(wire)?;
            let actual = serde_json::to_value(restored).map_err(|error| error.to_string())?;
            assert_eq!(actual, expected);
        }
        Ok(())
    }

    #[test]
    fn custom_agent_tool_stream_chunk_stays_non_terminal() -> Result<(), String> {
        let event = agent_stream_chunk_from_wire(AgentStreamChunkWire::ToolStream {
            call_id: "call-1".to_string(),
            name: "search".to_string(),
            event: ToolStreamChunkWire::Progress {
                message: "halfway".to_string(),
                percent: Some(50),
            },
        })?;
        assert!(matches!(
            event,
            AgentEvent::ToolStream {
                event: ToolStreamEvent::Progress {
                    percent: Some(50),
                    ..
                },
                ..
            }
        ));
        Ok(())
    }
}
