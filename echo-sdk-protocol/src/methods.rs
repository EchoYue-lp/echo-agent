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

use agent_client_protocol::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};use serde::{Deserialize, Serialize};

use crate::error::EchoSdkError;
use crate::event::WireEventEnvelope;
use crate::handle::{HandleKind, WireHandle};
use crate::scalar::{WireDuration, WireNonZeroU64, WirePath, WireU64, WireValue};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Pending,
    Running,
    Paused,
    Completed,
    Cancelled,
    Failed,
    /// The Host process restarted while the run was still active. Interrupted
    /// runs never gain a terminal or receipt: `run/get` reports the status,
    /// `run/wait` fails with typed `host_exited`, and new work continues from
    /// the framework's committed checkpoint only.
    Interrupted,
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
    InterventionCallback,
    AgentFactory,
    CustomAgent,
}

impl ExtensionKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ExtensionKind::Tool => "tool",
            ExtensionKind::LlmClient => "llm_client",
            ExtensionKind::Store => "store",
            ExtensionKind::HumanLoopProvider => "human_loop_provider",
            ExtensionKind::Hook => "hook",
            ExtensionKind::AgentCallback => "agent_callback",
            ExtensionKind::InterventionCallback => "intervention_callback",
            ExtensionKind::AgentFactory => "agent_factory",
            ExtensionKind::CustomAgent => "custom_agent",
        }
    }
}

// ── Agent lifecycle ─────────────────────────────────────────────────────────

/// `_echo_agent/agent/create` request. The construction grammar is a
/// versioned typed config, not a free-form value: `host_default` binds the
/// Host's own startup definition; `explicit` carries a strict projection of
/// the framework config. Unsupported builder capabilities (tool callbacks,
/// custom stores, human-in-loop, structured-output contracts) are absent
/// from this versioned surface on purpose — sending unknown fields fails
/// closed as invalid params instead of being silently ignored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonRpcRequest, schemars::JsonSchema)]
#[request(method = "_echo_agent/agent/create", response = AgentCreateResponse)]
#[serde(deny_unknown_fields)]
pub struct AgentCreateRequest {
    pub config: AgentConfigWire,
    /// Client-assigned idempotency identity; independent from JSON-RPC ids.
    /// The same id plus the same canonical config returns the same handle;
    /// the same id with a different config is a typed conflict.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 256))]
    pub idempotency_id: Option<String>,
}

impl AgentCreateRequest {
    pub fn validate(&self) -> Result<(), &'static str> {
        self.config.validate()?;
        if self
            .idempotency_id
            .as_ref()
            .is_some_and(|id| id.trim().is_empty() || id.chars().count() > 256)
        {
            return Err("idempotency_id must be non-empty and bounded");
        }
        Ok(())
    }
}

/// Versioned Agent construction config (design §10.4 agent family).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "variant", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentConfigWire {
    /// Bind the Host default Agent definition. Credential handling stays
    /// Host-local, so SDK Clients never transmit secrets for this branch.
    HostDefault,
    /// Explicit wire projection of the framework construction config.
    Explicit(Box<AgentConfigExplicitWire>),
}

impl AgentConfigWire {
    pub fn validate(&self) -> Result<(), &'static str> {
        let AgentConfigWire::Explicit(config) = self else {
            return Ok(());
        };
        if config.config_version != 1 {
            return Err("unsupported agent config_version");
        }
        if config.model.provider.trim().is_empty()
            || config.model.provider.chars().count() > 128
            || config.model.name.trim().is_empty()
            || config.model.name.chars().count() > 256
            || config.model.base_url.trim().is_empty()
            || config.model.base_url.chars().count() > 2048
        {
            return Err("model fields are empty or exceed their bounds");
        }
        if config.agent.name.trim().is_empty()
            || config.agent.name.chars().count() > 256
            || config.agent.system_prompt.trim().is_empty()
            || config.agent.system_prompt.chars().count() > 65_536
            || config.agent.max_iterations == 0
        {
            return Err("agent fields are empty or exceed their bounds");
        }
        if let Some(credential) = &config.model.credential {
            match credential {
                CredentialSourceWire::Inline { token }
                    if token.is_empty() || token.chars().count() > 4096 =>
                {
                    return Err("inline credential is empty or exceeds its bound");
                }
                CredentialSourceWire::Env { variable }
                    if variable.trim().is_empty() || variable.chars().count() > 256 =>
                {
                    return Err("credential environment variable is empty or exceeds its bound");
                }
                _ => {}
            }
        }
        Ok(())
    }
}

/// Explicit Agent construction payload. `config_version` gates evolution:
/// Hosts reject unknown versions with `invalid_config` instead of guessing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentConfigExplicitWire {
    pub config_version: u32,
    pub model: ModelConfigWire,
    pub agent: AgentSettingsWire,
}

/// Wire projection of the model construction settings. Exactly one
/// credential source may be provided — the tagged `credential` field makes
/// inline-token and environment sourcing mutually exclusive by grammar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelConfigWire {
    #[schemars(length(min = 1, max = 128))]
    pub provider: String,
    #[schemars(length(min = 1, max = 256))]
    pub name: String,
    /// Absolute HTTP(S) endpoint of the model API.
    #[schemars(length(min = 1, max = 2048))]
    pub base_url: String,
    pub api_protocol: LlmApiProtocolWire,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential: Option<CredentialSourceWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// Temperature is serialized as a bounded string decimal to avoid
    /// float ambiguity across languages.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<WireU64>,
}

