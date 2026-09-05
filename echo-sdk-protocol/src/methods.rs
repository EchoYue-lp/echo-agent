//! `_echo_agent/*` method catalog payloads.
//!
//! Only the echo-agent extension profile is defined here. Standard ACP
//! methods (`initialize`, `session/new`, `session/prompt`, ...) and the
//! JSON-RPC envelope itself are owned by the official schema crate and are
//! never re-declared (design §10.1). Every custom method starts with an
//! underscore as ACP extensibility requires, and the catalog in `catalog.rs`
//! asserts that each method is declared in the capability.
//!
//! Payload shapes deliberately keep request/response DTOs thin: they carry
//! handles, lossless scalars and the closed tagged `WireValue` algebra. They never
//! recompute framework semantics — ready-frontier decisions, terminal
//! states, retries and recovery belong to the Rust authority (design §10.4).

use serde::{Deserialize, Serialize};

use crate::error::EchoSdkError;
use crate::event::WireEventEnvelope;
use crate::handle::{HandleKind, WireHandle};
use crate::scalar::{WireDuration, WireNonZeroU64, WirePath, WireU64, WireValue};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunInputKind {
    Chat,
    Execute,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Pending,
    Running,
    Paused,
    Completed,
    Cancelled,
    Failed,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ControlAction {
    Pause,
    Resume,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum WireTaskStatus {
    Pending,
    Running,
    Blocked { reason: String },
    Completed,
    Failed { error: String },
    Skipped,
    Cancelled,
    TimedOut { error: String },
    Retrying { attempt: u32, last_error: String },
    Paused { reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionKind {
    Tool,
    LlmClient,
    Store,
    HumanLoopProvider,
    Hook,
    AgentCallback,
    AgentFactory,
}

// ── Agent lifecycle ─────────────────────────────────────────────────────────

/// `_echo_agent/agent/create` request. The full construction grammar of the
/// Rust builder is projected as a typed, versioned config value; the Host
/// remains the validation authority.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentCreateRequest {
    /// Framework construction config represented by manifest-identified values.
    pub config: WireValue,
    /// Client-assigned idempotency identity; independent from JSON-RPC ids.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 256))]
    pub idempotency_id: Option<String>,
}

/// `_echo_agent/agent/create` response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentCreateResponse {
    pub agent: WireHandle,
}

/// `_echo_agent/agent/describe` request/response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentDescribeRequest {
    pub agent: WireHandle,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentDescribeResponse {
    /// Immutable construction facts and capability snapshot of the agent.
    pub snapshot: WireValue,
}

/// `_echo_agent/agent/close` request. In-flight runs settle per the
/// framework's own cancellation semantics; closing never fabricates
/// terminals.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentCloseRequest {
    pub agent: WireHandle,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentCloseResponse {
    /// True when this call released the agent; false when it was already
    /// closed (idempotent close).
    pub released: bool,
}

// ── Session handles ─────────────────────────────────────────────────────────

/// `_echo_agent/session/create` request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SessionCreateRequest {
    pub agent: WireHandle,
    /// Session configuration represented by manifest-identified values.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<WireValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotency_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SessionCreateResponse {
    pub session: WireHandle,
}

/// `_echo_agent/session/load` request: resume a persisted session by
/// framework identity. Only stores that support persistence can serve it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SessionLoadRequest {
    pub agent: WireHandle,
    #[schemars(length(min = 1, max = 256))]
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SessionLoadResponse {
    pub session: WireHandle,
    /// Sequence watermark the session recovered to, when replayable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovered_sequence: Option<WireU64>,
}

/// `_echo_agent/session/close` request/response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SessionCloseRequest {
    pub session: WireHandle,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SessionCloseResponse {
    pub released: bool,
}

// ── Runs ────────────────────────────────────────────────────────────────────

/// Input of one run: chat prompt parts or execute directive.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RunInput {
    /// `chat` or `execute`, mirroring the facade's unified turn driver.
    pub kind: RunInputKind,
    /// Facade request payload (TurnRequest/ChatRequest shape).
    pub payload: WireValue,
}

/// `_echo_agent/run/start` request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RunStartRequest {
    pub session: WireHandle,
    pub input: RunInput,
    /// Optional structured-output contract; validated by the framework.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub structured_output: Option<WireValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 256))]
    pub idempotency_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RunStartResponse {
    pub run: WireHandle,
    /// First accepted event of the run, if already available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_event: Option<WireEventEnvelope>,
}

/// `_echo_agent/run/get` request/response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RunGetRequest {
    pub run: WireHandle,
}

