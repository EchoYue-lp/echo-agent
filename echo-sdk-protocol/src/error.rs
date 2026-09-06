//! Extension error contract.
//!
//! Standard ACP methods keep returning standard JSON-RPC/ACP errors and stop
//! reasons exactly as the official schema defines them. `_echo_agent/*`
//! methods additionally use the typed error envelope here (design §10.6):
//! a stable code, a human message, retryability, optional operation/handle
//! identity and a bounded details bag. Language SDKs map codes to typed
//! exceptions; they must never parse standard ACP message text to guess
//! framework state.

use serde::{Deserialize, Serialize};

use crate::handle::WireHandle;

/// Stable extension error codes. Codes are closed: reusing a retired code for
/// a different meaning is a breaking protocol change (design §18).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionErrorCode {
    // ── ACP / extension capability mismatches ─────────────────────────────
    /// Peer does not speak a compatible ACP wire protocol version.
    AcpProtocolMismatch,
    /// Peer's `_echo_agent` extension protocol version is incompatible.
    ExtensionVersionMismatch,
    /// Extension contract/source digest mismatch.
    ExtensionDigestMismatch,
    /// Required extension capability missing or unsupported.
    ExtensionCapabilityMismatch,
    // ── Invalid input ──────────────────────────────────────────────────────
    /// Malformed request (structure, method usage, ordering).
    InvalidRequest,
    /// Semantically invalid configuration.
    InvalidConfig,
    /// Semantically invalid value (scalars, paths, payload shapes).
    InvalidValue,
    /// Operation requires a feature the Host was not compiled with.
    FeatureUnavailable,
    // ── Handle lifecycle ───────────────────────────────────────────────────
    /// Handle generation no longer matches the live object.
    StaleHandle,
    /// Handle refers to an object that has been closed/destroyed.
    ClosedHandle,
    // ── Framework failures ─────────────────────────────────────────────────
    /// Framework returned a typed domain error.
    FrameworkError,
    // ── Extension bridge failures ──────────────────────────────────────────
    /// Registered extension rejected the invocation.
    ExtensionRejected,
    /// Registered extension failed while executing.
    ExtensionFailed,
    /// Extension invocation exceeded its deadline.
    ExtensionTimeout,
    /// Extension's SDK connection is gone.
    ExtensionDisconnected,
    // ── Cancellation / lifecycle ───────────────────────────────────────────
    /// Operation was cancelled; framework terminal state remains authoritative.
    Cancelled,
    /// Host is shutting down and refuses new work.
    HostShuttingDown,
    /// Host process exited before the operation settled.
    HostExited,
    // ── Event streaming ────────────────────────────────────────────────────
    /// Consumer fell below the retained watermark.
    EventGap,
    /// Replay is unavailable for the requested window.
    ReplayUnavailable,
    // ── Transport bounds ───────────────────────────────────────────────────
    /// Payload exceeds the negotiated bound.
    PayloadTooLarge,
    /// Payload violates the extension serialization contract.
    SerializationViolation,
}

impl ExtensionErrorCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            ExtensionErrorCode::AcpProtocolMismatch => "acp_protocol_mismatch",
            ExtensionErrorCode::ExtensionVersionMismatch => "extension_version_mismatch",
            ExtensionErrorCode::ExtensionDigestMismatch => "extension_digest_mismatch",
            ExtensionErrorCode::ExtensionCapabilityMismatch => "extension_capability_mismatch",
            ExtensionErrorCode::InvalidRequest => "invalid_request",
            ExtensionErrorCode::InvalidConfig => "invalid_config",
            ExtensionErrorCode::InvalidValue => "invalid_value",
            ExtensionErrorCode::FeatureUnavailable => "feature_unavailable",
            ExtensionErrorCode::StaleHandle => "stale_handle",
            ExtensionErrorCode::ClosedHandle => "closed_handle",
            ExtensionErrorCode::FrameworkError => "framework_error",
            ExtensionErrorCode::ExtensionRejected => "extension_rejected",
            ExtensionErrorCode::ExtensionFailed => "extension_failed",
            ExtensionErrorCode::ExtensionTimeout => "extension_timeout",
            ExtensionErrorCode::ExtensionDisconnected => "extension_disconnected",
            ExtensionErrorCode::Cancelled => "cancelled",
            ExtensionErrorCode::HostShuttingDown => "host_shutting_down",
            ExtensionErrorCode::HostExited => "host_exited",
            ExtensionErrorCode::EventGap => "event_gap",
            ExtensionErrorCode::ReplayUnavailable => "replay_unavailable",
            ExtensionErrorCode::PayloadTooLarge => "payload_too_large",
            ExtensionErrorCode::SerializationViolation => "serialization_violation",
        }
    }
}