/// API protocols the core profile can construct. Unlisted protocols fail
/// closed; new protocols extend this enum in a later contract version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LlmApiProtocolWire {
    ChatCompletions,
    Responses,
    Anthropic,
}

/// Mutually exclusive credential sourcing (design §10.4). Environment
/// sourcing keeps secrets out of the wire entirely.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "source", rename_all = "snake_case", deny_unknown_fields)]
pub enum CredentialSourceWire {
    /// Literal token passed inline. Only for local, single-user machines.
    Inline {
        #[schemars(length(min = 1, max = 4096))]
        token: String,
    },
    /// Name of the environment variable holding the token.
    Env {
        #[schemars(length(min = 1, max = 256))]
        variable: String,
    },
}

/// Agent behavior settings projected on the wire. Deliberately minimal:
/// every unsupported knob (memory, human-in-loop, compressor strategy,
/// subagent timeouts, tool toggles) is rejected by `deny_unknown_fields`
/// rather than silently ignored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentSettingsWire {
    #[schemars(length(min = 1, max = 256))]
    pub name: String,
    #[schemars(length(min = 1, max = 65536))]
    pub system_prompt: String,
    #[schemars(range(min = 1))]
    pub max_iterations: u32,
}

/// `_echo_agent/agent/create` response.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, JsonRpcResponse,
)]
pub struct AgentCreateResponse {
    pub agent: WireHandle,
}

/// `_echo_agent/agent/describe` request/response.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, JsonRpcRequest,
)]
#[request(method = "_echo_agent/agent/describe", response = AgentDescribeResponse)]
#[serde(deny_unknown_fields)]
pub struct AgentDescribeRequest {
    pub agent: WireHandle,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, JsonRpcResponse,
)]
#[serde(deny_unknown_fields)]
pub struct AgentDescribeResponse {
    /// Immutable construction facts and capability snapshot of the agent.
    pub snapshot: AgentSnapshotWire,
}

/// Typed capability snapshot of one Agent handle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentSnapshotWire {
    #[schemars(length(min = 1, max = 256))]
    pub name: String,
    #[schemars(length(min = 1, max = 256))]
    pub model_name: String,
    pub system_prompt: String,
    pub tool_names: Vec<String>,
    pub skill_names: Vec<String>,
    pub mcp_server_names: Vec<String>,
    /// Absolute working directory bound to new Sessions of this agent, when
    /// the definition carries one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
    /// Whether the definition came from the Host default configuration.
    pub host_default: bool,
}

/// `_echo_agent/agent/close` request. In-flight runs settle per the
/// framework's own cancellation semantics; closing never fabricates
/// terminals.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, JsonRpcRequest,
)]
#[request(method = "_echo_agent/agent/close", response = AgentCloseResponse)]
#[serde(deny_unknown_fields)]
pub struct AgentCloseRequest {
    pub agent: WireHandle,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, JsonRpcResponse,
)]
pub struct AgentCloseResponse {
    /// True when this call released the agent; false when it was already
    /// closed (idempotent close).
    pub released: bool,
}

// ── Session handles ─────────────────────────────────────────────────────────

/// `_echo_agent/session/create` request.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, JsonRpcRequest,
)]
#[request(method = "_echo_agent/session/create", response = SessionCreateResponse)]
#[serde(deny_unknown_fields)]
pub struct SessionCreateRequest {
    pub agent: WireHandle,
    /// Absolute primary working directory for the new Session.
    pub working_dir: Option<WirePath>,
    /// Stable framework identity to bind the Session to. When omitted the
    /// Host assigns one; the response always reports the resolved identity.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 256))]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotency_id: Option<String>,
}

impl SessionCreateRequest {
    pub fn validate(&self) -> Result<(), &'static str> {
        self.agent.validate().map_err(|_| "invalid Agent handle")?;
        if let Some(path) = &self.working_dir {
            path.validate().map_err(|_| "invalid working_dir")?;
        }
        if self
            .session_id
            .as_ref()
            .is_some_and(|id| id.trim().is_empty() || id.chars().count() > 256)
        {
            return Err("session_id must be non-empty and bounded");
        }
        if self
            .idempotency_id
            .as_ref()
            .is_some_and(|id| id.trim().is_empty() || id.chars().count() > 256)
        {
            return Err("idempotency_id must be non-empty and bounded");
        }
        Ok(())
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, JsonRpcResponse,
)]
#[serde(deny_unknown_fields)]
pub struct SessionCreateResponse {
    pub session: WireHandle,
    /// ACP Session identity of the same Session object, so the SDK Client
    /// can address the standard `session/prompt` / `session/cancel` methods
    /// on it without a second creation step.
    #[schemars(length(min = 1, max = 256))]
    pub acp_session_id: String,
}

/// `_echo_agent/session/load` request: resume a persisted session by
/// framework identity. Only state roots configured for persistence can
/// serve it; loading mints fresh-generation Session/Run/Stream handles and
/// never revives an interrupted Driver.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, JsonRpcRequest,
)]
#[request(method = "_echo_agent/session/load", response = SessionLoadResponse)]
#[serde(deny_unknown_fields)]
pub struct SessionLoadRequest {
    pub agent: WireHandle,
    #[schemars(length(min = 1, max = 256))]
    pub session_id: String,
    /// Absolute primary working directory for the resumed Session.
    pub working_dir: Option<WirePath>,
}

