//! Structured-output contract validation (plan 07, todo 3).
//!
//! `_echo_agent/structured_output/validate` checks one structured-output
//! contract before a run depends on it: the JSON Schema itself must compile
//! (typed parse errors, never a silent fallback) and an optional sample
//! instance must satisfy it. Validation is contract-level only — executing
//! a run with a structured-output contract still goes through the one
//! AgentTurnDriver path (design §10.4); this surface never runs the agent.

use echo_sdk_protocol::error::{EchoSdkError, ExtensionErrorCode, Retryability};
use echo_sdk_protocol::methods::FeatureOperationRequest;
use echo_sdk_protocol::scalar::WireValue;

use super::super::wire;

/// Maximum accepted schema document size; matches the negotiated
/// `max_structured_output_bytes` default.
const MAX_SCHEMA_BYTES: usize = 262_144;

fn invalid(message: impl Into<String>) -> EchoSdkError {
    wire::sdk_error(
        ExtensionErrorCode::InvalidValue,
        message,
        Retryability::Never,
        "_echo_agent/structured_output/validate",
    )
}

/// Validate one contract request: `arguments = [schema, instance?]` where
/// `schema` is a JSON Schema object and `instance` an optional sample.
pub(crate) fn validate(request: &FeatureOperationRequest) -> Result<WireValue, EchoSdkError> {
    if request.arguments.len() > 2 {
        return Err(invalid(
            "structured output validation accepts at most a schema and one sample instance",
        ));
    }
    let schema = request
        .arguments
        .first()
        .ok_or_else(|| invalid("structured output validation requires a schema argument"))?;
    let encoded = schema
        .clone()
        .into_json()
        .map_err(|error| invalid(format!("schema is not a lossless wire value: {error}")))?;
    let encoded_len = serde_json::to_vec(&encoded)
        .map(|bytes| bytes.len())
        .unwrap_or(usize::MAX);
    if encoded_len > MAX_SCHEMA_BYTES {
        return Err(wire::sdk_error(
            ExtensionErrorCode::PayloadTooLarge,
            "structured output schema exceeds the payload bound",
            Retryability::Never,
            "_echo_agent/structured_output/validate",
        ));
    }
    let compiled = jsonschema::validator_for(&encoded)
        .map_err(|error| invalid(format!("schema does not compile: {error}")))?;
    let mut errors = Vec::new();
    if let Some(instance) = request.arguments.get(1) {
        let value = instance
            .clone()
            .into_json()
            .map_err(|error| invalid(format!("instance is not a lossless wire value: {error}")))?;
        for item in compiled.iter_errors(&value).take(16) {
            errors.push(serde_json::json!({
                "instance_path": item.instance_path().to_string(),
                "message": item.to_string(),
            }));
        }
    }
    let value = serde_json::json!({
        "valid": errors.is_empty(),
        "errors": errors,
    });
    WireValue::from_json(value)
        .map_err(|error| invalid(format!("validation result is not encodable: {error}")))
}
