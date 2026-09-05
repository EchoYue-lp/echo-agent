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
}