impl SessionLoadRequest {
    pub fn validate(&self) -> Result<(), &'static str> {
        self.agent.validate().map_err(|_| "invalid Agent handle")?;
        if self.session_id.trim().is_empty() || self.session_id.chars().count() > 256 {
            return Err("session_id must be non-empty and bounded");
        }
        if let Some(path) = &self.working_dir {
            path.validate().map_err(|_| "invalid working_dir")?;
        }
        Ok(())
    }
}

#[derive(
    Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema, JsonRpcResponse,
)]
#[serde(deny_unknown_fields)]
pub struct SessionLoadResponse {
    pub session: WireHandle,
    /// ACP Session identity of the resumed Session (see
    /// [`SessionCreateResponse::acp_session_id`]).
    #[schemars(length(min = 1, max = 256))]
    pub acp_session_id: String,
    /// Sequence watermark the session recovered to, when replayable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovered_sequence: Option<WireU64>,
    /// Historical runs recovered for this session, in run start order.
    /// Interrupted runs carry `status: "interrupted"` without terminal or
    /// receipt; settled runs keep their single authoritative terminal.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runs: Vec<RecoveredRunWire>,
}

/// One recovered historical Run with its fresh-generation handles.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecoveredRunWire {
    pub run: WireHandle,
    pub stream: WireHandle,
    pub status: RunStatus,
    /// Last sequence the recovered event history covers.
    pub last_sequence: WireU64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal: Option<RunTerminal>,
}

/// `_echo_agent/session/close` request/response.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, JsonRpcRequest,
)]
#[request(method = "_echo_agent/session/close", response = SessionCloseResponse)]
#[serde(deny_unknown_fields)]
pub struct SessionCloseRequest {
    pub session: WireHandle,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, JsonRpcResponse,
)]
pub struct SessionCloseResponse {
    pub released: bool,
}

// ── Runs ────────────────────────────────────────────────────────────────────

/// Input of one run: a chat prompt or an execute directive, mirroring the
/// facade's unified turn driver. The payload is typed — the Host never
/// guesses payload tags or applies defaults.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RunInput {
    /// Interactive chat turn with one text message.
    Chat {
        #[schemars(length(min = 1, max = 1_048_576))]
        text: String,
    },
    /// Non-interactive execution directive.
    Execute {
        #[schemars(length(min = 1, max = 1_048_576))]
        task: String,
    },
}

impl RunInput {
    pub fn validate(&self) -> Result<(), &'static str> {
        let (empty, over_limit) = match self {
            RunInput::Chat { text } => (text.trim().is_empty(), text.chars().count() > 1_048_576),
            RunInput::Execute { task } => {
                (task.trim().is_empty(), task.chars().count() > 1_048_576)
            }
        };
        if empty {
            Err("run input text must not be empty")
        } else if over_limit {
            Err("run input text exceeds the character bound")
        } else {
            Ok(())
        }
    }
}

impl RunSteerRequest {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.text.trim().is_empty() {
            return Err("steer text must not be empty");
        }
        if self.text.chars().count() > 1_048_576 {
            return Err("steer text exceeds the character bound");
        }
        Ok(())
    }
}

impl RunTerminal {
    pub fn validate(&self) -> Result<(), &'static str> {
        match self {
            RunTerminal::Completed { final_answer } => {
                if final_answer
                    .as_ref()
                    .is_some_and(|text| text.is_empty() || text.chars().count() > 1_048_576)
                {
                    Err("completed terminal final_answer must be omitted when empty")
                } else {
                    Ok(())
                }
            }
            RunTerminal::Cancelled => Ok(()),
            RunTerminal::Failed { failure } => failure.validate(),
        }
    }
}

impl RunReceiptWire {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.turn_id.trim().is_empty() || self.turn_id.chars().count() > 256 {
            return Err("receipt turn_id must be non-empty and bounded");
        }
        if !matches!(self.outcome.as_str(), "completed" | "cancelled" | "failed") {
            return Err("receipt outcome must match its terminal");
        }
        if self
            .final_answer
            .as_ref()
            .is_some_and(|answer| answer.chars().count() > 1_048_576)
        {
            return Err("receipt final_answer exceeds the character bound");
        }
        if self.final_message_id.as_ref().is_some_and(|message_id| {
            message_id.trim().is_empty() || message_id.chars().count() > 256
        }) {
            return Err("receipt final_message_id must be non-empty and bounded");
        }
        Ok(())
    }
}

impl RecoveredRunWire {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.run.kind != HandleKind::Run || self.stream.kind != HandleKind::Stream {
            return Err("recovered run handles must be run and stream kind");
        }
        if self.status == RunStatus::Interrupted && self.terminal.is_some() {
            return Err("interrupted runs never carry a terminal");
        }
        if let Some(terminal) = &self.terminal {
            terminal.validate()?;
        }
        Ok(())
    }
}

/// `_echo_agent/run/start` request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema, JsonRpcRequest)]
#[request(method = "_echo_agent/run/start", response = RunStartResponse)]
#[serde(deny_unknown_fields)]
pub struct RunStartRequest {
    pub session: WireHandle,
    pub input: RunInput,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 256))]
    pub idempotency_id: Option<String>,
}

impl RunStartRequest {
    pub fn validate(&self) -> Result<(), &'static str> {
        self.session
            .validate()
            .map_err(|_| "invalid Session handle")?;
        self.input.validate()?;
        if self
            .idempotency_id
            .as_ref()
            .is_some_and(|id| id.trim().is_empty() || id.chars().count() > 256)
        {
            return Err("idempotency_id must be non-empty and bounded");
        }
        Ok(())
    }
}