/// Whether an operation may be retried and when (design §10.6). Only the
/// framework's own operation contract decides to actually retry; this field
/// tells language SDKs what is *safe* to retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Retryability {
    Never,
    Always,
    AfterDelay,
}

/// Bounded details attached to an extension error. The bound (entry count and
/// per-entry size) is enforced by the Host before emission; secrets never
/// enter error details (design §16).
pub const MAX_ERROR_DETAIL_ENTRIES: usize = 16;
pub const MAX_ERROR_MESSAGE_CHARS: usize = 4096;
pub const MAX_ERROR_OPERATION_CHARS: usize = 256;
pub const MAX_ERROR_DETAIL_KEY_CHARS: usize = 128;
pub const MAX_ERROR_DETAIL_VALUE_CHARS: usize = 2048;

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ErrorDetails {
    /// Bounded key/value facts; values are strings, never raw payloads.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(length(max = 16))]
    pub fields: Option<Vec<DetailField>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DetailField {
    #[schemars(length(max = 128))]
    pub key: String,
    #[schemars(length(max = 2048))]
    pub value: String,
}

/// The typed `_echo_agent/*` error envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EchoSdkError {
    pub code: ExtensionErrorCode,
    #[schemars(length(max = 4096))]
    pub message: String,
    pub retryable: Retryability,
    /// Domain operation identity (e.g. `run/start`), independent from any
    /// JSON-RPC request id.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(length(max = 256))]
    pub operation: Option<String>,
    /// Handle the error refers to, when applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handle: Option<WireHandle>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<ErrorDetails>,
}

impl EchoSdkError {
    pub fn new(
        code: ExtensionErrorCode,
        message: impl Into<String>,
        retryable: Retryability,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            retryable,
            operation: None,
            handle: None,
            details: None,
        }
    }

    /// Attach the domain operation identity (e.g. `run/start`).
    pub fn with_operation(mut self, operation: impl Into<String>) -> Self {
        self.operation = Some(operation.into());
        self
    }

    /// Attach the handle the error refers to.
    pub fn with_handle(mut self, handle: WireHandle) -> Self {
        self.handle = Some(handle);
        self
    }

    /// Validate the bounded shape before emission/parsing.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.message.chars().count() > MAX_ERROR_MESSAGE_CHARS {
            return Err("error message exceeds the character bound");
        }
        if self
            .operation
            .as_ref()
            .is_some_and(|operation| operation.trim().is_empty())
        {
            return Err("error operation must be non-empty when present");
        }
        if self
            .operation
            .as_ref()
            .is_some_and(|operation| operation.chars().count() > MAX_ERROR_OPERATION_CHARS)
        {
            return Err("error operation exceeds the character bound");
        }
        let over_bound = self
            .details
            .as_ref()
            .and_then(|d| d.fields.as_ref())
            .is_some_and(|fields| fields.len() > MAX_ERROR_DETAIL_ENTRIES);
        if over_bound {
            return Err("error details exceed the bounded entry count");
        }
        let mut keys = std::collections::BTreeSet::new();
        for field in self
            .details
            .as_ref()
            .and_then(|details| details.fields.as_ref())
            .into_iter()
            .flatten()
        {
            if field.key.trim().is_empty() {
                return Err("error detail key must be non-empty");
            }
            if field.key.chars().count() > MAX_ERROR_DETAIL_KEY_CHARS {
                return Err("error detail key exceeds the character bound");
            }
            if field.value.chars().count() > MAX_ERROR_DETAIL_VALUE_CHARS {
                return Err("error detail value exceeds the character bound");
            }
            if !keys.insert(field.key.as_str()) {
                return Err("error detail keys must be unique");
            }
        }
        if let Some(handle) = &self.handle {
            handle.validate()?;
        }
        Ok(())
    }
}

/// The single fixed JSON-RPC server-error code used by every semantic
/// `_echo_agent/*` failure (design §10.6). It sits in the JSON-RPC reserved
/// server-error range (-32000..-32099) so it can never collide with the
/// standard ACP codes the official runtime returns (-32700..-32603,
/// -32800, -32002). `error.data` carries the bounded [`EchoSdkError`];
/// malformed typed params and unknown methods keep returning the standard
/// parse/invalid-params/method-not-found errors instead.
pub const EXTENSION_ERROR_CODE: i32 = -32050;

