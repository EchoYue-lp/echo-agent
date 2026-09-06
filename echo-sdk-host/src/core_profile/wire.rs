//! Lossless conversions between framework facts and `_echo_agent/*` wire
//! DTOs, plus the single extension-error construction helpers.
//!
//! Every conversion preserves framework identity: sequences, hashes, ids and
//! failure categories travel verbatim. Errors are always built through
//! [`sdk_error`] so the fixed JSON-RPC code, the bounded message and the
//! operation/handle identity stay consistent across handlers.

use echo_agent::agent::EventEnvelope;
use echo_agent::error::ReactError;
use echo_agent::runtime::{TurnOutcome, TurnReceipt};
use echo_sdk_protocol::error::{AgentFailureWire, EchoSdkError, ExtensionErrorCode, Retryability};
use echo_sdk_protocol::event::{EventWireError, WireEventEnvelope};
use echo_sdk_protocol::handle::{HandleKind, WireHandle};
use echo_sdk_protocol::methods::{RunReceiptWire, RunStatus, RunTerminal};
use echo_sdk_protocol::scalar::WireU64;

/// Wire handle of a live object issued by this Host generation.
pub(crate) fn handle(id: impl Into<String>, kind: HandleKind, generation: u64) -> WireHandle {
    WireHandle {
        id: id.into(),
        generation: WireU64::from_u64(generation),
        kind,
    }
}

/// Typed extension error with operation identity.
pub(crate) fn sdk_error(
    code: ExtensionErrorCode,
    message: impl Into<String>,
    retryable: Retryability,
    operation: &str,
) -> EchoSdkError {
    EchoSdkError::new(code, message, retryable).with_operation(operation)
}

/// Typed extension error bound to the handle it refers to.
pub(crate) fn handle_error(
    code: ExtensionErrorCode,
    message: impl Into<String>,
    operation: &str,
    handle: &WireHandle,
) -> EchoSdkError {
    EchoSdkError::new(code, message, Retryability::Never)
        .with_operation(operation)
        .with_handle(handle.clone())
}

/// Encode a validated [`EchoSdkError`] as the official JSON-RPC error.
pub(crate) fn into_jsonrpc_error(error: EchoSdkError) -> agent_client_protocol::Error {
    error.into_jsonrpc_error()
}

/// Map a framework error onto the typed extension envelope. The framework
/// message is truncated UTF-8 safely before emission; secrets never enter
/// these messages because framework errors already redact credentials.
pub(crate) fn framework_error(error: &ReactError, operation: &str) -> EchoSdkError {
    sdk_error(
        ExtensionErrorCode::FrameworkError,
        bounded_message(&error.to_string()),
        Retryability::Never,
        operation,
    )
}

fn bounded_message(message: &str) -> String {
    const MAX_MESSAGE_CHARS: usize = 2048;
    message.chars().take(MAX_MESSAGE_CHARS).collect()
}

/// The single authoritative terminal projected from a framework receipt.
/// There is deliberately no `interrupted` mapping here: interruption is a
/// run status without a terminal, so a crashed run can never be mistaken
/// for success (design §15).
pub(crate) fn terminal_of(receipt: &TurnReceipt) -> std::result::Result<RunTerminal, String> {
    let terminal = match &receipt.outcome {
        TurnOutcome::Completed => RunTerminal::Completed {
            final_answer: receipt.final_answer.clone(),
        },
        TurnOutcome::Cancelled => RunTerminal::Cancelled,
        TurnOutcome::Failed(failure) => RunTerminal::Failed {
            failure: AgentFailureWire::from(failure),
        },
    };
    terminal.validate().map_err(|error| error.to_string())?;
    Ok(terminal)
}