#[derive(
    Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema, JsonRpcResponse,
)]
#[serde(deny_unknown_fields)]
pub struct RunStartResponse {
    pub run: WireHandle,
    /// Live event stream of the run; `_echo_agent/event` notifications and
    /// `run/replay` requests address it by this handle.
    pub stream: WireHandle,
    /// First accepted event of the run, if already available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_event: Option<WireEventEnvelope>,
}

/// `_echo_agent/run/get` request/response.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, JsonRpcRequest,
)]
#[request(method = "_echo_agent/run/get", response = RunGetResponse)]
#[serde(deny_unknown_fields)]
pub struct RunGetRequest {
    pub run: WireHandle,
}

/// Run state snapshot: status, the single authoritative terminal (when
/// settled) and the receipt facts. Never synthesizes a terminal that the
/// framework has not emitted (exactly-one-terminal, design §11.1).
#[derive(
    Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema, JsonRpcResponse,
)]
#[serde(deny_unknown_fields)]
pub struct RunGetResponse {
    pub status: RunStatus,
    /// Last sequence the snapshot covers.
    pub last_sequence: WireU64,
    /// Live event stream of the run, when one exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<WireHandle>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal: Option<RunTerminal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt: Option<RunReceiptWire>,
}

/// `_echo_agent/run/wait` request: bounded wait for the terminal. A run
/// that is already `interrupted` never settles — the wait responds with the
/// typed `host_exited` error instead of success.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, JsonRpcRequest,
)]
#[request(method = "_echo_agent/run/wait", response = RunWaitResponse)]
#[serde(deny_unknown_fields)]
pub struct RunWaitRequest {
    pub run: WireHandle,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<WireDuration>,
}

impl RunWaitRequest {
    pub fn validate(&self) -> Result<(), &'static str> {
        self.run.validate().map_err(|_| "invalid Run handle")?;
        if self.timeout.as_ref().is_some_and(|timeout| {
            timeout.validate().is_err() || timeout.seconds.to_u64().is_none()
        }) {
            return Err("timeout nanos must be below one second");
        }
        Ok(())
    }
}

#[derive(
    Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema, JsonRpcResponse,
)]
#[serde(deny_unknown_fields)]
pub struct RunWaitResponse {
    pub settled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal: Option<RunTerminal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt: Option<RunReceiptWire>,
}

/// `_echo_agent/run/cancel` request. Competing with natural completion, the
/// framework's own CAS/terminal semantics decide the unique outcome; the
/// transport never writes a second terminal (design §14.3).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, JsonRpcRequest,
)]
#[request(method = "_echo_agent/run/cancel", response = RunCancelResponse)]
#[serde(deny_unknown_fields)]
pub struct RunCancelRequest {
    pub run: WireHandle,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, JsonRpcResponse,
)]
#[serde(deny_unknown_fields)]
pub struct RunCancelResponse {
    /// Whether this call initiated cancellation.
    pub cancellation_initiated: bool,
    /// Status at the time of the call; final state still arrives as events.
    pub status: RunStatus,
}

/// `_echo_agent/run/steer` request: mid-flight steering for chats that
/// support it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema, JsonRpcRequest)]
#[request(method = "_echo_agent/run/steer", response = RunSteerResponse)]
#[serde(deny_unknown_fields)]
pub struct RunSteerRequest {
    pub run: WireHandle,
    #[schemars(length(min = 1, max = 1_048_576))]
    pub text: String,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, JsonRpcResponse,
)]
#[serde(deny_unknown_fields)]
pub struct RunSteerResponse {
    pub accepted: bool,
    /// Steer identity assigned by the framework when accepted.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 256))]
    pub steer_id: Option<String>,
}

/// The single authoritative terminal of a settled run. There is no
/// `interrupted` terminal: interruption is a run *status* without terminal
/// or receipt, so a crashed run can never be mistaken for success.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum RunTerminal {
    Completed {
        #[serde(skip_serializing_if = "Option::is_none")]
        final_answer: Option<String>,
    },
    Cancelled,
    Failed {
        /// Lossless framework failure contract (category, terminal kind,
        /// retryability, code, bounded message).
        failure: crate::error::AgentFailureWire,
    },
}

/// Typed projection of the framework `TurnReceipt` (design §4.1). Counters
/// use `WireU64` so JavaScript numbers never see an unsafe integer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunReceiptWire {
    #[schemars(length(min = 1, max = 256))]
    pub turn_id: String,
    /// `completed`, `cancelled`, or `failed` — identical to the terminal.
    #[schemars(length(min = 1, max = 32))]
    pub outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_answer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_message_id: Option<String>,
    pub prompt_tokens: WireU64,
    pub completion_tokens: WireU64,
    pub llm_calls: WireU64,
    pub compaction_count: WireU64,
    /// Sequence watermark of the last emitted event; aligned with the
    /// journal and replay cursors.
    pub last_event_sequence: WireU64,
    /// Total wall time of the run in milliseconds.
    pub elapsed_ms: WireU64,
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

/// Bound of a client-side implementation identity.
pub const MAX_EXTENSION_IMPLEMENTATION_ID_CHARS: usize = 256;
/// Bound of the serialized extension descriptor accepted at registration.
pub const MAX_EXTENSION_DESCRIPTOR_BYTES: usize = 65_536;
/// Bound of one serialized extension invocation input or result payload.
pub const MAX_EXTENSION_PAYLOAD_BYTES: usize = 1_048_576;
/// Bound of one serialized extension stream chunk payload.
pub const MAX_EXTENSION_STREAM_CHUNK_BYTES: usize = 262_144;

