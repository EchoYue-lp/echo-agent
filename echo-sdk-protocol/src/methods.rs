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

use agent_client_protocol::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};
use serde::{Deserialize, Serialize};

use crate::error::{AgentFailureWire, EchoSdkError};
use crate::event::WireEventEnvelope;
use crate::handle::{HandleKind, WireHandle};
use crate::scalar::{
    WireDuration, WireField, WireI64, WireNonZeroU64, WirePath, WireU64, WireValue,
};

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
    Critic,
    ChannelPlugin,
    ChannelMessageHandler,
    ContextCompressor,
    AgentComponent,
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
            ExtensionKind::Critic => "critic",
            ExtensionKind::ChannelPlugin => "channel_plugin",
            ExtensionKind::ChannelMessageHandler => "channel_message_handler",
            ExtensionKind::ContextCompressor => "context_compressor",
            ExtensionKind::AgentComponent => "agent_component",
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
    /// Opaque TaskRun handle for this Session's revisioned task graph.
    /// It is issued by the Host and cannot be reconstructed from the ACP
    /// Session identity.
    pub task_run: WireHandle,
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
    /// Fresh-generation TaskRun handle for the resumed Session.
    pub task_run: WireHandle,
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
    /// Interactive chat turn carrying the lossless provider-neutral Message
    /// shape used by the framework's multimodal and tool-call paths.
    ChatMessage { message: LlmMessageWire },
    /// Non-interactive execution carrying the lossless provider-neutral
    /// Message shape. This preserves the concrete ReactAgent
    /// `execute_stream_message` mode without smuggling an execute flag into a
    /// chat payload.
    ExecuteMessage { message: LlmMessageWire },
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
            RunInput::ChatMessage { message } => {
                let encoded = serde_json::to_vec(message).unwrap_or_default();
                (
                    message.role.trim().is_empty(),
                    encoded.len() > 4 * 1024 * 1024,
                )
            }
            RunInput::ExecuteMessage { message } => {
                let encoded = serde_json::to_vec(message).unwrap_or_default();
                (
                    message.role.trim().is_empty(),
                    encoded.len() > 4 * 1024 * 1024,
                )
            }
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
        if let Some(delivery) = self.delivery.as_deref()
            && !matches!(
                delivery,
                "not_attempted" | "delivered" | "closed" | "failed"
            )
        {
            return Err("receipt delivery must be a known delivery status");
        }
        if self.delivery.as_deref() == Some("failed") && self.delivery_error.is_none() {
            return Err("failed delivery must include delivery_error");
        }
        if self.delivery.as_deref() != Some("failed") && self.delivery_error.is_some() {
            return Err("delivery_error requires failed delivery");
        }
        if let Some(failure) = &self.delivery_error {
            failure
                .validate()
                .map_err(|_| "delivery_error is invalid")?;
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
        if self.outcome != "completed"
            && (self.final_answer.is_some() || self.final_message_id.is_some())
        {
            return Err("non-completed receipt must not carry final fields");
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
    /// Delivery status from the driven event sink. Optional only for legacy
    /// persisted receipts created before delivery accounting existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(max = 32))]
    pub delivery: Option<String>,
    /// Lossless delivery failure details when `delivery` is `failed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery_error: Option<AgentFailureWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_answer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_message_id: Option<String>,
    pub prompt_tokens: WireU64,
    pub completion_tokens: WireU64,
    pub llm_calls: WireU64,
    pub compaction_count: WireU64,
    /// Sequence watermark of the last event observed by the driver. A sink
    /// failure may leave the committed Journal/Ledger watermark behind this
    /// value; replay APIs continue to report their own committed watermark.
    pub last_event_sequence: WireU64,
    /// Total wall time of the run in milliseconds.
    pub elapsed_ms: WireU64,
}

// ── Task graph (TaskRun / PlanTask) ────────────────────────────────────────

/// `_echo_agent/task/create` request: one atomic graph creation through
/// the session's TaskRevisionService. The spec is the framework task_create
/// payload (`tasks`, `base_revision`, `reason`, `assumptions`, `risks`,
/// `execution_mode`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonRpcRequest, schemars::JsonSchema)]
#[request(method = "_echo_agent/task/create", response = TaskCreateResponse)]
pub struct TaskCreateRequest {
    /// TaskRun handle; the id is the session-scoped graph identity.
    pub task_run: WireHandle,
    /// TaskCreateInput payload (the exact grammar the in-conversation
    /// `task_create` tool accepts).
    pub spec: WireValue,
}

impl TaskCreateRequest {
    pub fn validate(&self) -> Result<(), &'static str> {
        self.task_run.validate()?;
        if self.task_run.kind != crate::handle::HandleKind::TaskRun {
            return Err("task create requires a task_run handle");
        }
        Ok(())
    }
}

#[derive(
    Debug, Clone, PartialEq, Serialize, Deserialize, JsonRpcResponse, schemars::JsonSchema,
)]
pub struct TaskCreateResponse {
    /// PlanTask handles of every task in the committed graph.
    pub tasks: Vec<WireHandle>,
    pub revision: WireU64,
}

/// `_echo_agent/task/update` request: revision-checked patch through the
/// same TaskRevisionService the in-conversation `task_update` tool uses.
/// The patch is the framework `task_update` payload (`base_revision`,
/// `reason`, `operations`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonRpcRequest, schemars::JsonSchema)]
#[request(method = "_echo_agent/task/update", response = TaskUpdateResponse)]
pub struct TaskUpdateRequest {
    /// TaskRun handle addressing the graph.
    pub task_run: WireHandle,
    pub patch: WireValue,
}

#[derive(
    Debug, Clone, PartialEq, Serialize, Deserialize, JsonRpcResponse, schemars::JsonSchema,
)]
pub struct TaskUpdateResponse {
    pub revision: WireU64,
    pub updated: Vec<WireHandle>,
}

/// `_echo_agent/task/list` request/response.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonRpcRequest, schemars::JsonSchema,
)]
#[request(method = "_echo_agent/task/list", response = TaskListResponse)]
pub struct TaskListRequest {
    pub task_run: WireHandle,
}

#[derive(
    Debug, Clone, PartialEq, Serialize, Deserialize, JsonRpcResponse, schemars::JsonSchema,
)]
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

/// `_echo_agent/task/execute` request: drive one task graph through the
/// framework RuntimeTaskService (single authority for scheduling, waves,
/// retry and terminals). The run continues after the response; terminals
/// are observed through `task/list`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonRpcRequest, schemars::JsonSchema)]
#[request(method = "_echo_agent/task/execute", response = TaskExecuteResponse)]
pub struct TaskExecuteRequest {
    /// TaskRun handle addressing the graph to drive.
    pub task_run: WireHandle,
}

#[derive(
    Debug, Clone, PartialEq, Serialize, Deserialize, JsonRpcResponse, schemars::JsonSchema,
)]
pub struct TaskExecuteResponse {
    /// The driven TaskRun handle.
    pub run: WireHandle,
}

/// `_echo_agent/task/control` request: pause/resume/cancel one exact task
/// through the framework store (claim-settling semantics).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonRpcRequest, schemars::JsonSchema)]
#[request(method = "_echo_agent/task/control", response = TaskControlResponse)]
pub struct TaskControlRequest {
    pub task_run: WireHandle,
    /// PlanTask handle of the controlled task.
    pub task: WireHandle,
    /// One of the framework control verbs (`pause`, `resume`, `cancel`).
    pub action: ControlAction,
}

#[derive(
    Debug, Clone, PartialEq, Serialize, Deserialize, JsonRpcResponse, schemars::JsonSchema,
)]
pub struct TaskControlResponse {
    pub accepted: bool,
    pub status: WireTaskStatus,
}

// ── Subagents ───────────────────────────────────────────────────────────────

/// `_echo_agent/subagent/dispatch` request. The framework executor remains
/// the only scheduler.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonRpcRequest, schemars::JsonSchema)]
#[request(method = "_echo_agent/subagent/dispatch", response = SubagentDispatchResponse)]
pub struct SubagentDispatchRequest {
    pub session: WireHandle,
    /// DispatchRequest payload for the session's SubagentExecutor.
    pub request: WireValue,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 256))]
    pub idempotency_id: Option<String>,
}

#[derive(
    Debug, Clone, PartialEq, Serialize, Deserialize, JsonRpcResponse, schemars::JsonSchema,
)]
pub struct SubagentDispatchResponse {
    pub subagent: WireHandle,
}

/// `_echo_agent/subagent/await` request/response.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonRpcRequest, schemars::JsonSchema,
)]
#[request(method = "_echo_agent/subagent/await", response = SubagentAwaitResponse)]
pub struct SubagentAwaitRequest {
    pub subagent: WireHandle,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<WireDuration>,
}

#[derive(
    Debug, Clone, PartialEq, Serialize, Deserialize, JsonRpcResponse, schemars::JsonSchema,
)]
pub struct SubagentAwaitResponse {
    pub settled: bool,
    /// SubagentResult payload.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<WireValue>,
}

/// Control verbs of `_echo_agent/subagent/control`. These are exactly the
/// SubagentExecutor's real control semantics — there is deliberately no
/// pause/resume: a subagent is not a task graph node (design §10.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SubagentControlAction {
    /// Deliver one tracked message into the running attempt.
    Message,
    /// Queue guidance for the attempt's next planning step.
    Guidance,
    /// Interrupt the running attempt (cooperative, settles as interrupted).
    Interrupt,
    /// Cancel the running attempt and settle its outcome.
    Cancel,
}

/// `_echo_agent/subagent/control` request/response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonRpcRequest, schemars::JsonSchema)]
#[request(method = "_echo_agent/subagent/control", response = SubagentControlResponse)]
pub struct SubagentControlRequest {
    pub subagent: WireHandle,
    pub action: SubagentControlAction,
    /// Message/guidance text for the message and guidance verbs.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 8192))]
    pub payload: Option<String>,
}

#[derive(
    Debug, Clone, PartialEq, Serialize, Deserialize, JsonRpcResponse, schemars::JsonSchema,
)]
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

/// Permission classes declared by a host-language Tool implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolPermissionWire {
    Read,
    Write,
    Network,
    Execute,
    Sensitive,
}

/// Coarse risk level declared by a host-language Tool implementation.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, Default,
)]
#[serde(rename_all = "snake_case")]
pub enum ToolRiskLevelWire {
    ReadOnly,
    #[default]
    Standard,
    Dangerous,
}

fn default_true() -> bool {
    true
}

/// Provider-level LLM capabilities declared by a host-language client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LlmCapabilitiesWire {
    pub streaming_tool_calls: bool,
    pub named_sse_events: bool,
    pub reasoning_content: bool,
    pub image_input: bool,
    pub system_as_top_level: bool,
    pub ndjson_streaming: bool,
    pub tool_support: bool,
    pub structured_output: bool,
    pub requires_version_header: bool,
    pub supports_parallel_tool_calls: bool,
    pub supports_tool_choice_none: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokenizer_name: Option<String>,
}

impl Default for LlmCapabilitiesWire {
    fn default() -> Self {
        Self {
            streaming_tool_calls: true,
            named_sse_events: false,
            reasoning_content: true,
            image_input: true,
            system_as_top_level: false,
            ndjson_streaming: false,
            tool_support: true,
            structured_output: true,
            requires_version_header: false,
            supports_parallel_tool_calls: true,
            supports_tool_choice_none: true,
            tokenizer_name: None,
        }
    }
}