/// Bounded receipt projection; counters stay lossless integer strings.
pub(crate) fn receipt_wire(receipt: &TurnReceipt) -> std::result::Result<RunReceiptWire, String> {
    let wire = RunReceiptWire {
        turn_id: receipt.turn_id.as_str().to_string(),
        outcome: receipt.status().to_string(),
        final_answer: receipt.final_answer.clone(),
        final_message_id: receipt
            .final_message_id
            .as_ref()
            .map(|id| id.as_str().to_string()),
        prompt_tokens: WireU64::from_u64(receipt.prompt_tokens),
        completion_tokens: WireU64::from_u64(receipt.completion_tokens),
        llm_calls: WireU64::from_u64(receipt.llm_calls),
        compaction_count: WireU64::from_u64(receipt.compaction_count),
        last_event_sequence: WireU64::from_u64(receipt.last_event_sequence),
        elapsed_ms: WireU64::from_u64(
            u64::try_from(receipt.elapsed.as_millis()).unwrap_or(u64::MAX),
        ),
    };
    wire.validate().map_err(|error| error.to_string())?;
    Ok(wire)
}

/// Live run status from a framework receipt (None = still running).
pub(crate) fn status_of(receipt: Option<&TurnReceipt>) -> RunStatus {
    match receipt {
        None => RunStatus::Running,
        Some(receipt) => match &receipt.outcome {
            TurnOutcome::Completed => RunStatus::Completed,
            TurnOutcome::Cancelled => RunStatus::Cancelled,
            TurnOutcome::Failed(_) => RunStatus::Failed,
        },
    }
}

/// Full `EventEnvelope` projection. Identity fields, sequence, content hash
/// and payload are preserved verbatim; conversion failure fails the run (the
/// observer surfaces it to the driver) instead of shipping a lossy event.
pub(crate) fn wire_envelope(envelope: &EventEnvelope) -> Result<WireEventEnvelope, EchoSdkError> {
    WireEventEnvelope::try_from(envelope.clone()).map_err(|error: EventWireError| {
        sdk_error(
            ExtensionErrorCode::SerializationViolation,
            format!("failed to project framework event: {error}"),
            Retryability::Never,
            "_echo_agent/event",
        )
    })
}

// ── Wire → framework conversions ───────────────────────────────────────────

/// Convert a wire working directory into a native path. UTF-8 paths convert
/// losslessly; encoded Unix-bytes/Windows-UTF-16 paths convert through the
/// OS encoding and fail closed with a typed error when impossible.
pub(crate) fn working_dir_from_wire(
    path: Option<&echo_sdk_protocol::scalar::WirePath>,
) -> std::result::Result<Option<std::path::PathBuf>, EchoSdkError> {
    let Some(path) = path else {
        return Ok(None);
    };
    to_path_buf(path).map(Some).map_err(|error| {
        sdk_error(
            ExtensionErrorCode::InvalidValue,
            format!("working_dir cannot be represented on this platform: {error}"),
            Retryability::Never,
            "_echo_agent/session",
        )
    })
}