/// Model input modality a Tool descriptor may require (wire projection of
/// the framework `ModelInputModality`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ModelModalityWire {
    Text,
    Image,
    Audio,
    Video,
}

/// Search modes a Store descriptor may declare (wire projection of the
/// framework `SearchMode`). Declaring a mode does not downgrade it: the
/// descriptor is a promise the implementation keeps, not a Host-side default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SearchModeWire {
    Keyword,
    Semantic,
    Hybrid,
}

/// Versioned per-kind registration descriptor. Exactly one variant matches
/// the registration's [`ExtensionKind`]; the Host dispatches on this typed
/// snapshot and never guesses trait semantics from free-form JSON (design
/// §12.2). `descriptor_version` gates evolution: unknown versions fail with
/// `invalid_config` instead of being partially applied.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExtensionDescriptor {
    Tool {
        descriptor_version: u32,
        #[schemars(length(min = 1, max = 128))]
        name: String,
        #[schemars(length(max = 8192))]
        description: String,
        /// JSON Schema of the tool parameters.
        parameters: WireValue,
        schema_revision: WireU64,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        required_input_modalities: Vec<ModelModalityWire>,
        supports_streaming: bool,
    },
    LlmClient {
        descriptor_version: u32,
        #[schemars(length(min = 1, max = 256))]
        model_name: String,
        supports_streaming: bool,
    },
    Store {
        descriptor_version: u32,
        /// Search modes the implementation actually supports. Semantic or
        /// hybrid searches against an implementation that did not declare
        /// them are rejected before any callback is sent.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        search_modes: Vec<SearchModeWire>,
    },
    HumanLoopProvider {
        descriptor_version: u32,
    },
    Hook {
        descriptor_version: u32,
        /// Hook events the implementation subscribes to (framework hook
        /// event names). Empty means the implementation decides per context.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        events: Vec<String>,
    },
    AgentCallback {
        descriptor_version: u32,
    },
    InterventionCallback {
        descriptor_version: u32,
    },
    AgentFactory {
        descriptor_version: u32,
    },
    CustomAgent {
        descriptor_version: u32,
        #[schemars(length(min = 1, max = 256))]
        name: String,
        #[schemars(length(min = 1, max = 256))]
        model_name: String,
        system_prompt: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tool_names: Vec<String>,
    },
}

impl ExtensionDescriptor {
    /// The extension kind this descriptor addresses.
    pub fn kind(&self) -> ExtensionKind {
        match self {
            ExtensionDescriptor::Tool { .. } => ExtensionKind::Tool,
            ExtensionDescriptor::LlmClient { .. } => ExtensionKind::LlmClient,
            ExtensionDescriptor::Store { .. } => ExtensionKind::Store,
            ExtensionDescriptor::HumanLoopProvider { .. } => ExtensionKind::HumanLoopProvider,
            ExtensionDescriptor::Hook { .. } => ExtensionKind::Hook,
            ExtensionDescriptor::AgentCallback { .. } => ExtensionKind::AgentCallback,
            ExtensionDescriptor::InterventionCallback { .. } => ExtensionKind::InterventionCallback,
            ExtensionDescriptor::AgentFactory { .. } => ExtensionKind::AgentFactory,
            ExtensionDescriptor::CustomAgent { .. } => ExtensionKind::CustomAgent,
        }
    }

    /// Validate the typed shape: known descriptor version, bounded strings
    /// and a well-formed parameters schema. Unknown versions fail closed.
    pub fn validate(&self) -> Result<(), &'static str> {
        const SUPPORTED_DESCRIPTOR_VERSION: u32 = 1;
        let version = match self {
            ExtensionDescriptor::Tool {
                descriptor_version, ..
            }
            | ExtensionDescriptor::LlmClient {
                descriptor_version, ..
            }
            | ExtensionDescriptor::Store {
                descriptor_version, ..
            }
            | ExtensionDescriptor::HumanLoopProvider {
                descriptor_version, ..
            }
            | ExtensionDescriptor::Hook {
                descriptor_version, ..
            }
            | ExtensionDescriptor::AgentCallback {
                descriptor_version, ..
            }
            | ExtensionDescriptor::InterventionCallback {
                descriptor_version, ..
            }
            | ExtensionDescriptor::AgentFactory {
                descriptor_version, ..
            }
            | ExtensionDescriptor::CustomAgent {
                descriptor_version, ..
            } => *descriptor_version,
        };
        if version != SUPPORTED_DESCRIPTOR_VERSION {
            return Err("unsupported extension descriptor_version");
        }
        if let ExtensionDescriptor::Tool {
            name,
            description,
            parameters,
            ..
        } = self
        {
            if name.trim().is_empty() || name.chars().count() > 128 {
                return Err("tool descriptor name must be non-empty and bounded");
            }
            if description.chars().count() > 8192 {
                return Err("tool descriptor description exceeds its bound");
            }
            parameters
                .validate()
                .map_err(|_| "tool descriptor parameters are not a valid wire value")?;
        }
        if let ExtensionDescriptor::LlmClient { model_name, .. } = self
            && (model_name.trim().is_empty() || model_name.chars().count() > 256)
        {
            return Err("llm client descriptor model_name must be non-empty and bounded");
        }
        if let ExtensionDescriptor::CustomAgent {
            name, model_name, ..
        } = self
        {
            if name.trim().is_empty() || name.chars().count() > 256 {
                return Err("custom agent descriptor name must be non-empty and bounded");
            }
            if model_name.trim().is_empty() || model_name.chars().count() > 256 {
                return Err("custom agent descriptor model_name must be non-empty and bounded");
            }
        }
        Ok(())
    }

    /// Canonical fingerprint for idempotent registration comparison: the
    /// canonical JSON of the descriptor. Registration identity plus this
    /// fingerprint decides same-handle idempotency vs typed conflict.
    pub fn fingerprint(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "<unencodable>".to_string())
    }
}