/// Lossless Tool execution context exposed to host-language implementations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ToolContextWire {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<WirePath>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_message: Option<LlmMessageWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_artifacts: Option<ToolOutputArtifactConfigWire>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ToolOutputArtifactConfigWire {
    pub root_dir: WirePath,
    pub retention: String,
    pub threshold_bytes: WireU64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_age_secs: Option<WireU64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ToolExecuteInput {
    pub parameters: WireValue,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<ToolContextWire>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ToolValidateInput {
    pub parameters: WireValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ToolResultKindWire {
    Text,
    Json,
    Image {
        mime_type: String,
    },
    Table {
        columns: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    Diff {
        unified_diff: String,
    },
    FileReference {
        path: String,
    },
    CommandOutput {
        exit_code: Option<i32>,
    },
    SkillActivation {
        name: String,
    },
    StructuredError {
        error_code: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ToolFailureWire {
    pub category: String,
    pub recovery: String,
    pub side_effect: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<WireU64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub postcondition: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ToolOutputArtifactRefWire {
    pub path: WirePath,
    pub artifact_bytes: WireU64,
    pub payload_bytes: WireU64,
    pub sha256: String,
    pub retention: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ToolResultContentWire {
    ImageUrl {
        url: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ToolResultWire {
    pub kind: ToolResultKindWire,
    pub success: bool,
    pub output: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<ToolFailureWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<WireValue>,
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact: Option<ToolOutputArtifactRefWire>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub metadata: std::collections::BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub model_content: Vec<ToolResultContentWire>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
#[allow(clippy::large_enum_variant)]
pub enum ToolStreamEventWire {
    Progress {
        message: String,
        percent: Option<u8>,
    },
    Output {
        channel: String,
        chunk: String,
    },
    Complete {
        result: ToolResultWire,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LlmMessageWire {
    pub role: String,
    pub content: WireValue,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<LlmToolCallWire>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_blocks: Option<Vec<LlmReasoningBlockWire>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LlmToolCallWire {
    pub id: String,
    pub call_type: String,
    pub function_name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LlmReasoningBlockWire {
    Signed {
        thinking: String,
        signature: String,
    },
    Redacted {
        data: String,
    },
    Opaque {
        provider: String,
        id: String,
        data: String,
        summary: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LlmToolDefinitionWire {
    pub tool_type: String,
    pub name: String,
    pub description: String,
    pub parameters: WireValue,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LlmChatRequestWire {
    pub messages: Vec<LlmMessageWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<LlmToolDefinitionWire>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<WireValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<WireValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeouts: Option<WireValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_hints: Option<WireValue>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LlmUsageWire {
    pub value: WireValue,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LlmChatResponseWire {
    pub message: LlmMessageWire,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<LlmUsageWire>,
    pub raw: WireValue,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct LlmChatChunkWire {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_blocks: Option<Vec<LlmReasoningBlockWire>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<LlmDeltaToolCallWire>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<LlmUsageWire>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LlmDeltaToolCallWire {
    pub index: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<LlmDeltaFunctionWire>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct LlmDeltaFunctionWire {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StoreItemWire {
    pub namespace: Vec<String>,
    pub key: String,
    pub value: WireValue,
    pub created_at: WireU64,
    pub updated_at: WireU64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f32>,
    pub importance: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_accessed: Option<WireU64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<WireU64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StorePutInput {
    pub namespace: Vec<String>,
    pub key: String,
    pub value: WireValue,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StoreKeyInput {
    pub namespace: Vec<String>,
    pub key: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StoreSearchInput {
    pub namespace: Vec<String>,
    pub query: String,
    pub limit: WireU64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StoreSearchWithInput {
    pub namespace: Vec<String>,
    pub query: StoreSearchQueryWire,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StoreSearchQueryWire {
    pub text: String,
    pub limit: WireU64,
    pub mode: StoreSearchModeWire,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum StoreSearchModeWire {
    Keyword,
    Semantic,
    Hybrid { vector_weight: Option<f32> },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StoreNamespaceInput {
    pub namespace: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StoreListNamespacesInput {
    pub prefix: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HumanLoopKindWire {
    Approval,
    Input,
    Selection,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HumanRiskLevelWire {
    Low,
    Medium,
    High,
    Critical,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HumanLoopRequestWire {
    pub request_id: Option<String>,
    pub session_id: Option<String>,
    pub agent_name: Option<String>,
    pub kind: HumanLoopKindWire,
    pub prompt: String,
    pub tool_name: Option<String>,
    pub args: Option<WireValue>,
    pub risk_level: Option<HumanRiskLevelWire>,
    pub approval_context: Option<WireValue>,
    pub suggestions: Vec<WireValue>,
    pub timeout: Option<WireDuration>,
    pub task_id: Option<String>,
    pub options: Option<Vec<String>>,
    pub context: Option<WireValue>,
    pub phase: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalScopeWire {
    Once,
    Session,
    SessionTool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "variant", rename_all = "snake_case", deny_unknown_fields)]
pub enum HumanLoopResponseWire {
    Approved,
    ApprovedWithScope {
        scope: ApprovalScopeWire,
    },
    ModifiedArgs {
        args: WireValue,
        scope: ApprovalScopeWire,
    },
    Rejected {
        reason: Option<String>,
    },
    Text {
        text: String,
    },
    Timeout,
    Deferred,
    Selection {
        selection: String,
        instructions: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HookRunInput {
    pub context: WireValue,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct HookResultWire {
    pub block: bool,
    pub block_reason: Option<String>,
    pub updated_input: Option<WireValue>,
    pub messages: Vec<String>,
    pub stop_propagation: bool,
    pub permission_decision: Option<PermissionDecisionWire>,
    pub permission_mode_override: Option<String>,
    pub continue_reason: Option<String>,
    pub injected_context: Option<String>,
    pub retry: bool,
    pub metadata: Option<WireValue>,
    pub activate_skill: Option<ActivateSkillWire>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "decision", rename_all = "snake_case", deny_unknown_fields)]
pub enum PermissionDecisionWire {
    Allow,
    Deny { reason: String },
    RequireApproval,
    Ask { suggestions: Vec<String> },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActivateSkillWire {
    pub name: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CallbackThinkStartInput {
    pub agent: String,
    pub messages: Vec<LlmMessageWire>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CallbackThinkEndInput {
    pub agent: String,
    pub steps: Vec<StepTypeWire>,
    pub prompt_tokens: WireU64,
    pub completion_tokens: WireU64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum StepTypeWire {
    Thought {
        text: String,
    },
    Call {
        tool_call_id: String,
        function_name: String,
        arguments: WireValue,
    },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CallbackToolStartInput {
    pub agent: String,
    pub tool: String,
    pub args: WireValue,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CallbackToolEndInput {
    pub agent: String,
    pub tool: String,
    pub result: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CallbackToolErrorInput {
    pub agent: String,
    pub tool: String,
    pub error: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CallbackFinalAnswerInput {
    pub agent: String,
    pub answer: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CallbackIterationInput {
    pub agent: String,
    pub iteration: WireU64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct InterventionResultWire {
    pub block: bool,
    pub block_reason: Option<String>,
    pub injected_context: Option<String>,
    pub redirect_to: Option<String>,
    pub cancel: bool,
    pub modified_args: Option<WireValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentFactoryConfigWire {
    pub model: String,
    pub name: String,
    pub system_prompt: String,
    pub tool_count: WireU64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CustomAgentDescriptorWire {
    pub name: String,
    pub model_name: String,
    pub system_prompt: String,
    pub tool_names: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentTaskInput {
    pub task: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentMessageInput {
    pub message: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct ExtensionUnit;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChannelChatTypeWire {
    Direct,
    Group,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChannelCapabilitiesWire {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chat_types: Vec<ChannelChatTypeWire>,
    pub supports_media: bool,
    pub supports_threads: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChannelPluginDescriptorWire {
    pub descriptor_version: u32,
    pub channel_id: String,
    pub label: String,
    pub capabilities: ChannelCapabilitiesWire,
    pub handler_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChannelMessageHandlerDescriptorWire {
    pub descriptor_version: u32,
    pub handler_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChannelAttachmentWire {
    pub kind: String,
    pub data_base64: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChannelInboundMessageWire {
    pub channel_id: String,
    pub sender_id: String,
    pub chat_id: String,
    pub chat_type: ChannelChatTypeWire,
    pub text: String,
    pub message_id: String,
    pub timestamp: WireU64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<ChannelAttachmentWire>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChannelOutboundMessageWire {
    pub channel_id: String,
    pub to: String,
    pub chat_type: ChannelChatTypeWire,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<ChannelAttachmentWire>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChannelStartInput {
    pub handler_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChannelSendInput {
    pub message: ChannelOutboundMessageWire,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChannelHandleInput {
    pub message: ChannelInboundMessageWire,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChannelReplyInput {
    pub message: ChannelOutboundMessageWire,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompressionInputWire {
    pub messages: Vec<LlmMessageWire>,
    pub token_limit: WireU64,
    pub current_query: Option<String>,
    pub focus_instructions: Option<String>,
    /// Host-owned tokenizer for this compression pass. SDK implementations
    /// call the canonical `Tokenizer::count_tokens` source operation through
    /// this temporary resource instead of substituting a language heuristic.
    pub tokenizer: TokenizerReferenceWire,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TokenizerReferenceWire {
    pub resource: WireHandle,
    pub owner_session_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompressionOutputWire {
    pub messages: Vec<LlmMessageWire>,
    pub evicted: Vec<LlmMessageWire>,
    pub checkpoint: Option<WireValue>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentComponentKindWire {
    ConversationStore,
    RunStore,
    RuntimeStateStore,
    AuditLogger,
    ContextProjector,
    MemoryTriggerSink,
    Guard,
    SearchProvider,
    WorkflowCheckpointStore,
    RevisionedTaskStore,
    SandboxExecutor,
    McpTransport,
    Embedder,
    MemoryPromoter,
    Workflow,
    IntentClassifier,
    SkillLoadPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentComponentOperationWire {
    ConversationCreate,
    ConversationGet,
    ConversationList,
    ConversationUpdate,
    ConversationDelete,
    ConversationSaveMessages,
    ConversationGetMessages,
    ConversationCountMessages,
    ConversationEnsure,
    ConversationSearch,
    RunSave,
    RunLoad,
    RunListBySession,
    RunListAll,
    RunAppendEvent,
    RunListByParent,
    RuntimeGetCheckpoint,
    RuntimeSaveCheckpoint,
    RuntimeSaveCheckpointForScope,
    RuntimeStateIds,
    RuntimeClearState,
    RuntimeClearScope,
    RuntimeClearConversation,
    AuditLog,
    AuditQuery,
    ContextProject,
    MemoryTrigger,
    GuardCheck,
    SearchProviderSearch,
    WorkflowCheckpointSave,
    WorkflowCheckpointLoad,
    WorkflowCheckpointClaim,
    WorkflowCheckpointList,
    WorkflowCheckpointListByGraph,
    WorkflowCheckpointListFiltered,
    WorkflowCheckpointDelete,
    WorkflowCheckpointClear,
    RevisionedTaskLoad,
    RevisionedTaskCompareAndCommit,
    SandboxIsAvailable,
    SandboxExecute,
    SandboxExecuteStream,
    SandboxExecuteWithLimits,
    SandboxExecuteWithLimitsAndCancel,
    SandboxCleanup,
    McpTransportSend,
    McpTransportNotify,
    McpTransportClose,
    McpTransportTryNotification,
    EmbedderEmbed,
    MemoryPromoterPromote,
    WorkflowRun,
    WorkflowRunStream,
    IntentClassify,
    SkillLoadAllows,
}

impl AgentComponentOperationWire {
    pub fn component(self) -> AgentComponentKindWire {
        match self {
            Self::ConversationCreate
            | Self::ConversationGet
            | Self::ConversationList
            | Self::ConversationUpdate
            | Self::ConversationDelete
            | Self::ConversationSaveMessages
            | Self::ConversationGetMessages
            | Self::ConversationCountMessages
            | Self::ConversationEnsure
            | Self::ConversationSearch => AgentComponentKindWire::ConversationStore,
            Self::RunSave
            | Self::RunLoad
            | Self::RunListBySession
            | Self::RunListAll
            | Self::RunAppendEvent
            | Self::RunListByParent => AgentComponentKindWire::RunStore,
            Self::RuntimeGetCheckpoint
            | Self::RuntimeSaveCheckpoint
            | Self::RuntimeSaveCheckpointForScope
            | Self::RuntimeStateIds
            | Self::RuntimeClearState
            | Self::RuntimeClearScope
            | Self::RuntimeClearConversation => AgentComponentKindWire::RuntimeStateStore,
            Self::AuditLog | Self::AuditQuery => AgentComponentKindWire::AuditLogger,
            Self::ContextProject => AgentComponentKindWire::ContextProjector,
            Self::MemoryTrigger => AgentComponentKindWire::MemoryTriggerSink,
            Self::GuardCheck => AgentComponentKindWire::Guard,
            Self::SearchProviderSearch => AgentComponentKindWire::SearchProvider,
            Self::WorkflowCheckpointSave
            | Self::WorkflowCheckpointLoad
            | Self::WorkflowCheckpointClaim
            | Self::WorkflowCheckpointList
            | Self::WorkflowCheckpointListByGraph
            | Self::WorkflowCheckpointListFiltered
            | Self::WorkflowCheckpointDelete
            | Self::WorkflowCheckpointClear => AgentComponentKindWire::WorkflowCheckpointStore,
            Self::RevisionedTaskLoad | Self::RevisionedTaskCompareAndCommit => {
                AgentComponentKindWire::RevisionedTaskStore
            }
            Self::SandboxIsAvailable
            | Self::SandboxExecute
            | Self::SandboxExecuteStream
            | Self::SandboxExecuteWithLimits
            | Self::SandboxExecuteWithLimitsAndCancel
            | Self::SandboxCleanup => AgentComponentKindWire::SandboxExecutor,
            Self::McpTransportSend
            | Self::McpTransportNotify
            | Self::McpTransportClose
            | Self::McpTransportTryNotification => AgentComponentKindWire::McpTransport,
            Self::EmbedderEmbed => AgentComponentKindWire::Embedder,
            Self::MemoryPromoterPromote => AgentComponentKindWire::MemoryPromoter,
            Self::WorkflowRun | Self::WorkflowRunStream => AgentComponentKindWire::Workflow,
            Self::IntentClassify => AgentComponentKindWire::IntentClassifier,
            Self::SkillLoadAllows => AgentComponentKindWire::SkillLoadPolicy,
        }
    }
}

/// Lossless public projection passed to a host-language [`SkillLoadPolicy`].
/// Fields skipped by the framework's persistence serde remain explicit here
/// because a live policy may legitimately inspect their runtime values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillDescriptorPolicyWire {
    pub name: String,
    pub description: String,
    pub location: WirePath,
    pub license: Option<String>,
    pub compatibility: Option<String>,
    pub metadata: std::collections::BTreeMap<String, String>,
    pub source: Option<String>,
    pub allowed_tools: Vec<String>,
    pub shell: Option<String>,
    pub paths: Vec<String>,
    pub triggers: Vec<String>,
    pub hooks: Option<WireValue>,
    pub sandbox: Option<WireValue>,
    pub depends_on: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(
    tag = "operation",
    content = "input",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum AgentComponentCallInputWire {
    ConversationCreate {
        conversation: WireValue,
    },
    ConversationGet {
        conversation_id: String,
    },
    ConversationList {
        user_id: Option<String>,
        agent_type: Option<String>,
        limit: Option<WireU64>,
        offset: Option<WireU64>,
    },
    ConversationUpdate {
        conversation_id: String,
        title: Option<String>,
        summary: Option<String>,
        compressed_before_id: Option<WireI64>,
    },
    ConversationDelete {
        conversation_id: String,
    },
    ConversationSaveMessages {
        conversation_id: String,
        messages: Vec<WireValue>,
    },
    ConversationGetMessages {
        conversation_id: String,
    },
    ConversationCountMessages {
        conversation_id: String,
    },
    ConversationEnsure {
        conversation: WireValue,
    },
    ConversationSearch {
        query: String,
        limit: WireU64,
    },
    RunSave {
        run: WireValue,
    },
    RunLoad {
        run_id: String,
    },
    RunListBySession {
        session_id: String,
    },
    RunListAll {
        limit: WireU64,
    },
    RunAppendEvent {
        run_id: String,
        event: WireValue,
    },
    RunListByParent {
        parent_run_id: String,
    },
    RuntimeGetCheckpoint {
        conversation_id: String,
    },
    RuntimeSaveCheckpoint {
        checkpoint: WireValue,
    },
    RuntimeSaveCheckpointForScope {
        scope_id: String,
        checkpoint: WireValue,
    },
    RuntimeStateIds {
        scope_id: String,
    },
    RuntimeClearState {
        scope_id: String,
        runtime_state_id: String,
    },
    RuntimeClearScope {
        scope_id: String,
    },
    RuntimeClearConversation {
        conversation_id: String,
    },
    AuditLog {
        event: WireValue,
    },
    AuditQuery {
        session_id: Option<String>,
        agent_name: Option<String>,
        from: Option<String>,
        to: Option<String>,
        limit: Option<WireU64>,
    },
    ContextProject {
        iteration: WireU64,
        agent_name: String,
        session_id: Option<String>,
        conversation_id: Option<String>,
        run_id: Option<String>,
        turn_id: Option<String>,
    },
    MemoryTrigger {
        trigger: WireValue,
    },
    GuardCheck {
        content: String,
        direction: String,
    },
    SearchProviderSearch {
        query: String,
        max_results: WireU64,
    },
    WorkflowCheckpointSave {
        checkpoint: WireValue,
    },
    WorkflowCheckpointLoad {
        checkpoint_id: String,
    },
    WorkflowCheckpointClaim {
        checkpoint_id: String,
    },
    WorkflowCheckpointList,
    WorkflowCheckpointListByGraph {
        graph_name: String,
    },
    WorkflowCheckpointListFiltered {
        filter: WireValue,
    },
    WorkflowCheckpointDelete {
        checkpoint_id: String,
    },
    WorkflowCheckpointClear,
    RevisionedTaskLoad {
        scope_id: String,
    },
    RevisionedTaskCompareAndCommit {
        scope_id: String,
        commit: WireValue,
    },
    SandboxIsAvailable,
    SandboxExecute {
        command: WireValue,
    },
    SandboxExecuteStream {
        command: WireValue,
    },
    SandboxExecuteWithLimits {
        command: WireValue,
        limits: WireValue,
    },
    SandboxExecuteWithLimitsAndCancel {
        command: WireValue,
        limits: WireValue,
    },
    SandboxCleanup,
    McpTransportSend {
        request: WireValue,
    },
    McpTransportNotify {
        notification: WireValue,
    },
    McpTransportClose,
    McpTransportTryNotification,
    EmbedderEmbed {
        text: String,
    },
    MemoryPromoterPromote {
        evicted: Vec<LlmMessageWire>,
    },
    WorkflowRun {
        input: String,
    },
    WorkflowRunStream {
        input: String,
    },
    IntentClassify {
        user_input: String,
        context: Vec<LlmMessageWire>,
    },
    SkillLoadAllows {
        descriptor: Box<SkillDescriptorPolicyWire>,
    },
}

impl AgentComponentCallInputWire {
    pub fn operation(&self) -> AgentComponentOperationWire {
        match self {
            Self::ConversationCreate { .. } => AgentComponentOperationWire::ConversationCreate,
            Self::ConversationGet { .. } => AgentComponentOperationWire::ConversationGet,
            Self::ConversationList { .. } => AgentComponentOperationWire::ConversationList,
            Self::ConversationUpdate { .. } => AgentComponentOperationWire::ConversationUpdate,
            Self::ConversationDelete { .. } => AgentComponentOperationWire::ConversationDelete,
            Self::ConversationSaveMessages { .. } => {
                AgentComponentOperationWire::ConversationSaveMessages
            }
            Self::ConversationGetMessages { .. } => {
                AgentComponentOperationWire::ConversationGetMessages
            }
            Self::ConversationCountMessages { .. } => {
                AgentComponentOperationWire::ConversationCountMessages
            }
            Self::ConversationEnsure { .. } => AgentComponentOperationWire::ConversationEnsure,
            Self::ConversationSearch { .. } => AgentComponentOperationWire::ConversationSearch,
            Self::RunSave { .. } => AgentComponentOperationWire::RunSave,
            Self::RunLoad { .. } => AgentComponentOperationWire::RunLoad,
            Self::RunListBySession { .. } => AgentComponentOperationWire::RunListBySession,
            Self::RunListAll { .. } => AgentComponentOperationWire::RunListAll,
            Self::RunAppendEvent { .. } => AgentComponentOperationWire::RunAppendEvent,
            Self::RunListByParent { .. } => AgentComponentOperationWire::RunListByParent,
            Self::RuntimeGetCheckpoint { .. } => AgentComponentOperationWire::RuntimeGetCheckpoint,
            Self::RuntimeSaveCheckpoint { .. } => {
                AgentComponentOperationWire::RuntimeSaveCheckpoint
            }
            Self::RuntimeSaveCheckpointForScope { .. } => {
                AgentComponentOperationWire::RuntimeSaveCheckpointForScope
            }
            Self::RuntimeStateIds { .. } => AgentComponentOperationWire::RuntimeStateIds,
            Self::RuntimeClearState { .. } => AgentComponentOperationWire::RuntimeClearState,
            Self::RuntimeClearScope { .. } => AgentComponentOperationWire::RuntimeClearScope,
            Self::RuntimeClearConversation { .. } => {
                AgentComponentOperationWire::RuntimeClearConversation
            }
            Self::AuditLog { .. } => AgentComponentOperationWire::AuditLog,
            Self::AuditQuery { .. } => AgentComponentOperationWire::AuditQuery,
            Self::ContextProject { .. } => AgentComponentOperationWire::ContextProject,
            Self::MemoryTrigger { .. } => AgentComponentOperationWire::MemoryTrigger,
            Self::GuardCheck { .. } => AgentComponentOperationWire::GuardCheck,
            Self::SearchProviderSearch { .. } => AgentComponentOperationWire::SearchProviderSearch,
            Self::WorkflowCheckpointSave { .. } => {
                AgentComponentOperationWire::WorkflowCheckpointSave
            }
            Self::WorkflowCheckpointLoad { .. } => {
                AgentComponentOperationWire::WorkflowCheckpointLoad
            }
            Self::WorkflowCheckpointClaim { .. } => {
                AgentComponentOperationWire::WorkflowCheckpointClaim
            }
            Self::WorkflowCheckpointList => AgentComponentOperationWire::WorkflowCheckpointList,
            Self::WorkflowCheckpointListByGraph { .. } => {
                AgentComponentOperationWire::WorkflowCheckpointListByGraph
            }
            Self::WorkflowCheckpointListFiltered { .. } => {
                AgentComponentOperationWire::WorkflowCheckpointListFiltered
            }
            Self::WorkflowCheckpointDelete { .. } => {
                AgentComponentOperationWire::WorkflowCheckpointDelete
            }
            Self::WorkflowCheckpointClear => AgentComponentOperationWire::WorkflowCheckpointClear,
            Self::RevisionedTaskLoad { .. } => AgentComponentOperationWire::RevisionedTaskLoad,
            Self::RevisionedTaskCompareAndCommit { .. } => {
                AgentComponentOperationWire::RevisionedTaskCompareAndCommit
            }
            Self::SandboxIsAvailable => AgentComponentOperationWire::SandboxIsAvailable,
            Self::SandboxExecute { .. } => AgentComponentOperationWire::SandboxExecute,
            Self::SandboxExecuteStream { .. } => AgentComponentOperationWire::SandboxExecuteStream,
            Self::SandboxExecuteWithLimits { .. } => {
                AgentComponentOperationWire::SandboxExecuteWithLimits
            }
            Self::SandboxExecuteWithLimitsAndCancel { .. } => {
                AgentComponentOperationWire::SandboxExecuteWithLimitsAndCancel
            }
            Self::SandboxCleanup => AgentComponentOperationWire::SandboxCleanup,
            Self::McpTransportSend { .. } => AgentComponentOperationWire::McpTransportSend,
            Self::McpTransportNotify { .. } => AgentComponentOperationWire::McpTransportNotify,
            Self::McpTransportClose => AgentComponentOperationWire::McpTransportClose,
            Self::McpTransportTryNotification => {
                AgentComponentOperationWire::McpTransportTryNotification
            }
            Self::EmbedderEmbed { .. } => AgentComponentOperationWire::EmbedderEmbed,
            Self::MemoryPromoterPromote { .. } => {
                AgentComponentOperationWire::MemoryPromoterPromote
            }
            Self::WorkflowRun { .. } => AgentComponentOperationWire::WorkflowRun,
            Self::WorkflowRunStream { .. } => AgentComponentOperationWire::WorkflowRunStream,
            Self::IntentClassify { .. } => AgentComponentOperationWire::IntentClassify,
            Self::SkillLoadAllows { .. } => AgentComponentOperationWire::SkillLoadAllows,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(
    tag = "operation",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum AgentComponentCallResultWire {
    ConversationCreate {
        conversation: WireValue,
    },
    ConversationGet {
        conversation: Option<WireValue>,
    },
    ConversationList {
        conversations: Vec<WireValue>,
    },
    ConversationUpdate,
    ConversationDelete,
    ConversationSaveMessages,
    ConversationGetMessages {
        messages: Vec<WireValue>,
    },
    ConversationCountMessages {
        count: WireU64,
    },
    ConversationEnsure {
        conversation: WireValue,
    },
    ConversationSearch {
        conversations: Vec<WireValue>,
    },
    RunSave,
    RunLoad {
        run: Option<WireValue>,
    },
    RunListBySession {
        runs: Vec<WireValue>,
    },
    RunListAll {
        runs: Vec<WireValue>,
    },
    RunAppendEvent,
    RunListByParent {
        runs: Vec<WireValue>,
    },
    RuntimeGetCheckpoint {
        checkpoint: Option<WireValue>,
    },
    RuntimeSaveCheckpoint,
    RuntimeSaveCheckpointForScope,
    RuntimeStateIds {
        state_ids: Vec<String>,
    },
    RuntimeClearState {
        receipt: WireValue,
    },
    RuntimeClearScope {
        receipt: WireValue,
    },
    RuntimeClearConversation,
    AuditLog,
    AuditQuery {
        events: Vec<WireValue>,
    },
    ContextProject {
        projections: Vec<WireValue>,
    },
    MemoryTrigger {
        disposition: String,
    },
    GuardCheck {
        result: WireValue,
    },
    SearchProviderSearch {
        results: Vec<WireValue>,
    },
    WorkflowCheckpointSave,
    WorkflowCheckpointLoad {
        checkpoint: Option<WireValue>,
    },
    WorkflowCheckpointClaim {
        checkpoint: Option<WireValue>,
    },
    WorkflowCheckpointList {
        checkpoints: Vec<WireValue>,
    },
    WorkflowCheckpointListByGraph {
        checkpoints: Vec<WireValue>,
    },
    WorkflowCheckpointListFiltered {
        checkpoints: Vec<WireValue>,
    },
    WorkflowCheckpointDelete,
    WorkflowCheckpointClear,
    RevisionedTaskLoad {
        graph: Option<WireValue>,
    },
    RevisionedTaskCompareAndCommit {
        graph: WireValue,
    },
    SandboxIsAvailable {
        available: bool,
    },
    SandboxExecute {
        result: WireValue,
    },
    SandboxExecuteWithLimits {
        result: WireValue,
    },
    SandboxExecuteWithLimitsAndCancel {
        result: WireValue,
    },
    SandboxCleanup,
    McpTransportSend {
        response: WireValue,
    },
    McpTransportNotify,
    McpTransportClose,
    McpTransportTryNotification {
        notification: Option<WireValue>,
    },
    EmbedderEmbed {
        vector: Vec<f32>,
    },
    MemoryPromoterPromote {
        submitted: WireU64,
        promoted: WireU64,
        deduplicated: WireU64,
    },
    WorkflowRun {
        output: WireValue,
    },
    IntentClassify {
        intent: WireValue,
    },
    SkillLoadAllows {
        allowed: bool,
    },
}

impl AgentComponentCallResultWire {
    pub fn operation(&self) -> AgentComponentOperationWire {
        match self {
            Self::ConversationCreate { .. } => AgentComponentOperationWire::ConversationCreate,
            Self::ConversationGet { .. } => AgentComponentOperationWire::ConversationGet,
            Self::ConversationList { .. } => AgentComponentOperationWire::ConversationList,
            Self::ConversationUpdate => AgentComponentOperationWire::ConversationUpdate,
            Self::ConversationDelete => AgentComponentOperationWire::ConversationDelete,
            Self::ConversationSaveMessages => AgentComponentOperationWire::ConversationSaveMessages,
            Self::ConversationGetMessages { .. } => {
                AgentComponentOperationWire::ConversationGetMessages
            }
            Self::ConversationCountMessages { .. } => {
                AgentComponentOperationWire::ConversationCountMessages
            }
            Self::ConversationEnsure { .. } => AgentComponentOperationWire::ConversationEnsure,
            Self::ConversationSearch { .. } => AgentComponentOperationWire::ConversationSearch,
            Self::RunSave => AgentComponentOperationWire::RunSave,
            Self::RunLoad { .. } => AgentComponentOperationWire::RunLoad,
            Self::RunListBySession { .. } => AgentComponentOperationWire::RunListBySession,
            Self::RunListAll { .. } => AgentComponentOperationWire::RunListAll,
            Self::RunAppendEvent => AgentComponentOperationWire::RunAppendEvent,
            Self::RunListByParent { .. } => AgentComponentOperationWire::RunListByParent,
            Self::RuntimeGetCheckpoint { .. } => AgentComponentOperationWire::RuntimeGetCheckpoint,
            Self::RuntimeSaveCheckpoint => AgentComponentOperationWire::RuntimeSaveCheckpoint,
            Self::RuntimeSaveCheckpointForScope => {
                AgentComponentOperationWire::RuntimeSaveCheckpointForScope
            }
            Self::RuntimeStateIds { .. } => AgentComponentOperationWire::RuntimeStateIds,
            Self::RuntimeClearState { .. } => AgentComponentOperationWire::RuntimeClearState,
            Self::RuntimeClearScope { .. } => AgentComponentOperationWire::RuntimeClearScope,
            Self::RuntimeClearConversation => AgentComponentOperationWire::RuntimeClearConversation,
            Self::AuditLog => AgentComponentOperationWire::AuditLog,
            Self::AuditQuery { .. } => AgentComponentOperationWire::AuditQuery,
            Self::ContextProject { .. } => AgentComponentOperationWire::ContextProject,
            Self::MemoryTrigger { .. } => AgentComponentOperationWire::MemoryTrigger,
            Self::GuardCheck { .. } => AgentComponentOperationWire::GuardCheck,
            Self::SearchProviderSearch { .. } => AgentComponentOperationWire::SearchProviderSearch,
            Self::WorkflowCheckpointSave => AgentComponentOperationWire::WorkflowCheckpointSave,
            Self::WorkflowCheckpointLoad { .. } => {
                AgentComponentOperationWire::WorkflowCheckpointLoad
            }
            Self::WorkflowCheckpointClaim { .. } => {
                AgentComponentOperationWire::WorkflowCheckpointClaim
            }
            Self::WorkflowCheckpointList { .. } => {
                AgentComponentOperationWire::WorkflowCheckpointList
            }
            Self::WorkflowCheckpointListByGraph { .. } => {
                AgentComponentOperationWire::WorkflowCheckpointListByGraph
            }
            Self::WorkflowCheckpointListFiltered { .. } => {
                AgentComponentOperationWire::WorkflowCheckpointListFiltered
            }
            Self::WorkflowCheckpointDelete => AgentComponentOperationWire::WorkflowCheckpointDelete,
            Self::WorkflowCheckpointClear => AgentComponentOperationWire::WorkflowCheckpointClear,
            Self::RevisionedTaskLoad { .. } => AgentComponentOperationWire::RevisionedTaskLoad,
            Self::RevisionedTaskCompareAndCommit { .. } => {
                AgentComponentOperationWire::RevisionedTaskCompareAndCommit
            }
            Self::SandboxIsAvailable { .. } => AgentComponentOperationWire::SandboxIsAvailable,
            Self::SandboxExecute { .. } => AgentComponentOperationWire::SandboxExecute,
            Self::SandboxExecuteWithLimits { .. } => {
                AgentComponentOperationWire::SandboxExecuteWithLimits
            }
            Self::SandboxExecuteWithLimitsAndCancel { .. } => {
                AgentComponentOperationWire::SandboxExecuteWithLimitsAndCancel
            }
            Self::SandboxCleanup => AgentComponentOperationWire::SandboxCleanup,
            Self::McpTransportSend { .. } => AgentComponentOperationWire::McpTransportSend,
            Self::McpTransportNotify => AgentComponentOperationWire::McpTransportNotify,
            Self::McpTransportClose => AgentComponentOperationWire::McpTransportClose,
            Self::McpTransportTryNotification { .. } => {
                AgentComponentOperationWire::McpTransportTryNotification
            }
            Self::EmbedderEmbed { .. } => AgentComponentOperationWire::EmbedderEmbed,
            Self::MemoryPromoterPromote { .. } => {
                AgentComponentOperationWire::MemoryPromoterPromote
            }
            Self::WorkflowRun { .. } => AgentComponentOperationWire::WorkflowRun,
            Self::IntentClassify { .. } => AgentComponentOperationWire::IntentClassify,
            Self::SkillLoadAllows { .. } => AgentComponentOperationWire::SkillLoadAllows,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentComponentCallWire {
    pub component: AgentComponentKindWire,
    pub call: AgentComponentCallInputWire,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentComponentResultWire {
    pub component: AgentComponentKindWire,
    pub result: AgentComponentCallResultWire,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentComponentCapabilitiesWire {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isolation_level: Option<String>,
    #[serde(default)]
    pub supports_streaming: bool,
    #[serde(default)]
    pub supports_notifications: bool,
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
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        required_permissions: Vec<ToolPermissionWire>,
        #[serde(default)]
        risk_level: ToolRiskLevelWire,
        supports_streaming: bool,
        #[serde(default)]
        exempt_from_batch_timeout: bool,
        #[serde(default = "default_true")]
        allows_parallel_batch_execution: bool,
        #[serde(default)]
        manages_own_timeout: bool,
    },
    LlmClient {
        descriptor_version: u32,
        #[schemars(length(min = 1, max = 256))]
        model_name: String,
        supports_streaming: bool,
        #[serde(default)]
        capabilities: LlmCapabilitiesWire,
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
    Critic {
        descriptor_version: u32,
        #[schemars(length(min = 1, max = 256))]
        name: String,
    },
    ChannelPlugin(ChannelPluginDescriptorWire),
    ChannelMessageHandler(ChannelMessageHandlerDescriptorWire),
    ContextCompressor {
        descriptor_version: u32,
        #[schemars(length(min = 1, max = 256))]
        name: String,
    },
    AgentComponent {
        descriptor_version: u32,
        component: AgentComponentKindWire,
        #[schemars(length(min = 1, max = 256))]
        name: String,
        #[serde(default)]
        capabilities: AgentComponentCapabilitiesWire,
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
            ExtensionDescriptor::Critic { .. } => ExtensionKind::Critic,
            ExtensionDescriptor::ChannelPlugin(_) => ExtensionKind::ChannelPlugin,
            ExtensionDescriptor::ChannelMessageHandler(_) => ExtensionKind::ChannelMessageHandler,
            ExtensionDescriptor::ContextCompressor { .. } => ExtensionKind::ContextCompressor,
            ExtensionDescriptor::AgentComponent { .. } => ExtensionKind::AgentComponent,
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
            }
            | ExtensionDescriptor::ContextCompressor {
                descriptor_version, ..
            }
            | ExtensionDescriptor::AgentComponent {
                descriptor_version, ..
            } => *descriptor_version,
            ExtensionDescriptor::Critic {
                descriptor_version, ..
            } => *descriptor_version,
            ExtensionDescriptor::ChannelPlugin(value) => value.descriptor_version,
            ExtensionDescriptor::ChannelMessageHandler(value) => value.descriptor_version,
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
        if let ExtensionDescriptor::LlmClient {
            model_name,
            capabilities,
            ..
        } = self
        {
            if model_name.trim().is_empty() || model_name.chars().count() > 256 {
                return Err("llm client descriptor model_name must be non-empty and bounded");
            }
            if capabilities
                .tokenizer_name
                .as_deref()
                .is_some_and(|name| !matches!(name, "cl100k_base" | "o200k_base" | "claude"))
            {
                return Err("llm client descriptor tokenizer_name is unsupported by Rust");
            }
        }
        if let ExtensionDescriptor::CustomAgent {
            name,
            model_name,
            system_prompt,
            tool_names,
            ..
        } = self
        {
            if name.trim().is_empty() || name.chars().count() > 256 {
                return Err("custom agent descriptor name must be non-empty and bounded");
            }
            if model_name.trim().is_empty() || model_name.chars().count() > 256 {
                return Err("custom agent descriptor model_name must be non-empty and bounded");
            }
            if system_prompt.chars().count() > 65_536 {
                return Err("custom agent descriptor system_prompt exceeds its bound");
            }
            if tool_names.len() > 1024
                || tool_names
                    .iter()
                    .any(|name| name.trim().is_empty() || name.chars().count() > 256)
            {
                return Err("custom agent descriptor tool_names are empty or exceed their bound");
            }
        }
        if let ExtensionDescriptor::Critic { name, .. } = self
            && (name.trim().is_empty() || name.chars().count() > 256)
        {
            return Err("critic descriptor name is empty or exceeds its bound");
        }
        if let ExtensionDescriptor::ContextCompressor { name, .. } = self
            && (name.trim().is_empty() || name.chars().count() > 256)
        {
            return Err("context compressor descriptor name is empty or exceeds its bound");
        }
        if let ExtensionDescriptor::AgentComponent { name, .. } = self
            && (name.trim().is_empty() || name.chars().count() > 256)
        {
            return Err("agent component descriptor name is empty or exceeds its bound");
        }
        if let ExtensionDescriptor::AgentComponent {
            component,
            capabilities,
            ..
        } = self
        {
            if capabilities
                .isolation_level
                .as_deref()
                .is_some_and(|level| {
                    !matches!(
                        level,
                        "none" | "process" | "os-sandbox" | "container" | "orchestrated"
                    )
                })
            {
                return Err("agent component isolation_level is invalid");
            }
            if capabilities.supports_streaming
                && !matches!(
                    component,
                    AgentComponentKindWire::SandboxExecutor | AgentComponentKindWire::Workflow
                )
            {
                return Err("streaming is only valid for sandbox and workflow components");
            }
            if *component != AgentComponentKindWire::SandboxExecutor
                && capabilities.isolation_level.is_some()
            {
                return Err("isolation_level is only valid for sandbox components");
            }
            if *component != AgentComponentKindWire::McpTransport
                && capabilities.supports_notifications
            {
                return Err("notifications are only valid for MCP transport components");
            }
        }
        if let ExtensionDescriptor::ChannelPlugin(value) = self {
            if value.channel_id.trim().is_empty()
                || value.channel_id.chars().count() > 256
                || value.label.chars().count() > 256
                || value.handler_id.trim().is_empty()
                || value.handler_id.chars().count() > 256
            {
                return Err("channel plugin descriptor identity is empty or exceeds its bound");
            }
            if value.capabilities.chat_types.is_empty() {
                return Err("channel plugin descriptor must declare a chat type");
            }
        }
        if let ExtensionDescriptor::ChannelMessageHandler(value) = self
            && (value.handler_id.trim().is_empty() || value.handler_id.chars().count() > 256)
        {
            return Err("channel message handler descriptor identity is empty or bounded");
        }
        if let ExtensionDescriptor::Hook { events, .. } = self
            && (events.len() > 128
                || events
                    .iter()
                    .any(|event| echo_core::hooks::HookEvent::from_name(event).is_none()))
        {
            return Err("hook descriptor contains an unknown or excessive event name");
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

/// Inputs for one non-streaming Critic callback. The three strings mirror the
/// framework `Critic::critique` contract without exposing any Host-owned
/// runtime state or a second verifier authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CritiqueInput {
    #[schemars(length(max = 65_536))]
    pub task: String,
    #[schemars(length(max = 65_536))]
    pub answer: String,
    #[schemars(length(max = 65_536))]
    pub context: String,
}

impl CritiqueInput {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.task.chars().count() > 65_536
            || self.answer.chars().count() > 65_536
            || self.context.chars().count() > 65_536
        {
            return Err("critic input exceeds its text bound");
        }
        Ok(())
    }
}

/// Typed Critic callback result. `score` follows the framework's documented
/// 0..=10 scale and the remaining fields preserve the framework `Critique`
/// value losslessly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CritiqueWire {
    pub score: f64,
    pub passed: bool,
    #[schemars(length(max = 65_536))]
    pub feedback: String,
    #[serde(default)]
    #[schemars(length(max = 128))]
    pub suggestions: Vec<String>,
}

impl CritiqueWire {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !self.score.is_finite() || !(0.0..=10.0).contains(&self.score) {
            return Err("critic score must be finite and within 0..=10");
        }
        if self.feedback.chars().count() > 65_536 {
            return Err("critic feedback exceeds its text bound");
        }
        if self.suggestions.len() > 128
            || self
                .suggestions
                .iter()
                .any(|suggestion| suggestion.chars().count() > 4096)
        {
            return Err("critic suggestions are empty or exceed their bound");
        }
        Ok(())
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
    // Critic
    CriticCritique,
    // ChannelPlugin / MessageHandler
    ChannelStart,
    ChannelStop,
    ChannelSend,
    ChannelHealth,
    ChannelHandle,
    ChannelHandleStream,
    ChannelReply,
    CompressorCompress,
    AgentComponentCall,
    AgentComponentCallStream,
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
            ExtensionOperation::CriticCritique => "critic_critique",
            ExtensionOperation::ChannelStart => "channel_start",
            ExtensionOperation::ChannelStop => "channel_stop",
            ExtensionOperation::ChannelSend => "channel_send",
            ExtensionOperation::ChannelHealth => "channel_health",
            ExtensionOperation::ChannelHandle => "channel_handle",
            ExtensionOperation::ChannelHandleStream => "channel_handle_stream",
            ExtensionOperation::ChannelReply => "channel_reply",
            ExtensionOperation::CompressorCompress => "compressor_compress",
            ExtensionOperation::AgentComponentCall => "agent_component_call",
            ExtensionOperation::AgentComponentCallStream => "agent_component_call_stream",
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
            | ExtensionOperation::InterventionOnFinalAnswer => ExtensionKind::InterventionCallback,
            ExtensionOperation::FactoryCreateAgent => ExtensionKind::AgentFactory,
            ExtensionOperation::AgentExecute
            | ExtensionOperation::AgentExecuteStream
            | ExtensionOperation::AgentChat
            | ExtensionOperation::AgentChatStream
            | ExtensionOperation::AgentClose => ExtensionKind::CustomAgent,
            ExtensionOperation::CriticCritique => ExtensionKind::Critic,
            ExtensionOperation::ChannelStart
            | ExtensionOperation::ChannelStop
            | ExtensionOperation::ChannelSend
            | ExtensionOperation::ChannelHealth => ExtensionKind::ChannelPlugin,
            ExtensionOperation::ChannelHandle
            | ExtensionOperation::ChannelHandleStream
            | ExtensionOperation::ChannelReply => ExtensionKind::ChannelMessageHandler,
            ExtensionOperation::CompressorCompress => ExtensionKind::ContextCompressor,
            ExtensionOperation::AgentComponentCall
            | ExtensionOperation::AgentComponentCallStream => ExtensionKind::AgentComponent,
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
                | ExtensionOperation::ChannelHandleStream
                | ExtensionOperation::AgentComponentCallStream
        )
    }
}

/// Operation-discriminated reverse invocation payload. Every input shape is
/// named and available to schema/code generators.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(
    tag = "operation",
    content = "input",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ExtensionInvocation {
    ToolExecute(ToolExecuteInput),
    ToolExecuteStream(ToolExecuteInput),
    ToolValidateParameters(ToolValidateInput),
    LlmChat(LlmChatRequestWire),
    LlmChatStream(LlmChatRequestWire),
    StorePut(StorePutInput),
    StoreGet(StoreKeyInput),
    StoreSearch(StoreSearchInput),
    StoreSearchWith(StoreSearchWithInput),
    StoreDelete(StoreKeyInput),
    StoreListNamespaces(StoreListNamespacesInput),
    StoreList(StoreNamespaceInput),
    StorePruneExpired(StoreNamespaceInput),
    StoreDedupByContent(StoreNamespaceInput),
    HumanLoopRequest(HumanLoopRequestWire),
    HookRun(HookRunInput),
    CallbackOnThinkStart(CallbackThinkStartInput),
    CallbackOnThinkEnd(CallbackThinkEndInput),
    CallbackOnToolStart(CallbackToolStartInput),
    CallbackOnToolEnd(CallbackToolEndInput),
    CallbackOnToolError(CallbackToolErrorInput),
    CallbackOnFinalAnswer(CallbackFinalAnswerInput),
    CallbackOnIteration(CallbackIterationInput),
    InterventionOnToolCall(CallbackToolStartInput),
    InterventionOnThinkStart(CallbackThinkStartInput),
    InterventionOnFinalAnswer(CallbackFinalAnswerInput),
    FactoryCreateAgent(AgentFactoryConfigWire),
    AgentExecute(AgentTaskInput),
    AgentExecuteStream(AgentTaskInput),
    AgentChat(AgentMessageInput),
    AgentChatStream(AgentMessageInput),
    AgentClose(ExtensionUnit),
    CriticCritique(CritiqueInput),
    ChannelStart(ChannelStartInput),
    ChannelStop(ExtensionUnit),
    ChannelSend(ChannelSendInput),
    ChannelHealth(ExtensionUnit),
    ChannelHandle(ChannelHandleInput),
    ChannelHandleStream(ChannelHandleInput),
    ChannelReply(ChannelReplyInput),
    CompressorCompress(CompressionInputWire),
    AgentComponentCall(AgentComponentCallWire),
    AgentComponentCallStream(AgentComponentCallWire),
}

impl ExtensionInvocation {
    pub fn operation(&self) -> ExtensionOperation {
        match self {
            Self::ToolExecute(_) => ExtensionOperation::ToolExecute,
            Self::ToolExecuteStream(_) => ExtensionOperation::ToolExecuteStream,
            Self::ToolValidateParameters(_) => ExtensionOperation::ToolValidateParameters,
            Self::LlmChat(_) => ExtensionOperation::LlmChat,
            Self::LlmChatStream(_) => ExtensionOperation::LlmChatStream,
            Self::StorePut(_) => ExtensionOperation::StorePut,
            Self::StoreGet(_) => ExtensionOperation::StoreGet,
            Self::StoreSearch(_) => ExtensionOperation::StoreSearch,
            Self::StoreSearchWith(_) => ExtensionOperation::StoreSearchWith,
            Self::StoreDelete(_) => ExtensionOperation::StoreDelete,
            Self::StoreListNamespaces(_) => ExtensionOperation::StoreListNamespaces,
            Self::StoreList(_) => ExtensionOperation::StoreList,
            Self::StorePruneExpired(_) => ExtensionOperation::StorePruneExpired,
            Self::StoreDedupByContent(_) => ExtensionOperation::StoreDedupByContent,
            Self::HumanLoopRequest(_) => ExtensionOperation::HumanLoopRequest,
            Self::HookRun(_) => ExtensionOperation::HookRun,
            Self::CallbackOnThinkStart(_) => ExtensionOperation::CallbackOnThinkStart,
            Self::CallbackOnThinkEnd(_) => ExtensionOperation::CallbackOnThinkEnd,
            Self::CallbackOnToolStart(_) => ExtensionOperation::CallbackOnToolStart,
            Self::CallbackOnToolEnd(_) => ExtensionOperation::CallbackOnToolEnd,
            Self::CallbackOnToolError(_) => ExtensionOperation::CallbackOnToolError,
            Self::CallbackOnFinalAnswer(_) => ExtensionOperation::CallbackOnFinalAnswer,
            Self::CallbackOnIteration(_) => ExtensionOperation::CallbackOnIteration,
            Self::InterventionOnToolCall(_) => ExtensionOperation::InterventionOnToolCall,
            Self::InterventionOnThinkStart(_) => ExtensionOperation::InterventionOnThinkStart,
            Self::InterventionOnFinalAnswer(_) => ExtensionOperation::InterventionOnFinalAnswer,
            Self::FactoryCreateAgent(_) => ExtensionOperation::FactoryCreateAgent,
            Self::AgentExecute(_) => ExtensionOperation::AgentExecute,
            Self::AgentExecuteStream(_) => ExtensionOperation::AgentExecuteStream,
            Self::AgentChat(_) => ExtensionOperation::AgentChat,
            Self::AgentChatStream(_) => ExtensionOperation::AgentChatStream,
            Self::AgentClose(_) => ExtensionOperation::AgentClose,
            Self::CriticCritique(_) => ExtensionOperation::CriticCritique,
            Self::ChannelStart(_) => ExtensionOperation::ChannelStart,
            Self::ChannelStop(_) => ExtensionOperation::ChannelStop,
            Self::ChannelSend(_) => ExtensionOperation::ChannelSend,
            Self::ChannelHealth(_) => ExtensionOperation::ChannelHealth,
            Self::ChannelHandle(_) => ExtensionOperation::ChannelHandle,
            Self::ChannelHandleStream(_) => ExtensionOperation::ChannelHandleStream,
            Self::ChannelReply(_) => ExtensionOperation::ChannelReply,
            Self::CompressorCompress(_) => ExtensionOperation::CompressorCompress,
            Self::AgentComponentCall(_) => ExtensionOperation::AgentComponentCall,
            Self::AgentComponentCallStream(_) => ExtensionOperation::AgentComponentCallStream,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(
    tag = "operation",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ExtensionResult {
    ToolExecute(ToolResultWire),
    ToolValidateParameters(Option<String>),
    LlmChat(LlmChatResponseWire),
    StorePut(ExtensionUnit),
    StoreGet(Option<StoreItemWire>),
    StoreSearch(Vec<StoreItemWire>),
    StoreSearchWith(Vec<StoreItemWire>),
    StoreDelete(bool),
    StoreListNamespaces(Vec<Vec<String>>),
    StoreList(Vec<StoreItemWire>),
    StorePruneExpired(WireU64),
    StoreDedupByContent(WireU64),
    HumanLoopRequest(HumanLoopResponseWire),
    HookRun(HookResultWire),
    CallbackOnThinkStart(ExtensionUnit),
    CallbackOnThinkEnd(ExtensionUnit),
    CallbackOnToolStart(ExtensionUnit),
    CallbackOnToolEnd(ExtensionUnit),
    CallbackOnToolError(ExtensionUnit),
    CallbackOnFinalAnswer(ExtensionUnit),
    CallbackOnIteration(ExtensionUnit),
    InterventionOnToolCall(InterventionResultWire),
    InterventionOnThinkStart(InterventionResultWire),
    InterventionOnFinalAnswer(InterventionResultWire),
    FactoryCreateAgent(CustomAgentDescriptorWire),
    AgentExecute(String),
    AgentChat(String),
    AgentClose(ExtensionUnit),
    CriticCritique(CritiqueWire),
    ChannelStart(ExtensionUnit),
    ChannelStop(ExtensionUnit),
    ChannelSend(ExtensionUnit),
    ChannelHealth(ExtensionUnit),
    ChannelHandle(ChannelOutboundMessageWire),
    ChannelReply(ExtensionUnit),
    CompressorCompress(CompressionOutputWire),
    AgentComponentCall(AgentComponentResultWire),
}

impl ExtensionResult {
    pub fn operation(&self) -> ExtensionOperation {
        match self {
            Self::ToolExecute(_) => ExtensionOperation::ToolExecute,
            Self::ToolValidateParameters(_) => ExtensionOperation::ToolValidateParameters,
            Self::LlmChat(_) => ExtensionOperation::LlmChat,
            Self::StorePut(_) => ExtensionOperation::StorePut,
            Self::StoreGet(_) => ExtensionOperation::StoreGet,
            Self::StoreSearch(_) => ExtensionOperation::StoreSearch,
            Self::StoreSearchWith(_) => ExtensionOperation::StoreSearchWith,
            Self::StoreDelete(_) => ExtensionOperation::StoreDelete,
            Self::StoreListNamespaces(_) => ExtensionOperation::StoreListNamespaces,
            Self::StoreList(_) => ExtensionOperation::StoreList,
            Self::StorePruneExpired(_) => ExtensionOperation::StorePruneExpired,
            Self::StoreDedupByContent(_) => ExtensionOperation::StoreDedupByContent,
            Self::HumanLoopRequest(_) => ExtensionOperation::HumanLoopRequest,
            Self::HookRun(_) => ExtensionOperation::HookRun,
            Self::CallbackOnThinkStart(_) => ExtensionOperation::CallbackOnThinkStart,
            Self::CallbackOnThinkEnd(_) => ExtensionOperation::CallbackOnThinkEnd,
            Self::CallbackOnToolStart(_) => ExtensionOperation::CallbackOnToolStart,
            Self::CallbackOnToolEnd(_) => ExtensionOperation::CallbackOnToolEnd,
            Self::CallbackOnToolError(_) => ExtensionOperation::CallbackOnToolError,
            Self::CallbackOnFinalAnswer(_) => ExtensionOperation::CallbackOnFinalAnswer,
            Self::CallbackOnIteration(_) => ExtensionOperation::CallbackOnIteration,
            Self::InterventionOnToolCall(_) => ExtensionOperation::InterventionOnToolCall,
            Self::InterventionOnThinkStart(_) => ExtensionOperation::InterventionOnThinkStart,
            Self::InterventionOnFinalAnswer(_) => ExtensionOperation::InterventionOnFinalAnswer,
            Self::FactoryCreateAgent(_) => ExtensionOperation::FactoryCreateAgent,
            Self::AgentExecute(_) => ExtensionOperation::AgentExecute,
            Self::AgentChat(_) => ExtensionOperation::AgentChat,
            Self::AgentClose(_) => ExtensionOperation::AgentClose,
            Self::CriticCritique(_) => ExtensionOperation::CriticCritique,
            Self::ChannelStart(_) => ExtensionOperation::ChannelStart,
            Self::ChannelStop(_) => ExtensionOperation::ChannelStop,
            Self::ChannelSend(_) => ExtensionOperation::ChannelSend,
            Self::ChannelHealth(_) => ExtensionOperation::ChannelHealth,
            Self::ChannelHandle(_) => ExtensionOperation::ChannelHandle,
            Self::ChannelReply(_) => ExtensionOperation::ChannelReply,
            Self::CompressorCompress(_) => ExtensionOperation::CompressorCompress,
            Self::AgentComponentCall(_) => ExtensionOperation::AgentComponentCall,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SandboxOutputChannelWire {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub enum SandboxStreamChunkWire {
    Output {
        channel: SandboxOutputChannelWire,
        chunk: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SandboxStreamFailureWire {
    Cancelled { message: String },
    IoError { message: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "terminal", rename_all = "snake_case", deny_unknown_fields)]
pub enum SandboxStreamCompleteWire {
    Complete { result: WireValue },
    Failed { failure: SandboxStreamFailureWire },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowStreamChunkWire {
    NodeStart {
        node_name: String,
        step_index: WireU64,
    },
    NodeEnd {
        node_name: String,
        step_index: WireU64,
        elapsed: WireDuration,
    },
    Token {
        node_name: String,
        token: String,
    },
    NodeError {
        node_name: String,
        error: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowStreamCompleteWire {
    pub result: String,
    pub total_steps: WireU64,
    pub elapsed: WireDuration,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(
    tag = "component",
    content = "event",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum AgentComponentStreamChunkWire {
    Sandbox(SandboxStreamChunkWire),
    Workflow(WorkflowStreamChunkWire),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(
    tag = "component",
    content = "terminal",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum AgentComponentStreamCompleteWire {
    Sandbox(SandboxStreamCompleteWire),
    Workflow(WorkflowStreamCompleteWire),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
#[allow(clippy::large_enum_variant)]
pub enum ExtensionStreamChunkValue {
    Tool(ToolStreamChunkWire),
    Llm(LlmStreamChunkWire),
    Agent(AgentStreamChunkWire),
    Channel(ChannelOutboundMessageWire),
    AgentComponent(AgentComponentStreamChunkWire),
}

impl ExtensionStreamChunkValue {
    pub fn operation_kind(&self) -> ExtensionKind {
        match self {
            Self::Tool(_) => ExtensionKind::Tool,
            Self::Llm(_) => ExtensionKind::LlmClient,
            Self::Agent(_) => ExtensionKind::CustomAgent,
            Self::Channel(_) => ExtensionKind::ChannelMessageHandler,
            Self::AgentComponent(_) => ExtensionKind::AgentComponent,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
#[allow(clippy::large_enum_variant)]
pub enum ExtensionStreamCompleteValue {
    Tool(ToolResultWire),
    Llm(LlmStreamCompleteWire),
    Agent(AgentStreamTerminalWire),
    Channel(ChannelOutboundMessageWire),
    AgentComponent(AgentComponentStreamCompleteWire),
}

impl ExtensionStreamCompleteValue {
    pub fn operation_kind(&self) -> ExtensionKind {
        match self {
            Self::Tool(_) => ExtensionKind::Tool,
            Self::Llm(_) => ExtensionKind::LlmClient,
            Self::Agent(_) => ExtensionKind::CustomAgent,
            Self::Channel(_) => ExtensionKind::ChannelMessageHandler,
            Self::AgentComponent(_) => ExtensionKind::AgentComponent,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub enum ToolStreamChunkWire {
    Progress {
        message: String,
        percent: Option<u8>,
    },
    Output {
        channel: String,
        chunk: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct LlmStreamChunkWire {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_blocks: Option<Vec<LlmReasoningBlockWire>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<LlmDeltaToolCallWire>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<LlmUsageWire>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LlmStreamCompleteWire {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_blocks: Option<Vec<LlmReasoningBlockWire>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<LlmDeltaToolCallWire>>,
    #[schemars(length(min = 1, max = 256))]
    pub finish_reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<LlmUsageWire>,
}

/// Public typed projection of framework `AgentEvent` for CustomAgent streams.
/// Complex framework-owned leaves retain their own typed wire DTOs or the
/// closed `WireValue` algebra; the event discriminator and lifecycle fields
/// are never hidden behind an untagged JSON blob.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(
    tag = "event",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
#[allow(clippy::large_enum_variant)]
pub enum AgentEventWire {
    Token {
        text: String,
    },
    ThinkStart,
    ThinkEnd {
        prompt_tokens: WireU64,
        completion_tokens: WireU64,
    },
    LlmUsage {
        model: String,
        prompt_tokens: WireU64,
        completion_tokens: WireU64,
        total_tokens: WireU64,
        cached_prompt_tokens: WireU64,
        cache_creation_prompt_tokens: WireU64,
        usage_reported: bool,
    },
    BudgetDecision {
        decision: WireValue,
        reason: String,
        iteration: WireU64,
        reported_model_tokens: WireU64,
        usage_complete: bool,
    },
    ToolCall {
        call_id: String,
        invocation: WireValue,
    },
    ToolResult {
        call_id: String,
        name: String,
        result: ToolResultWire,
    },
    ToolStream {
        call_id: String,
        name: String,
        event: ToolStreamEventWire,
    },
    ToolBatchStart {
        tool_count: WireU64,
    },
    ToolBatchEnd,
    GuardTriggered {
        guard: String,
        blocked: bool,
    },
    MemoryRecalled {
        count: WireU64,
    },
    ContextCompressed {
        before_count: WireU64,
        after_count: WireU64,
        before_tokens: WireU64,
        after_tokens: WireU64,
    },
    Chart {
        spec: WireValue,
    },
    Error {
        source: String,
        message: String,
        failure: WireValue,
    },
    SafetyNotice {
        action: String,
        reason: String,
        risk: String,
        permission: String,
    },
    ParameterError {
        tool: String,
        parameter: String,
        expected: String,
        got: String,
    },
    FinalAnswer {
        text: String,
    },
    Cancelled,
}

/// Canonical `WireValue` type id for one [`AgentEventWire`] payload.
///
/// Source-operation adapters accept this closed type only.  A `Variant` uses
/// the enum variant name as its discriminator and stores the variant fields
/// directly; a `Record` uses an `event` string field and the same payload
/// fields (or a nested `data` record).  This keeps the source-operation
/// boundary typed without admitting an arbitrary JSON object.
pub const AGENT_EVENT_WIRE_TYPE_ID: &str = "echo_sdk_protocol::methods::AgentEventWire";

impl AgentEventWire {
    /// Decode the explicit `AgentEventWire` representation carried by a
    /// facade [`WireValue`].
    ///
    /// The conversion first validates the closed enum discriminator and then
    /// deserializes the already-typed fields into this DTO.  It never accepts
    /// a plain JSON map as an event payload, and typed `WireValue` fields are
    /// preserved as typed values instead of being flattened.
    pub fn from_wire_value(value: &WireValue) -> Result<Self, String> {
        let (variant, fields) = match value {
            WireValue::Variant {
                type_id,
                variant,
                fields,
            } if type_id == AGENT_EVENT_WIRE_TYPE_ID => (variant.clone(), fields.clone()),
            WireValue::Record { type_id, fields } if type_id == AGENT_EVENT_WIRE_TYPE_ID => {
                let event = fields
                    .iter()
                    .find(|field| field.name == "event")
                    .ok_or_else(|| "AgentEventWire record requires an event field".to_string())?;
                let WireValue::String(variant) = &event.value else {
                    return Err("AgentEventWire event field must be a string".to_string());
                };
                let nested = fields.iter().find(|field| field.name == "data");
                let mut payload = fields
                    .iter()
                    .filter(|field| field.name != "event" && field.name != "data")
                    .cloned()
                    .collect::<Vec<_>>();
                if let Some(data) = nested {
                    let data_fields = match &data.value {
                        WireValue::Record { fields, .. } | WireValue::Variant { fields, .. } => {
                            fields.clone()
                        }
                        WireValue::Map(entries) => entries
                            .iter()
                            .map(|entry| {
                                let WireValue::String(name) = &entry.key else {
                                    return Err("AgentEventWire record data keys must be strings"
                                        .to_string());
                                };
                                Ok(WireField {
                                    name: name.clone(),
                                    value: entry.value.clone(),
                                })
                            })
                            .collect::<Result<Vec<_>, String>>()?,
                        _ => {
                            return Err(
                                "AgentEventWire record data must be a record, variant, or map"
                                    .to_string(),
                            );
                        }
                    };
                    payload.extend(data_fields);
                }
                (variant.clone(), payload)
            }
            _ => {
                return Err(format!(
                    "expected WireValue variant or record with type_id {AGENT_EVENT_WIRE_TYPE_ID}"
                ));
            }
        };

        let known_variant = matches!(
            variant.as_str(),
            "token"
                | "think_start"
                | "think_end"
                | "llm_usage"
                | "budget_decision"
                | "tool_call"
                | "tool_result"
                | "tool_stream"
                | "tool_batch_start"
                | "tool_batch_end"
                | "guard_triggered"
                | "memory_recalled"
                | "context_compressed"
                | "chart"
                | "error"
                | "safety_notice"
                | "parameter_error"
                | "final_answer"
                | "cancelled"
        );
        if !known_variant {
            return Err(format!("unknown AgentEventWire variant {variant}"));
        }

        let mut data = serde_json::Map::new();
        for field in payload_fields(&variant, &fields)? {
            let value = if is_wire_value_field(&variant, &field.name) {
                serde_json::to_value(&field.value).map_err(|error| {
                    format!("event field {} is not serializable: {error}", field.name)
                })?
            } else {
                wire_value_to_json(&field.value)?
            };
            if data.insert(field.name.clone(), value).is_some() {
                return Err(format!("duplicate AgentEventWire field {}", field.name));
            }
        }
        let mut object = serde_json::Map::new();
        object.insert("event".to_string(), serde_json::Value::String(variant));
        if !data.is_empty() {
            object.insert("data".to_string(), serde_json::Value::Object(data));
        }
        serde_json::from_value(serde_json::Value::Object(object))
            .map_err(|error| format!("malformed AgentEventWire payload: {error}"))
    }
}

fn payload_fields<'a>(variant: &str, fields: &'a [WireField]) -> Result<&'a [WireField], String> {
    // The enum deserializer below performs the exact field/type validation;
    // this helper only rejects fields that cannot belong to the selected
    // variant before any object projection occurs.
    let allowed = match variant {
        "token" => &["text"][..],
        "think_start" | "tool_batch_end" | "cancelled" => &[][..],
        "think_end" => &["prompt_tokens", "completion_tokens"][..],
        "llm_usage" => &[
            "model",
            "prompt_tokens",
            "completion_tokens",
            "total_tokens",
            "cached_prompt_tokens",
            "cache_creation_prompt_tokens",
            "usage_reported",
        ][..],
        "budget_decision" => &[
            "decision",
            "reason",
            "iteration",
            "reported_model_tokens",
            "usage_complete",
        ][..],
        "tool_call" => &["call_id", "invocation"][..],
        "tool_result" => &["call_id", "name", "result"][..],
        "tool_stream" => &["call_id", "name", "event"][..],
        "tool_batch_start" => &["tool_count"][..],
        "guard_triggered" => &["guard", "blocked"][..],
        "memory_recalled" => &["count"][..],
        "context_compressed" => &[
            "before_count",
            "after_count",
            "before_tokens",
            "after_tokens",
        ][..],
        "chart" => &["spec"][..],
        "error" => &["source", "message", "failure"][..],
        "safety_notice" => &["action", "reason", "risk", "permission"][..],
        "parameter_error" => &["tool", "parameter", "expected", "got"][..],
        "final_answer" => &["text"][..],
        _ => return Err(format!("unknown AgentEventWire variant {variant}")),
    };
    if let Some(field) = fields
        .iter()
        .find(|field| !allowed.contains(&field.name.as_str()))
    {
        return Err(format!(
            "field {} is not valid for AgentEventWire variant {variant}",
            field.name
        ));
    }
    Ok(fields)
}

fn is_wire_value_field(variant: &str, field: &str) -> bool {
    matches!(
        (variant, field),
        ("budget_decision", "decision")
            | ("tool_call", "invocation")
            | ("chart", "spec")
            | ("error", "failure")
    )
}

fn wire_value_to_json(value: &WireValue) -> Result<serde_json::Value, String> {
    match value {
        WireValue::Null => Ok(serde_json::Value::Null),
        WireValue::Bool(value) => Ok(serde_json::Value::Bool(*value)),
        WireValue::String(value) => Ok(serde_json::Value::String(value.clone())),
        WireValue::I64(value) => Ok(serde_json::Value::String(value.as_str().to_string())),
        WireValue::U64(value) => Ok(serde_json::Value::String(value.as_str().to_string())),
        WireValue::F64(value) if value.is_finite() => serde_json::Number::from_f64(*value)
            .map(serde_json::Value::Number)
            .ok_or_else(|| "non-finite AgentEventWire number".to_string()),
        WireValue::F64(_) => Err("non-finite AgentEventWire number".to_string()),
        WireValue::Bytes(value) => serde_json::to_value(value).map_err(|error| error.to_string()),
        WireValue::Duration(value) => {
            serde_json::to_value(value).map_err(|error| error.to_string())
        }
        WireValue::Timestamp(value) => {
            serde_json::to_value(value).map_err(|error| error.to_string())
        }
        WireValue::Path(value) => serde_json::to_value(value).map_err(|error| error.to_string()),
        WireValue::Handle(value) => serde_json::to_value(value).map_err(|error| error.to_string()),
        WireValue::List(values) => values
            .iter()
            .map(wire_value_to_json)
            .collect::<Result<Vec<_>, _>>()
            .map(serde_json::Value::Array),
        WireValue::Map(entries) => {
            let mut object = serde_json::Map::new();
            for entry in entries {
                let WireValue::String(key) = &entry.key else {
                    return Err("AgentEventWire map keys must be strings".to_string());
                };
                if object
                    .insert(key.clone(), wire_value_to_json(&entry.value)?)
                    .is_some()
                {
                    return Err(format!("duplicate AgentEventWire map key {key}"));
                }
            }
            Ok(serde_json::Value::Object(object))
        }
        WireValue::Record { fields, .. } => {
            let mut object = serde_json::Map::new();
            for field in fields {
                if object
                    .insert(field.name.clone(), wire_value_to_json(&field.value)?)
                    .is_some()
                {
                    return Err(format!(
                        "duplicate AgentEventWire record field {}",
                        field.name
                    ));
                }
            }
            Ok(serde_json::Value::Object(object))
        }
        WireValue::Variant {
            variant, fields, ..
        } => {
            let mut data = serde_json::Map::new();
            for field in fields {
                if data
                    .insert(field.name.clone(), wire_value_to_json(&field.value)?)
                    .is_some()
                {
                    return Err(format!(
                        "duplicate AgentEventWire variant field {}",
                        field.name
                    ));
                }
            }
            let mut object = serde_json::Map::new();
            object.insert(
                "event".to_string(),
                serde_json::Value::String(variant.clone()),
            );
            if !data.is_empty() {
                object.insert("data".to_string(), serde_json::Value::Object(data));
            }
            Ok(serde_json::Value::Object(object))
        }
        WireValue::Unknown { .. } => serde_json::to_value(value).map_err(|error| error.to_string()),
    }
}

/// Non-terminal CustomAgent stream event. Terminal variants intentionally do
/// not appear here, so generated SDKs cannot emit a framework terminal as an
/// ordinary chunk.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(
    tag = "event",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
#[allow(clippy::large_enum_variant)]
pub enum AgentStreamChunkWire {
    Token {
        text: String,
    },
    ThinkStart,
    ThinkEnd {
        prompt_tokens: WireU64,
        completion_tokens: WireU64,
    },
    LlmUsage {
        model: String,
        prompt_tokens: WireU64,
        completion_tokens: WireU64,
        total_tokens: WireU64,
        cached_prompt_tokens: WireU64,
        cache_creation_prompt_tokens: WireU64,
        usage_reported: bool,
    },
    BudgetDecision {
        decision: WireValue,
        reason: String,
        iteration: WireU64,
        reported_model_tokens: WireU64,
        usage_complete: bool,
    },
    ToolCall {
        call_id: String,
        invocation: WireValue,
    },
    ToolResult {
        call_id: String,
        name: String,
        result: ToolResultWire,
    },
    ToolStream {
        call_id: String,
        name: String,
        event: ToolStreamChunkWire,
    },
    ToolBatchStart {
        tool_count: WireU64,
    },
    ToolBatchEnd,
    GuardTriggered {
        guard: String,
        blocked: bool,
    },
    MemoryRecalled {
        count: WireU64,
    },
    ContextCompressed {
        before_count: WireU64,
        after_count: WireU64,
        before_tokens: WireU64,
        after_tokens: WireU64,
    },
    Chart {
        spec: WireValue,
    },
    SafetyNotice {
        action: String,
        reason: String,
        risk: String,
        permission: String,
    },
    ParameterError {
        tool: String,
        parameter: String,
        expected: String,
        got: String,
    },
}

/// Terminal CustomAgent stream event. The outer stream `complete` variant
/// accepts only these framework terminal facts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(
    tag = "event",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum AgentStreamTerminalWire {
    Error {
        source: String,
        message: String,
        failure: WireValue,
    },
    FinalAnswer {
        text: String,
    },
    Cancelled,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 256))]
    pub turn_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 256))]
    pub message_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 256))]
    pub execution_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 256))]
    pub call_id: Option<String>,
}

impl ExtensionInvocationContext {
    pub fn validate(&self) -> Result<(), &'static str> {
        for (name, value) in [
            ("session_id", &self.session_id),
            ("run_id", &self.run_id),
            ("stream_id", &self.stream_id),
            ("turn_id", &self.turn_id),
            ("message_id", &self.message_id),
            ("execution_id", &self.execution_id),
            ("call_id", &self.call_id),
        ] {
            if value
                .as_deref()
                .is_some_and(|id| id.trim().is_empty() || id.chars().count() > 256)
            {
                return Err(match name {
                    "session_id" => "session_id is empty or exceeds its bound",
                    "run_id" => "run_id is empty or exceeds its bound",
                    "stream_id" => "stream_id is empty or exceeds its bound",
                    "turn_id" => "turn_id is empty or exceeds its bound",
                    "message_id" => "message_id is empty or exceeds its bound",
                    "execution_id" => "execution_id is empty or exceeds its bound",
                    "call_id" => "call_id is empty or exceeds its bound",
                    _ => "context identity is empty or exceeds its bound",
                });
            }
        }
        Ok(())
    }
}

/// `_echo_agent/extension/register` request: register a host-language
/// implementation of a public framework trait (Tool, LlmClient, Store,
/// HumanLoopProvider, Hook, AgentCallback, InterventionCallback,
/// AgentFactory, Critic, custom Agent). Registration is owned by the current
/// connection generation: it never survives a Host restart or a reconnect.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema, JsonRpcRequest)]
#[request(method = "_echo_agent/extension/register", response = ExtensionRegisterResponse)]
#[serde(deny_unknown_fields)]
pub struct ExtensionRegisterRequest {
    /// Which extension point is implemented.
    pub kind: ExtensionKind,
    /// Client-side implementation identity (non-empty). Re-registering the
    /// same identity with the same descriptor and default timeout returns the
    /// same handle; a different registration snapshot is a typed conflict.
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
        if self.implementation_id.trim().is_empty()
            || self.implementation_id.chars().count() > MAX_EXTENSION_IMPLEMENTATION_ID_CHARS
        {
            return Err("implementation_id must be non-empty and bounded");
        }
        if self.descriptor.kind() != self.kind {
            return Err("descriptor kind does not match the registration kind");
        }
        self.descriptor.validate()?;
        let encoded =
            serde_json::to_vec(&self.descriptor).map_err(|_| "descriptor is not encodable")?;
        if encoded.len() > MAX_EXTENSION_DESCRIPTOR_BYTES {
            return Err("descriptor exceeds the serialized descriptor bound");
        }
        if let Some(timeout) = &self.timeout {
            timeout
                .validate()
                .map_err(|_| "registration timeout is out of range")?;
            if timeout.seconds.to_u64() == Some(0) && timeout.nanos == 0 {
                return Err("registration timeout must be positive");
            }
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema, JsonRpcRequest)]
#[request(method = "_echo_agent/extension/invoke", response = ExtensionInvokeOutcome)]
#[serde(deny_unknown_fields)]
pub struct ExtensionInvokeCall {
    pub extension: WireHandle,
    /// Invocation identity; unique per call, used for cancellation. It is a
    /// domain identity independent from any JSON-RPC request id.
    #[schemars(length(min = 1, max = 256))]
    pub invocation_id: String,
    /// Session/run correlation identity (diagnostic only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<ExtensionInvocationContext>,
    /// Operation-discriminated input. Keeping it as a named field makes the
    /// generated call schema reference `ExtensionInvocation` directly, so
    /// every language SDK can reuse one stable dispatcher union.
    pub invocation: ExtensionInvocation,
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
        if let Some(context) = &self.context {
            context.validate()?;
        }
        let operation = self.invocation.operation();
        if !operation.is_streaming() && self.stream.is_some() {
            return Err("non-streaming operations must not carry a stream handle");
        }
        if operation.is_streaming() {
            let Some(stream) = &self.stream else {
                return Err("streaming operations require a stream handle");
            };
            stream.validate().map_err(|_| "invalid stream handle")?;
            if stream.kind != HandleKind::Stream {
                return Err("streaming operations require a stream-kind handle");
            }
        }
        let encoded =
            serde_json::to_vec(&self.invocation).map_err(|_| "payload is not encodable")?;
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
#[allow(clippy::large_enum_variant)]
pub enum ExtensionInvokeOutcome {
    Result {
        result: ExtensionResult,
    },
    /// Streaming acknowledgement: the SDK echoes the Host-minted stream
    /// handle and delivers the payload through
    /// `_echo_agent/extension/stream` notifications.
    Stream {
        stream: WireHandle,
    },
    Error {
        error: EchoSdkError,
    },
}

impl ExtensionInvokeOutcome {
    pub fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::Result { result } => {
                let encoded =
                    serde_json::to_vec(result).map_err(|_| "callback result is not encodable")?;
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
/// contiguous from one per stream and exactly one terminal variant may be emitted by
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
        value: ExtensionStreamChunkValue,
    },
    Complete {
        stream: WireHandle,
        sequence: WireNonZeroU64,
        value: ExtensionStreamCompleteValue,
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
            Self::Chunk { value, .. } => {
                let encoded =
                    serde_json::to_vec(value).map_err(|_| "stream value is not encodable")?;
                if encoded.len() > MAX_EXTENSION_STREAM_CHUNK_BYTES {
                    return Err("stream value exceeds the chunk bound");
                }
            }
            Self::Complete { value, .. } => {
                if let ExtensionStreamCompleteValue::Llm(value) = value
                    && (value.finish_reason.trim().is_empty()
                        || value.finish_reason.chars().count() > 256)
                {
                    return Err("LLM stream terminal requires a bounded finish reason");
                }
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
        if self.operation.contains('*') {
            return Err("facade operation must be an exact identity, not a wildcard");
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

#[cfg(test)]
mod agent_event_wire_tests {
    use super::*;

    fn field(name: &str, value: WireValue) -> WireField {
        WireField {
            name: name.to_string(),
            value,
        }
    }

    #[test]
    fn explicit_variant_decodes_final_answer() {
        let value = WireValue::Variant {
            type_id: AGENT_EVENT_WIRE_TYPE_ID.to_string(),
            variant: "final_answer".to_string(),
            fields: vec![field("text", WireValue::String("done".to_string()))],
        };
        assert_eq!(
            AgentEventWire::from_wire_value(&value),
            Ok(AgentEventWire::FinalAnswer {
                text: "done".to_string()
            })
        );
    }

    #[test]
    fn explicit_record_decodes_cancelled() {
        let value = WireValue::Record {
            type_id: AGENT_EVENT_WIRE_TYPE_ID.to_string(),
            fields: vec![field("event", WireValue::String("cancelled".to_string()))],
        };
        assert_eq!(
            AgentEventWire::from_wire_value(&value),
            Ok(AgentEventWire::Cancelled)
        );
    }

    #[test]
    fn malformed_type_or_field_is_rejected() {
        let wrong_type = WireValue::Variant {
            type_id: "serde_json::Value".to_string(),
            variant: "final_answer".to_string(),
            fields: vec![field("text", WireValue::String("done".to_string()))],
        };
        assert!(AgentEventWire::from_wire_value(&wrong_type).is_err());

        let unknown_field = WireValue::Variant {
            type_id: AGENT_EVENT_WIRE_TYPE_ID.to_string(),
            variant: "cancelled".to_string(),
            fields: vec![field("text", WireValue::String("unexpected".to_string()))],
        };
        assert!(AgentEventWire::from_wire_value(&unknown_field).is_err());
    }
}