/// Maximum serialized bytes accepted inside `error.data`; larger encodings
/// fail closed as `serialization_violation` instead of leaking unbounded
/// payloads.
pub const MAX_ERROR_DATA_BYTES: usize = 8192;

impl EchoSdkError {
    /// Encode this typed error as the extension JSON-RPC error response
    /// value: fixed [`EXTENSION_ERROR_CODE`], bounded message, and
    /// `data` = the bounds-validated `EchoSdkError`. Violating shapes never
    /// reach the wire: they are replaced by a bounded
    /// `serialization_violation` error so callers cannot accidentally emit
    /// unvalidated payloads.
    pub fn into_jsonrpc_error(self) -> agent_client_protocol::Error {
        if let Err(reason) = self.validate() {
            let fallback = EchoSdkError::new(
                ExtensionErrorCode::SerializationViolation,
                format!("bounded error encoding rejected the payload: {reason}"),
                Retryability::Never,
            );
            return fallback.encode_bounded();
        }
        self.encode_bounded()
    }

    /// Decode a typed error from an extension JSON-RPC error `data` value.
    /// Fails closed when the payload is missing, malformed, or out of
    /// bounds — callers must not fabricate defaults for protocol errors.
    pub fn from_jsonrpc_data(data: Option<&serde_json::Value>) -> Result<Self, String> {
        let data = data.ok_or_else(|| "extension error is missing error.data".to_string())?;
        let serialized = serde_json::to_vec(data)
            .map_err(|error| format!("extension error data is not JSON: {error}"))?;
        if serialized.len() > MAX_ERROR_DATA_BYTES {
            return Err("extension error data exceeds the byte bound".to_string());
        }
        let error: Self = serde_json::from_value(data.clone())
            .map_err(|error| format!("malformed echo-agent error data: {error}"))?;
        error
            .validate()
            .map_err(|reason| format!("invalid echo-agent error data: {reason}"))?;
        Ok(error)
    }

    fn encode_bounded(self) -> agent_client_protocol::Error {
        let mut error =
            agent_client_protocol::Error::new(EXTENSION_ERROR_CODE, bounded_message(&self.message));
        let data = match serde_json::to_value(&self) {
            Ok(data) => match serde_json::to_vec(&data) {
                Ok(bytes) if bytes.len() <= MAX_ERROR_DATA_BYTES => data,
                _ => serde_json::json!({
                    "code": ExtensionErrorCode::SerializationViolation,
                    "message": "typed error data exceeded the byte bound",
                    "retryable": Retryability::Never,
                }),
            },
            Err(_) => serde_json::json!({
                "code": ExtensionErrorCode::SerializationViolation,
                "message": "typed error data was not serializable",
                "retryable": Retryability::Never,
            }),
        };
        error = error.data(data);
        error
    }
}

fn bounded_message(message: &str) -> String {
    message.chars().take(MAX_ERROR_MESSAGE_CHARS).collect()
}

/// Maximum characters accepted in one framework failure message projected
/// on the wire; longer framework messages are truncated UTF-8 safely.
pub const MAX_FAILURE_MESSAGE_CHARS: usize = 4096;

/// Lossless wire projection of the framework failure contract
/// (`echo_core::error::AgentFailure`). Categories and terminal kinds are
/// serialized as their framework snake_case names so a later framework
/// variant remains observable instead of collapsing into a generic code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentFailureWire {
    #[schemars(length(min = 1, max = 64))]
    pub category: String,
    #[schemars(length(min = 1, max = 64))]
    pub terminal_kind: String,
    pub retryable: bool,
    #[schemars(length(min = 1, max = 128))]
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    #[schemars(length(min = 1, max = 4096))]
    pub message: String,
}

impl AgentFailureWire {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.message.chars().count() > MAX_FAILURE_MESSAGE_CHARS {
            return Err("failure message exceeds the character bound");
        }
        Ok(())
    }
}