/// Closed operation set of the extension bridge. Every reverse invocation
/// names exactly one operation; `kind()` binds it to its extension family so
/// the Host can reject an operation dispatched to the wrong kind before any
/// callback leaves the process (design §12.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionOperation {
    // Tool
    ToolExecute,
    ToolExecuteStream,
    ToolValidateParameters,
    // LlmClient
    LlmChat,
    LlmChatStream,
    // Store
    StorePut,
    StoreGet,
    StoreSearch,
    StoreSearchWith,
    StoreDelete,
    StoreListNamespaces,
    StoreList,
    StorePruneExpired,
    StoreDedupByContent,
    // HumanLoopProvider
    HumanLoopRequest,
    // Hook
    HookRun,
    // AgentCallback
    CallbackOnThinkStart,
    CallbackOnThinkEnd,
    CallbackOnToolStart,
    CallbackOnToolEnd,
    CallbackOnToolError,
    CallbackOnFinalAnswer,
    CallbackOnIteration,
    // InterventionCallback
    InterventionOnToolCall,
    InterventionOnThinkStart,
    InterventionOnFinalAnswer,
    // AgentFactory
    FactoryCreateAgent,
    // CustomAgent
    AgentExecute,
    AgentExecuteStream,
    AgentChat,
    AgentChatStream,
    AgentClose,
}

impl ExtensionOperation {
    pub fn as_str(&self) -> &'static str {
        match self {
            ExtensionOperation::ToolExecute => "tool_execute",
            ExtensionOperation::ToolExecuteStream => "tool_execute_stream",
            ExtensionOperation::ToolValidateParameters => "tool_validate_parameters",
            ExtensionOperation::LlmChat => "llm_chat",
            ExtensionOperation::LlmChatStream => "llm_chat_stream",
            ExtensionOperation::StorePut => "store_put",
            ExtensionOperation::StoreGet => "store_get",
            ExtensionOperation::StoreSearch => "store_search",
            ExtensionOperation::StoreSearchWith => "store_search_with",
            ExtensionOperation::StoreDelete => "store_delete",
            ExtensionOperation::StoreListNamespaces => "store_list_namespaces",
            ExtensionOperation::StoreList => "store_list",
            ExtensionOperation::StorePruneExpired => "store_prune_expired",
            ExtensionOperation::StoreDedupByContent => "store_dedup_by_content",
            ExtensionOperation::HumanLoopRequest => "human_loop_request",
            ExtensionOperation::HookRun => "hook_run",
            ExtensionOperation::CallbackOnThinkStart => "callback_on_think_start",
            ExtensionOperation::CallbackOnThinkEnd => "callback_on_think_end",
            ExtensionOperation::CallbackOnToolStart => "callback_on_tool_start",
            ExtensionOperation::CallbackOnToolEnd => "callback_on_tool_end",
            ExtensionOperation::CallbackOnToolError => "callback_on_tool_error",
            ExtensionOperation::CallbackOnFinalAnswer => "callback_on_final_answer",
            ExtensionOperation::CallbackOnIteration => "callback_on_iteration",
            ExtensionOperation::InterventionOnToolCall => "intervention_on_tool_call",
            ExtensionOperation::InterventionOnThinkStart => "intervention_on_think_start",
            ExtensionOperation::InterventionOnFinalAnswer => "intervention_on_final_answer",
            ExtensionOperation::FactoryCreateAgent => "factory_create_agent",
            ExtensionOperation::AgentExecute => "agent_execute",
            ExtensionOperation::AgentExecuteStream => "agent_execute_stream",
            ExtensionOperation::AgentChat => "agent_chat",
            ExtensionOperation::AgentChatStream => "agent_chat_stream",
            ExtensionOperation::AgentClose => "agent_close",
        }
    }

    pub fn kind(&self) -> ExtensionKind {
        match self {
            ExtensionOperation::ToolExecute
            | ExtensionOperation::ToolExecuteStream
            | ExtensionOperation::ToolValidateParameters => ExtensionKind::Tool,
            ExtensionOperation::LlmChat | ExtensionOperation::LlmChatStream => {
                ExtensionKind::LlmClient
            }
            ExtensionOperation::StorePut
            | ExtensionOperation::StoreGet
            | ExtensionOperation::StoreSearch
            | ExtensionOperation::StoreSearchWith
            | ExtensionOperation::StoreDelete
            | ExtensionOperation::StoreListNamespaces
            | ExtensionOperation::StoreList
            | ExtensionOperation::StorePruneExpired
            | ExtensionOperation::StoreDedupByContent => ExtensionKind::Store,
            ExtensionOperation::HumanLoopRequest => ExtensionKind::HumanLoopProvider,
            ExtensionOperation::HookRun => ExtensionKind::Hook,
            ExtensionOperation::CallbackOnThinkStart
            | ExtensionOperation::CallbackOnThinkEnd
            | ExtensionOperation::CallbackOnToolStart
            | ExtensionOperation::CallbackOnToolEnd
            | ExtensionOperation::CallbackOnToolError
            | ExtensionOperation::CallbackOnFinalAnswer
            | ExtensionOperation::CallbackOnIteration => ExtensionKind::AgentCallback,
            ExtensionOperation::InterventionOnToolCall
            | ExtensionOperation::InterventionOnThinkStart
            | ExtensionOperation::InterventionOnFinalAnswer => {
                ExtensionKind::InterventionCallback
            }
            ExtensionOperation::FactoryCreateAgent => ExtensionKind::AgentFactory,
            ExtensionOperation::AgentExecute
            | ExtensionOperation::AgentExecuteStream
            | ExtensionOperation::AgentChat
            | ExtensionOperation::AgentChatStream
            | ExtensionOperation::AgentClose => ExtensionKind::CustomAgent,
        }
    }

    /// Whether the operation delivers its payload through an
    /// `_echo_agent/extension/stream` sequence instead of one result value.
    pub fn is_streaming(&self) -> bool {
        matches!(
            self,
            ExtensionOperation::ToolExecuteStream
                | ExtensionOperation::LlmChatStream
                | ExtensionOperation::AgentExecuteStream
                | ExtensionOperation::AgentChatStream
        )
    }
}

