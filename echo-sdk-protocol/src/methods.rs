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
//! handles, lossless scalars and verbatim framework values. They never
//! recompute framework semantics — ready-frontier decisions, terminal
//! states, retries and recovery belong to the Rust authority (design §10.4).

use serde::{Deserialize, Serialize};

use crate::error::EchoSdkError;
use crate::event::WireEventEnvelope;
use crate::handle::{HandleKind, WireHandle};
use crate::scalar::{WireDuration, WirePath, WireU64};

// ── Agent lifecycle ─────────────────────────────────────────────────────────

/// `_echo_agent/agent/create` request. The full construction grammar of the
/// Rust builder is projected as a typed, versioned config value; the Host
/// remains the validation authority.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentCreateRequest {
    /// Framework agent construction config, verbatim (schema owned by the
    /// facade DTOs the adapter plan will map).
    pub config: serde_json::Value,
    /// Client-assigned idempotency identity; independent from JSON-RPC ids.
    #[serde(skip_serializing_if = "Option::is_none")]
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
    pub snapshot: serde_json::Value,
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
    /// Session configuration, verbatim.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<serde_json::Value>,
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

/// Input of one run: chat prompt parts or execute directive, carried
/// verbatim from the facade request types.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RunInput {
    /// `chat` or `execute`, mirroring the facade's unified turn driver.
    pub kind: String,
    /// Facade request payload, verbatim (TurnRequest/ChatRequest shape).
    pub payload: serde_json::Value,
}

/// `_echo_agent/run/start` request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RunStartRequest {
    pub session: WireHandle,
    pub input: RunInput,
    /// Optional structured-output contract; validated by the framework.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub structured_output: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
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
    pub status: String,
    /// Last sequence the snapshot covers.
    pub last_sequence: WireU64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt: Option<serde_json::Value>,
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
    pub terminal: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt: Option<serde_json::Value>,
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
    pub status: String,
}

/// `_echo_agent/run/steer` request: mid-flight steering for chats that
/// support it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RunSteerRequest {
    pub run: WireHandle,
    pub payload: serde_json::Value,
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
    /// TaskSpec payload, verbatim.
    pub spec: serde_json::Value,
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
    pub patch: serde_json::Value,
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
    pub status: String,
    pub revision: WireU64,
}

/// `_echo_agent/task/execute` request: drive one PlanTask through the
/// framework executor (single authority for scheduling/retry/cancel).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TaskExecuteRequest {
    pub task: WireHandle,
    #[serde(skip_serializing_if = "Option::is_none")]
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
    pub action: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TaskControlResponse {
    pub accepted: bool,
    pub status: String,
}

// ── Subagents ───────────────────────────────────────────────────────────────

/// `_echo_agent/subagent/dispatch` request. DispatchRequest facts travel
/// verbatim; the framework executor remains the only scheduler.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SubagentDispatchRequest {
    pub session: WireHandle,
    pub request: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
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
    /// SubagentResult payload, verbatim.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
}

/// `_echo_agent/subagent/control` request/response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SubagentControlRequest {
    pub subagent: WireHandle,
    pub action: String,
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
    pub kind: String,
    /// Client-side implementation identity (non-empty).
    pub implementation_id: String,
    /// Descriptor the Host uses for dispatch: for Tools this covers name,
    /// description, JSON Schema parameters, revision and modality.
    pub descriptor: serde_json::Value,
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
    pub invocation_id: String,
    /// Typed invocation payload (tool input, chat request, store op, ...).
    pub input: serde_json::Value,
    /// Deadline for this invocation.
    pub deadline: WireDuration,
}

/// One callback outcome. Failures use the typed extension errors; there is
/// no implicit fallback to a built-in implementation (design §12.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ExtensionInvokeOutcome {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    /// Terminal failure of the invocation, typed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<EchoSdkError>,
}

/// `_echo_agent/extension/cancel` reverse notification (Host -> SDK): the
/// framework cancelled an in-flight invocation; the SDK must stop work but
/// still answer the original call with a `cancelled` error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ExtensionCancelNotice {
    pub invocation_id: String,
}

// ── Feature surfaces (memory / mcp / a2a / workflow / ...) ─────────────────

/// Generic verbatim operation envelope for feature-surface methods whose
/// full DTO mapping lands with the adapter plans: the method family is
/// frozen here, payloads carry the facade value under `opaque` until the
/// typed DTO plan replaces them (no second authority is introduced).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct FeatureOperationRequest {
    pub op: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handle: Option<WireHandle>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct FeatureOperationResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
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