/// Encode a native absolute path without lossy UTF-8 conversion. Unix paths
/// that are not valid UTF-8 use the byte-preserving wire variant; Windows
/// paths use UTF-16 code units as required by the protocol.
pub(crate) fn path_to_wire(
    path: &std::path::Path,
) -> std::result::Result<echo_sdk_protocol::scalar::WirePath, String> {
    use base64::Engine as _;
    use echo_sdk_protocol::scalar::WirePath;
    if !path.is_absolute() {
        return Err("path must be absolute".to_string());
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt as _;
        let bytes = path.as_os_str().as_bytes();
        if let Some(text) = path.to_str() {
            return Ok(WirePath::Utf8 {
                path: text.to_string(),
            });
        }
        Ok(WirePath::Unix {
            bytes_base64: base64::engine::general_purpose::STANDARD_NO_PAD.encode(bytes),
            display: None,
        })
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt as _;
        let units: Vec<u16> = path.as_os_str().encode_wide().collect();
        let mut bytes = Vec::with_capacity(units.len().saturating_mul(2));
        for unit in units {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        Ok(WirePath::Windows {
            utf16_base64: base64::engine::general_purpose::STANDARD_NO_PAD.encode(bytes),
            display: path.to_str().map(str::to_string),
        })
    }
    #[cfg(not(any(unix, windows)))]
    {
        Err("native paths are unsupported on this platform".to_string())
    }
}

pub(crate) fn path_from_wire(
    path: &echo_sdk_protocol::scalar::WirePath,
) -> std::result::Result<std::path::PathBuf, String> {
    path.validate()
        .map_err(|error| format!("invalid wire path: {error}"))?;
    to_path_buf(path)
}

/// Lossless-enough native conversion of a [`WirePath`]. UTF-8 paths convert
/// directly; Unix byte paths must be valid WTF-8 on this platform; Windows
/// UTF-16 paths only convert on Windows. Anything else fails closed with a
/// typed error instead of losing bytes.
fn to_path_buf(
    path: &echo_sdk_protocol::scalar::WirePath,
) -> std::result::Result<std::path::PathBuf, String> {
    use base64::Engine as _;
    use echo_sdk_protocol::scalar::WirePath;
    match path {
        WirePath::Utf8 { path } => Ok(std::path::PathBuf::from(path)),
        WirePath::Unix { bytes_base64, .. } => {
            let bytes = base64::engine::general_purpose::STANDARD_NO_PAD
                .decode(bytes_base64)
                .map_err(|error| format!("invalid base64 path encoding: {error}"))?;
            #[cfg(unix)]
            {
                use std::os::unix::ffi::OsStringExt as _;
                Ok(std::path::PathBuf::from(std::ffi::OsString::from_vec(
                    bytes,
                )))
            }
            #[cfg(not(unix))]
            {
                let _ = bytes;
                Err("Unix byte paths require a Unix platform".to_string())
            }
        }
        WirePath::Windows { utf16_base64, .. } => {
            let bytes = base64::engine::general_purpose::STANDARD_NO_PAD
                .decode(utf16_base64)
                .map_err(|error| format!("invalid base64 path encoding: {error}"))?;
            #[cfg(windows)]
            {
                let units: Vec<u16> = bytes
                    .chunks_exact(2)
                    .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                    .collect();
                Ok(std::path::PathBuf::from(std::ffi::OsString::from_wide(
                    &units,
                )))
            }
            #[cfg(not(windows))]
            {
                let _ = bytes;
                Err("Windows UTF-16 paths require a Windows platform".to_string())
            }
        }
    }
}

/// Build the framework model+agent config and its LLM client from the
/// explicit wire projection. Exactly one credential source arrives by
/// grammar; environment credentials are resolved here so secrets never sit
/// in the framework config longer than needed.
pub(crate) fn framework_from_wire(
    config: &echo_sdk_protocol::methods::AgentConfigExplicitWire,
) -> std::result::Result<
    (
        echo_agent::config::FrameworkConfig,
        std::sync::Arc<dyn echo_agent::llm::LlmClient>,
    ),
    String,
> {
    use echo_agent::config::{AgentSettings, FrameworkConfig, ModelConfig};
    use echo_agent::llm::{LlmApiProtocol, LlmConfig};
    use echo_sdk_protocol::methods::{CredentialSourceWire, LlmApiProtocolWire};

    const SUPPORTED_CONFIG_VERSION: u32 = 1;
    if config.config_version != SUPPORTED_CONFIG_VERSION {
        return Err(format!(
            "unsupported agent config_version {}; expected {SUPPORTED_CONFIG_VERSION}",
            config.config_version
        ));
    }
    let api_protocol = match config.model.api_protocol {
        LlmApiProtocolWire::ChatCompletions => LlmApiProtocol::ChatCompletions,
        LlmApiProtocolWire::Responses => LlmApiProtocol::Responses,
        LlmApiProtocolWire::Anthropic => LlmApiProtocol::Anthropic,
    };
    let credential_token = match &config.model.credential {
        None => None,
        Some(CredentialSourceWire::Inline { token }) => Some(token.clone()),
        Some(CredentialSourceWire::Env { variable }) => {
            Some(std::env::var(variable).map_err(|_| {
                format!("credential environment variable {variable} is unavailable")
            })?)
        }
    };
    let temperature = match &config.model.temperature {
        None => None,
        Some(text) => Some(
            text.parse::<f32>()
                .map_err(|_| format!("temperature must be a decimal number, got {text:?}"))?,
        ),
    };
    let context_window = match &config.model.context_window {
        None => None,
        Some(value) => {
            let raw = value.to_u64().ok_or("context_window is out of range")?;
            Some(u32::try_from(raw).map_err(|_| "context_window exceeds u32".to_string())?)
        }
    };
    let base_url = config.model.base_url.clone();
    let model = ModelConfig {
        provider: config.model.provider.clone(),
        name: config.model.name.clone(),
        // The credential is consumed to build the client above; retaining it
        // in the long-lived FrameworkConfig would leak inline secrets through
        // debug/state inspection and is unnecessary for future Sessions.
        auth_token: None,
        base_url: Some(base_url.clone()),
        api_protocol: Some(api_protocol),
        max_tokens: config.model.max_tokens,
        temperature,
        context_window,
    };
    let agent = AgentSettings {
        name: config.agent.name.clone(),
        system_prompt: config.agent.system_prompt.clone(),
        max_iterations: usize::try_from(config.agent.max_iterations)
            .map_err(|_| "max_iterations exceeds the platform width".to_string())?,
        enable_tools: true,
        ..AgentSettings::default()
    };
    let llm_client: std::sync::Arc<dyn echo_agent::llm::LlmClient> = std::sync::Arc::from(
        LlmConfig::for_provider(
            &config.model.provider,
            &base_url,
            credential_token.as_deref().unwrap_or(""),
            &config.model.name,
            api_protocol,
        )
        .and_then(|client_config| client_config.build_client())
        .map_err(|error| format!("failed to build model client: {error}"))?,
    );
    Ok((FrameworkConfig { model, agent }, llm_client))
}

#[cfg(test)]
mod tests {
    use super::*;
    use echo_agent::agent::{AgentEvent, EventIdentity};

    #[test]
    fn terminal_projects_cancelled() -> Result<(), ReactError> {
        let cancelled = TurnReceipt::cancelled("turn-wire-test")
            .map_err(|error| ReactError::Other(error.to_string()))?;
        assert!(matches!(
            terminal_of(&cancelled),
            Ok(RunTerminal::Cancelled)
        ));
        assert_eq!(status_of(Some(&cancelled)), RunStatus::Cancelled);
        assert_eq!(status_of(None), RunStatus::Running);
        let receipt = receipt_wire(&cancelled).map_err(ReactError::Other)?;
        assert_eq!(receipt.outcome, "cancelled");
        Ok(())
    }

    #[tokio::test]
    async fn wire_envelope_preserves_identity() -> Result<(), ReactError> {
        let identity = EventIdentity::new("stream-wire", "turn-wire")?;
        let envelope = EventEnvelope::new(&identity, 1, None, AgentEvent::Token("你好".into()))?;
        let wire = wire_envelope(&envelope).map_err(|error| ReactError::Other(error.message))?;
        assert_eq!(wire.sequence.to_u64(), Some(1));
        assert_eq!(wire.stream_id, "stream-wire");
        assert_eq!(wire.payload.event_type, "token");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn unix_non_utf8_paths_round_trip_without_loss() -> Result<(), ReactError> {
        use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
        let path =
            std::path::PathBuf::from(std::ffi::OsString::from_vec(b"/tmp/sdk-\xff".to_vec()));
        let wire = path_to_wire(&path).map_err(ReactError::Other)?;
        let restored = path_from_wire(&wire).map_err(ReactError::Other)?;
        assert_eq!(restored.as_os_str().as_bytes(), path.as_os_str().as_bytes());
        Ok(())
    }
}