/// Session/run context attached to a reverse invocation, so a language SDK
/// can correlate callbacks with the execution that caused them. Context is
/// diagnostic identity only — it never changes settlement semantics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExtensionInvocationContext {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 256))]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 256))]
    pub run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 256))]
    pub stream_id: Option<String>,
}

/// `_echo_agent/extension/register` request: register a host-language
/// implementation of a public framework trait (Tool, LlmClient, Store,
/// HumanLoopProvider, Hook, AgentCallback, InterventionCallback,
/// AgentFactory, custom Agent). Registration is owned by the current
/// connection generation: it never survives a Host restart or a reconnect.
#[derive(
    Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema, JsonRpcRequest,
)]
#[request(method = "_echo_agent/extension/register", response = ExtensionRegisterResponse)]
#[serde(deny_unknown_fields)]
pub struct ExtensionRegisterRequest {
    /// Which extension point is implemented.
    pub kind: ExtensionKind,
    /// Client-side implementation identity (non-empty). Re-registering the
    /// same identity with the same descriptor returns the same handle;
    /// a different descriptor is a typed conflict.
    #[schemars(length(min = 1, max = 256))]
    pub implementation_id: String,
    /// Typed per-kind descriptor snapshot the Host dispatches on.
    pub descriptor: ExtensionDescriptor,
    /// Per-registration default deadline for reverse invocations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<WireDuration>,
}

impl ExtensionRegisterRequest {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self
            .implementation_id
            .trim()
            .is_empty()
            || self.implementation_id.chars().count() > MAX_EXTENSION_IMPLEMENTATION_ID_CHARS
        {
            return Err("implementation_id must be non-empty and bounded");
        }
        if self.descriptor.kind() != self.kind {
            return Err("descriptor kind does not match the registration kind");
        }
        self.descriptor.validate()?;
        let encoded = serde_json::to_vec(&self.descriptor)
            .map_err(|_| "descriptor is not encodable")?;
        if encoded.len() > MAX_EXTENSION_DESCRIPTOR_BYTES {
            return Err("descriptor exceeds the serialized descriptor bound");
        }
        if let Some(timeout) = &self.timeout {
            timeout
                .validate()
                .map_err(|_| "registration timeout is out of range")?;
        }
        Ok(())
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, JsonRpcResponse,
)]
#[serde(deny_unknown_fields)]
pub struct ExtensionRegisterResponse {
    pub extension: WireHandle,
}

/// `_echo_agent/extension/unregister` request/response.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, JsonRpcRequest,
)]
#[request(method = "_echo_agent/extension/unregister", response = ExtensionUnregisterResponse)]
#[serde(deny_unknown_fields)]
pub struct ExtensionUnregisterRequest {
    pub extension: WireHandle,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, JsonRpcResponse,
)]
pub struct ExtensionUnregisterResponse {
    /// True when this call released the extension; false when it was already
    /// released (idempotent unregister).
    pub released: bool,
}

/// `_echo_agent/extension/invoke` reverse request (Host -> SDK): invoke a
/// registered implementation. The invocation identity is an independent
/// string — never the JSON-RPC request id — and settles exactly once. The
/// SDK dispatcher runs the host-language code and replies with exactly one
/// of `result`/`stream`/`error`.
#[derive(
    Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema, JsonRpcRequest,
)]
#[request(method = "_echo_agent/extension/invoke", response = ExtensionInvokeOutcome)]
#[serde(deny_unknown_fields)]
pub struct ExtensionInvokeCall {
    pub extension: WireHandle,
    /// Invocation identity; unique per call, used for cancellation. It is a
    /// domain identity independent from any JSON-RPC request id.
    #[schemars(length(min = 1, max = 256))]
    pub invocation_id: String,
    /// Which trait operation is being invoked.
    pub operation: ExtensionOperation,
    /// Session/run correlation identity (diagnostic only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<ExtensionInvocationContext>,
    /// Typed invocation payload selected by kind + operation (tool input,
    /// chat request, store op, ...). The Host never guesses a payload shape
    /// from free-form JSON: it constructs this value from real Rust trait
    /// arguments.
    pub input: WireValue,
    /// Total deadline for this invocation, including stream delivery.
    pub deadline: WireDuration,
    /// For streaming operations: the Host-minted stream handle the SDK must
    /// acknowledge and address `_echo_agent/extension/stream` notifications
    /// to. The SDK never invents stream identities.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<WireHandle>,
}