impl From<&echo_core::error::AgentFailure> for AgentFailureWire {
    fn from(failure: &echo_core::error::AgentFailure) -> Self {
        fn serde_name(value: &impl serde::Serialize) -> String {
            serde_json::to_value(value)
                .ok()
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_default()
        }
        Self {
            category: serde_name(&failure.category),
            terminal_kind: serde_name(&failure.terminal_kind),
            retryable: failure.retryable,
            code: failure.code.chars().take(128).collect(),
            http_status: failure.http_status,
            message: failure
                .message
                .chars()
                .take(MAX_FAILURE_MESSAGE_CHARS)
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handle::HandleKind;
    use crate::scalar::WireU64;

    #[test]
    fn codes_are_snake_case_and_unique() {
        let all = [
            ExtensionErrorCode::AcpProtocolMismatch,
            ExtensionErrorCode::ExtensionVersionMismatch,
            ExtensionErrorCode::ExtensionDigestMismatch,
            ExtensionErrorCode::ExtensionCapabilityMismatch,
            ExtensionErrorCode::InvalidRequest,
            ExtensionErrorCode::InvalidConfig,
            ExtensionErrorCode::InvalidValue,
            ExtensionErrorCode::FeatureUnavailable,
            ExtensionErrorCode::StaleHandle,
            ExtensionErrorCode::ClosedHandle,
            ExtensionErrorCode::FrameworkError,
            ExtensionErrorCode::ExtensionRejected,
            ExtensionErrorCode::ExtensionFailed,
            ExtensionErrorCode::ExtensionTimeout,
            ExtensionErrorCode::ExtensionDisconnected,
            ExtensionErrorCode::Cancelled,
            ExtensionErrorCode::HostShuttingDown,
            ExtensionErrorCode::HostExited,
            ExtensionErrorCode::EventGap,
            ExtensionErrorCode::ReplayUnavailable,
            ExtensionErrorCode::PayloadTooLarge,
            ExtensionErrorCode::SerializationViolation,
        ];
        let mut seen: Vec<&str> = Vec::new();
        for code in all {
            let s = code.as_str();
            assert!(s.chars().all(|c| c.is_ascii_lowercase() || c == '_'));
            assert!(!seen.contains(&s), "duplicate code {s}");
            seen.push(s);
        }
    }

    #[test]
    fn error_envelope_round_trip_with_handle() {
        let error = EchoSdkError {
            code: ExtensionErrorCode::StaleHandle,
            message: "run handle predates host restart".to_string(),
            retryable: Retryability::Never,
            operation: Some("run/start".to_string()),
            handle: Some(WireHandle {
                id: "run-9".to_string(),
                generation: WireU64::from_u64(1),
                kind: HandleKind::Run,
            }),
            details: None,
        };
        let json = serde_json::to_string(&error).unwrap_or_default();
        let back: EchoSdkError = serde_json::from_str(&json).unwrap_or_else(|_| {
            EchoSdkError::new(ExtensionErrorCode::InvalidValue, "x", Retryability::Never)
        });
        assert_eq!(back.code.as_str(), "stale_handle");
        assert!(back.handle.is_some());
        assert!(back.validate().is_ok());
    }

    #[test]
    fn jsonrpc_error_codec_round_trips_and_fails_closed() {
        let error = EchoSdkError::new(
            ExtensionErrorCode::ClosedHandle,
            "session was closed",
            Retryability::Never,
        )
        .with_operation("run/start");
        let rpc = error.clone().into_jsonrpc_error();
        assert_eq!(
            rpc.code,
            agent_client_protocol::ErrorCode::Other(EXTENSION_ERROR_CODE)
        );
        let decoded =
            EchoSdkError::from_jsonrpc_data(rpc.data.as_ref()).unwrap_or_else(|message| {
                EchoSdkError::new(
                    ExtensionErrorCode::SerializationViolation,
                    message,
                    Retryability::Never,
                )
            });
        assert_eq!(decoded, error);

        // Missing / malformed / out-of-bounds data must fail closed.
        assert!(EchoSdkError::from_jsonrpc_data(None).is_err());
        assert!(
            EchoSdkError::from_jsonrpc_data(Some(&serde_json::Value::Null)).is_err(),
            "null data is not a typed error"
        );
        let mut oversized =
            EchoSdkError::new(ExtensionErrorCode::InvalidValue, "x", Retryability::Never);
        let mut fields = Vec::new();
        for index in 0..20 {
            fields.push(DetailField {
                key: format!("key-{index}"),
                value: "v".to_string(),
            });
        }
        oversized.details = Some(ErrorDetails {
            fields: Some(fields),
        });
        assert!(oversized.validate().is_err());
        // Unbounded payloads are replaced by the bounded fallback, never forwarded.
        let rpc = oversized.into_jsonrpc_error();
        let decoded = EchoSdkError::from_jsonrpc_data(rpc.data.as_ref());
        assert!(
            decoded.is_ok_and(|decoded| decoded.code == ExtensionErrorCode::SerializationViolation)
        );
    }
}