/// Run state snapshot: status, the single authoritative terminal (when
/// settled) and the receipt facts. Never synthesizes a terminal that the
/// framework has not emitted (exactly-one-terminal, design §11.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RunGetResponse {
    pub status: RunStatus,
    /// Last sequence the snapshot covers.
    pub last_sequence: WireU64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal: Option<WireValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt: Option<WireValue>,
}

/// `_echo_agent/run/wait` request: bounded wait for the terminal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RunWaitRequest {
    pub run: WireHandle,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<WireDuration>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RunWaitResponse {
    pub settled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal: Option<WireValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt: Option<WireValue>,
}

/// `_echo_agent/run/cancel` request. Competing with natural completion, the
/// framework's own CAS/terminal semantics decide the unique outcome; the
/// transport never writes a second terminal (design §14.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RunCancelRequest {
    pub run: WireHandle,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RunCancelResponse {
    /// Whether this call initiated cancellation.
    pub cancellation_initiated: bool,
    /// Status at the time of the call; final state still arrives as events.
    pub status: RunStatus,
}

/// `_echo_agent/run/steer` request: mid-flight steering for chats that
/// support it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RunSteerRequest {
    pub run: WireHandle,
    pub payload: WireValue,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RunSteerResponse {
    pub accepted: bool,
}

// ── Task graph (TaskRun / PlanTask) ────────────────────────────────────────

/// `_echo_agent/task/create` request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TaskCreateRequest {
    pub task_run: WireHandle,
    /// TaskSpec payload.
    pub spec: WireValue,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TaskCreateResponse {
    pub task: WireHandle,
    pub revision: WireU64,
}

/// `_echo_agent/task/update` request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TaskUpdateRequest {
    pub task: WireHandle,
    /// Expected revision for optimistic concurrency; the framework rejects
    /// stale writers.
    pub expected_revision: WireU64,
    pub patch: WireValue,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TaskUpdateResponse {
    pub revision: WireU64,
}

/// `_echo_agent/task/list` request/response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TaskListRequest {
    pub task_run: WireHandle,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TaskListResponse {
    pub tasks: Vec<TaskSummary>,
}

/// Projection of one task's identity and state; authoritative state remains
/// in the framework store.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TaskSummary {
    pub task: WireHandle,
    pub status: WireTaskStatus,
    pub revision: WireU64,
}

/// `_echo_agent/task/execute` request: drive one PlanTask through the
/// framework executor (single authority for scheduling/retry/cancel).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TaskExecuteRequest {
    pub task: WireHandle,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 256))]
    pub idempotency_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TaskExecuteResponse {
    pub run: WireHandle,
}

/// `_echo_agent/task/control` request: pause/resume/cancel routed to the
/// framework service.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TaskControlRequest {
    pub task: WireHandle,
    /// One of the framework control verbs (`pause`, `resume`, `cancel`).
    pub action: ControlAction,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TaskControlResponse {
    pub accepted: bool,
    pub status: WireTaskStatus,
}

// ── Subagents ───────────────────────────────────────────────────────────────

/// `_echo_agent/subagent/dispatch` request. The framework executor remains
/// the only scheduler.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SubagentDispatchRequest {
    pub session: WireHandle,
    pub request: WireValue,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 256))]
    pub idempotency_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SubagentDispatchResponse {
    pub subagent: WireHandle,
}

/// `_echo_agent/subagent/await` request/response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SubagentAwaitRequest {
    pub subagent: WireHandle,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<WireDuration>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SubagentAwaitResponse {
    pub settled: bool,
    /// SubagentResult payload.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<WireValue>,
}

/// `_echo_agent/subagent/control` request/response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SubagentControlRequest {
    pub subagent: WireHandle,
    pub action: ControlAction,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SubagentControlResponse {
    pub accepted: bool,
}

// ── Extension bridge ────────────────────────────────────────────────────────

/// `_echo_agent/extension/register` request: register a host-language
/// implementation of a public framework trait (Tool, LlmClient, Store,
/// HumanLoopProvider, Hook, AgentFactory, ...).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ExtensionRegisterRequest {
    /// Which extension point is implemented.
    pub kind: ExtensionKind,
    /// Client-side implementation identity (non-empty).
    #[schemars(length(min = 1, max = 256))]
    pub implementation_id: String,
    /// Descriptor the Host uses for dispatch: for Tools this covers name,
    /// description, JSON Schema parameters, revision and modality.
    pub descriptor: WireValue,
    /// Declared concurrency/timeout contract.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<WireDuration>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ExtensionRegisterResponse {
    pub extension: WireHandle,
}

/// `_echo_agent/extension/unregister` request/response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ExtensionUnregisterRequest {
    pub extension: WireHandle,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ExtensionUnregisterResponse {
    pub released: bool,
}