impl ExtensionInvokeCall {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.invocation_id.trim().is_empty()
            || self.invocation_id.chars().count() > MAX_EXTENSION_IMPLEMENTATION_ID_CHARS
        {
            return Err("invocation_id must be non-empty and bounded");
        }
        self.extension
            .validate()
            .map_err(|_| "invalid extension handle")?;
        if self.extension.kind != HandleKind::Extension {
            return Err("invoke requires an extension handle");
        }
        if !self.operation.is_streaming() && self.stream.is_some() {
            return Err("non-streaming operations must not carry a stream handle");
        }
        if self.operation.is_streaming() {
            let Some(stream) = &self.stream else {
                return Err("streaming operations require a stream handle");
            };
            stream.validate().map_err(|_| "invalid stream handle")?;
            if stream.kind != HandleKind::Stream {
                return Err("streaming operations require a stream-kind handle");
            }
        }
        self.input
            .validate()
            .map_err(|_| "invalid invocation payload")?;
        let encoded = serde_json::to_vec(&self.input).map_err(|_| "payload is not encodable")?;
        if encoded.len() > MAX_EXTENSION_PAYLOAD_BYTES {
            return Err("invocation payload exceeds the serialized bound");
        }
        self.deadline
            .validate()
            .map_err(|_| "invocation deadline is out of range")?;
        Ok(())
    }
}

/// One callback outcome. Failures use the typed extension errors; there is
/// no implicit fallback to a built-in implementation (design §12.1).
#[derive(
    Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema, JsonRpcResponse,
)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExtensionInvokeOutcome {
    Result { value: WireValue },
    /// Streaming acknowledgement: the SDK echoes the Host-minted stream
    /// handle and delivers the payload through
    /// `_echo_agent/extension/stream` notifications.
    Stream { stream: WireHandle },
    Error { error: EchoSdkError },
}

impl ExtensionInvokeOutcome {
    pub fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::Result { value } => {
                value.validate().map_err(|_| "invalid callback result")?;
                let encoded =
                    serde_json::to_vec(value).map_err(|_| "callback result is not encodable")?;
                if encoded.len() > MAX_EXTENSION_PAYLOAD_BYTES {
                    return Err("callback result exceeds the serialized bound");
                }
                Ok(())
            }
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
/// still answer the original call with a `cancelled` error or a stream
/// `cancelled` terminal.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, JsonRpcNotification,
)]
#[notification(method = "_echo_agent/extension/cancel")]
#[serde(deny_unknown_fields)]
pub struct ExtensionCancelNotice {
    #[schemars(length(min = 1, max = 256))]
    pub invocation_id: String,
    /// Stable diagnostic reason (`cancelled` or `timeout`).
    #[schemars(length(min = 1, max = 64))]
    pub reason: String,
}

impl ExtensionCancelNotice {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.invocation_id.trim().is_empty()
            || self.invocation_id.chars().count() > MAX_EXTENSION_IMPLEMENTATION_ID_CHARS
        {
            return Err("invocation_id must be non-empty and bounded");
        }
        if self.reason.trim().is_empty() || self.reason.chars().count() > 64 {
            return Err("cancel reason must be non-empty and bounded");
        }
        Ok(())
    }
}

/// `_echo_agent/extension/stream` event (SDK -> Host notification): one
/// chunk or the single terminal of a streaming callback. Sequence is
/// monotonic per stream and exactly one terminal variant may be emitted by
/// the SDK; the Host enforces exactly-one-terminal and discards late events
/// after settlement with bounded diagnostics only.
#[derive(
    Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema, JsonRpcNotification,
)]
#[notification(method = "_echo_agent/extension/stream")]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
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
    pub fn stream(&self) -> &WireHandle {
        match self {
            Self::Chunk { stream, .. }
            | Self::Complete { stream, .. }
            | Self::Failed { stream, .. }
            | Self::Cancelled { stream, .. } => stream,
        }
    }

    pub fn sequence(&self) -> WireNonZeroU64 {
        match self {
            Self::Chunk { sequence, .. }
            | Self::Complete { sequence, .. }
            | Self::Failed { sequence, .. }
            | Self::Cancelled { sequence, .. } => sequence.clone(),
        }
    }

    pub fn is_terminal(&self) -> bool {
        !matches!(self, Self::Chunk { .. })
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        let (stream, sequence) = (self.stream().clone(), self.sequence());
        match self {
            Self::Chunk { value, .. } | Self::Complete { value, .. } => {
                value.validate().map_err(|_| "invalid stream value")?;
                let encoded =
                    serde_json::to_vec(value).map_err(|_| "stream value is not encodable")?;
                if encoded.len() > MAX_EXTENSION_STREAM_CHUNK_BYTES {
                    return Err("stream value exceeds the chunk bound");
                }
            }
            Self::Failed { error, .. } => error.validate()?,
            Self::Cancelled { .. } => {}
        }
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

pub use crate::event::{
    EventAck, EventAckNotification, EventCursor, EventNotification, GapNotification, ReplayRequest,
    ReplayResponse,
};

/// Handle kind check helper used by tests to keep DTOs aligned with the
/// handle taxonomy.
pub fn handle_kind_of(handle: &WireHandle) -> HandleKind {
    handle.kind
}