/// `_echo_agent/extension/invoke` reverse request (Host -> SDK): invoke a
/// registered implementation. The SDK dispatcher runs the host-language code
/// and replies with exactly one of `result`/`stream`/`error`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ExtensionInvokeCall {
    pub extension: WireHandle,
    /// Invocation identity; unique per call, used for cancellation.
    #[schemars(length(min = 1, max = 256))]
    pub invocation_id: String,
    /// Typed invocation payload (tool input, chat request, store op, ...).
    pub input: WireValue,
    /// Deadline for this invocation.
    pub deadline: WireDuration,
}

/// One callback outcome. Failures use the typed extension errors; there is
/// no implicit fallback to a built-in implementation (design §12.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ExtensionInvokeOutcome {
    Result { value: WireValue },
    Stream { stream: WireHandle },
    Error { error: EchoSdkError },
}

impl ExtensionInvokeOutcome {
    pub fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::Result { value } => value.validate().map_err(|_| "invalid callback result"),
            Self::Stream { stream } => {
                stream.validate()?;
                if stream.kind != HandleKind::Stream {
                    return Err("stream outcome requires a stream handle");
                }
                Ok(())
            }
            Self::Error { error } => error.validate(),
        }
    }
}

/// `_echo_agent/extension/cancel` reverse notification (Host -> SDK): the
/// framework cancelled an in-flight invocation; the SDK must stop work but
/// still answer the original call with a `cancelled` error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ExtensionCancelNotice {
    #[schemars(length(min = 1, max = 256))]
    pub invocation_id: String,
}

/// `_echo_agent/extension/stream` event. Sequence is monotonic per stream and
/// exactly one terminal variant may be emitted by the Host.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ExtensionStreamEvent {
    Chunk {
        stream: WireHandle,
        sequence: WireNonZeroU64,
        value: WireValue,
    },
    Complete {
        stream: WireHandle,
        sequence: WireNonZeroU64,
        value: WireValue,
    },
    Failed {
        stream: WireHandle,
        sequence: WireNonZeroU64,
        error: EchoSdkError,
    },
    Cancelled {
        stream: WireHandle,
        sequence: WireNonZeroU64,
    },
}

impl ExtensionStreamEvent {
    pub fn validate(&self) -> Result<(), &'static str> {
        let (stream, sequence) = match self {
            Self::Chunk {
                stream,
                sequence,
                value,
            }
            | Self::Complete {
                stream,
                sequence,
                value,
            } => {
                value.validate().map_err(|_| "invalid stream value")?;
                (stream, sequence)
            }
            Self::Failed {
                stream,
                sequence,
                error,
            } => {
                error.validate()?;
                (stream, sequence)
            }
            Self::Cancelled { stream, sequence } => (stream, sequence),
        };
        stream.validate()?;
        if stream.kind != HandleKind::Stream {
            return Err("stream event requires a stream handle");
        }
        if sequence.to_u64().is_none_or(|sequence| sequence == 0) {
            return Err("stream sequence must start at one");
        }
        Ok(())
    }
}

// ── Feature surfaces (memory / mcp / a2a / workflow / ...) ─────────────────

/// Manifest-identified facade operation using the closed `WireValue` algebra.
/// The operation must match an exact parity-manifest identity and signature.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct FeatureOperationRequest {
    #[schemars(length(min = 1, max = 1024))]
    pub operation: String,
    #[schemars(regex(pattern = "^sha256:[0-9a-fA-F]{64}$"))]
    pub signature_digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handle: Option<WireHandle>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub arguments: Vec<WireValue>,
}

impl FeatureOperationRequest {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.operation.trim().is_empty() {
            return Err("facade operation must be non-empty");
        }
        let digest_is_valid = self
            .signature_digest
            .strip_prefix("sha256:")
            .is_some_and(|hex| {
                hex.chars().count() == 64
                    && hex.chars().all(|character| character.is_ascii_hexdigit())
            });
        if !digest_is_valid {
            return Err("facade signature digest must be sha256");
        }
        if let Some(handle) = &self.handle {
            handle.validate()?;
        }
        for argument in &self.arguments {
            argument.validate().map_err(|_| "invalid facade argument")?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct FeatureOperationResponse {
    pub value: WireValue,
}

/// Working directory declaration a client may pass for run/session methods
/// that accept one; lossless via `WirePath`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkingDirectory {
    pub path: WirePath,
}

// ── Notifications re-exported for the catalog ───────────────────────────────

pub use crate::event::{EventCursor, EventNotification, GapNotification};

/// Handle kind check helper used by tests to keep DTOs aligned with the
/// handle taxonomy.
pub fn handle_kind_of(handle: &WireHandle) -> HandleKind {
    handle.kind
}
