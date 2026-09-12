//! State and delivery family adapters (plan 07, todo 4).
//!
//! `_echo_agent/state/op` routes checkpoint/runtime-state operations onto
//! the Host's shared [`echo_agent::state::RuntimeStateStore`] — the same
//! authority the core profile checkpoints session state through. The Host
//! adds no second store; scope identities and clear receipts stay the
//! framework's own shapes.
//!
//! `_echo_agent/delivery/op` resource-izes the framework's
//! [`echo_agent::delivery::DeliveryLedger`] over an in-memory event
//! journal and projection checkpoint store. Enqueue/claim/transition/
//! defer/settle/recover are the ledger's real verbs: sequence, CAS and
//! retention semantics stay entirely with the framework.

use echo_agent::delivery::{
    DeliveryClaim, DeliveryEnvelope, DeliveryEvent, DeliveryLedger, DeliveryLedgerConfig,
    DeliveryLedgerError, DeliveryOutcome, DeliveryRecord, DeliverySettlement, DeliveryTransition,
};
use echo_agent::state::RuntimeStateStore as _;
use echo_agent::state::journal::{MemoryCheckpointStore, MemoryEventJournal};
use echo_sdk_protocol::error::{EchoSdkError, ExtensionErrorCode, Retryability};
use echo_sdk_protocol::handle::WireHandle;
use echo_sdk_protocol::methods::FeatureOperationRequest;
use echo_sdk_protocol::scalar::WireValue;
use std::collections::HashMap;
use std::sync::Arc;

use super::super::handles::HandleRegistry;
use super::super::wire;

const STATE_METHOD: &str = "_echo_agent/state/op";
const DELIVERY_METHOD: &str = "_echo_agent/delivery/op";
/// Checkpoint the delivery projection after every N journal appends.
const DELIVERY_CHECKPOINT_EVERY: u64 = 16;

/// One delivery ledger resource: the framework ledger over its in-memory
/// journal, plus the owning ACP session.
pub(crate) struct DeliveryLedgerRecord {
    pub ledger: DeliveryLedger<MemoryEventJournal<DeliveryEvent>>,
}

fn invalid(method: &'static str, message: impl Into<String>) -> EchoSdkError {
    wire::sdk_error(
        ExtensionErrorCode::InvalidValue,
        message,
        Retryability::Never,
        method,
    )
}

fn framework(method: &'static str, error: echo_agent::error::ReactError) -> EchoSdkError {
    wire::sdk_error(
        ExtensionErrorCode::FrameworkError,
        wire::bounded_framework_message(&error.to_string()),
        Retryability::Never,
        method,
    )
}

fn json_of(
    method: &'static str,
    value: &WireValue,
    position: usize,
) -> Result<serde_json::Value, EchoSdkError> {
    value.clone().into_json().map_err(|error| {
        invalid(
            method,
            format!("argument {position} is not a lossless wire value: {error}"),
        )
    })
}

fn string_at(
    method: &'static str,
    arguments: &[serde_json::Value],
    position: usize,
    what: &str,
) -> Result<String, EchoSdkError> {
    arguments
        .get(position)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| {
            invalid(
                method,
                format!("operation requires {what} at argument {position}"),
            )
        })
}

// ── State family ────────────────────────────────────────────────────────────

/// Dispatch one state family operation over the Host's shared runtime
/// state store (the same authority session checkpoints flow through).
pub(crate) async fn dispatch_state(
    store: &Arc<echo_agent::state::FileRuntimeStateStore>,
    request: &FeatureOperationRequest,
) -> Result<WireValue, EchoSdkError> {
    let wire = |value: serde_json::Value| {
        WireValue::from_json(value).map_err(|error| invalid(STATE_METHOD, error.to_string()))
    };
    let arguments: Vec<serde_json::Value> = request
        .arguments
        .iter()
        .enumerate()
        .map(|(position, value)| json_of(STATE_METHOD, value, position))
        .collect::<Result<_, _>>()?;
    match request.operation.as_str() {
        "state.checkpoint.get" => {
            let scope = string_at(STATE_METHOD, &arguments, 0, "a scope id")?;
            let checkpoint = store
                .get_checkpoint(&scope)
                .await
                .map_err(|error| framework(STATE_METHOD, error))?;
            let value = checkpoint
                .as_ref()
                .map(|checkpoint| serde_json::to_value(checkpoint).unwrap_or_default())
                .unwrap_or(serde_json::Value::Null);
            wire(serde_json::json!({"checkpoint": value}))
        }
        "state.checkpoint.save" => {
            let scope = string_at(STATE_METHOD, &arguments, 0, "a scope id")?;
            let payload = arguments.get(1).ok_or_else(|| {
                invalid(
                    STATE_METHOD,
                    "state.checkpoint.save requires a checkpoint at argument 1",
                )
            })?;
            let checkpoint: echo_agent::state::AgentCheckpoint =
                serde_json::from_value(payload.clone()).map_err(|error| {
                    invalid(
                        STATE_METHOD,
                        format!("checkpoint payload malformed: {error}"),
                    )
                })?;
            store
                .save_checkpoint_for_scope(&scope, &checkpoint)
                .await
                .map_err(|error| framework(STATE_METHOD, error))?;
            wire(serde_json::json!({"ok": true}))
        }
        "state.runtime.list" => {
            let scope = string_at(STATE_METHOD, &arguments, 0, "a scope id")?;
            let ids = store
                .runtime_state_ids(&scope)
                .await
                .map_err(|error| framework(STATE_METHOD, error))?;
            wire(serde_json::json!({"runtime_state_ids": ids}))
        }
        "state.runtime.clear" => {
            let scope = string_at(STATE_METHOD, &arguments, 0, "a scope id")?;
            let runtime_state_id = string_at(STATE_METHOD, &arguments, 1, "a runtime state id")?;
            let receipt = store
                .clear_runtime_state(&scope, &runtime_state_id)
                .await
                .map_err(|error| framework(STATE_METHOD, error))?;
            wire(serde_json::json!({
                "scope_id": receipt.scope_id,
                "runtime_state_id": receipt.runtime_state_id,
                "checkpoint_removed": receipt.checkpoint_removed,
            }))
        }
        "state.scope.clear" => {
            let scope = string_at(STATE_METHOD, &arguments, 0, "a scope id")?;
            let receipt = store
                .clear_runtime_state_scope(&scope)
                .await
                .map_err(|error| framework(STATE_METHOD, error))?;
            wire(serde_json::json!({
                "scope_id": receipt.scope_id,
                "runtime_state_ids": receipt.runtime_state_ids,
            }))
        }
        other => Err(invalid(
            STATE_METHOD,
            format!("unknown state operation {other}; the family surface is closed"),
        )),
    }
}

// ── Delivery family ─────────────────────────────────────────────────────────

/// Resolve one ledger resource, owner-checked against the session.
fn ledger_record(
    handles: &HandleRegistry,
    ledgers: &std::sync::Mutex<HashMap<String, Arc<DeliveryLedgerRecord>>>,
    resource: &WireHandle,
    owner: &str,
) -> Result<Arc<DeliveryLedgerRecord>, EchoSdkError> {
    super::owned_resource(handles, resource, owner, DELIVERY_METHOD)?;
    ledgers
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&resource.id)
        .cloned()
        .ok_or_else(|| {
            invalid(
                DELIVERY_METHOD,
                format!("unknown ledger resource {}", resource.id),
            )
        })
}

/// Rebuild the framework claim for one message from the live projection;
/// transitions require the exact claim identity the ledger issued.
fn claim_of(
    record: &DeliveryLedgerRecord,
    message_id: &str,
) -> Result<DeliveryClaim<String, serde_json::Value>, EchoSdkError> {
    let found = record
        .ledger
        .with_projection(|projection| projection.record(message_id).cloned());
    let found: DeliveryRecord<String, serde_json::Value> =
        found.ok_or_else(|| invalid(DELIVERY_METHOD, format!("unknown message {message_id}")))?;
    let attempt_id = found.attempt_id.ok_or_else(|| {
        invalid(
            DELIVERY_METHOD,
            format!("message {message_id} has not been claimed yet"),
        )
    })?;
    let claimed_at = found.claimed_at.ok_or_else(|| {
        invalid(
            DELIVERY_METHOD,
            format!("message {message_id} has no claim timestamp"),
        )
    })?;
    Ok(DeliveryClaim {
        payload: found.payload,
        message_id: found.message_id,
        route: found.route,
        attempt_id,
        attempt: found.attempt,
        claimed_at,
    })
}

fn outcome_of(text: &str) -> Result<DeliveryOutcome, EchoSdkError> {
    match text {
        "completed" => Ok(DeliveryOutcome::Completed),
        "failed" => Ok(DeliveryOutcome::Failed),
        "cancelled" => Ok(DeliveryOutcome::Cancelled),
        "dropped" => Ok(DeliveryOutcome::Dropped),
        "outcome_unknown" => Ok(DeliveryOutcome::OutcomeUnknown),
        other => Err(invalid(
            DELIVERY_METHOD,
            format!(
                "unknown delivery outcome {other}; expected completed|failed|cancelled|dropped|outcome_unknown"
            ),
        )),
    }
}

/// Delivery ledger errors are framework outcomes (sequence conflicts,
/// preflight violations); surface their text through the framework error
/// code without swallowing their identity.
fn ledger_error(error: DeliveryLedgerError) -> echo_agent::error::ReactError {
    echo_agent::error::ReactError::Other(error.to_string())
}

/// Dispatch one delivery family operation.
pub(crate) async fn dispatch_delivery(
    handles: &HandleRegistry,
    ledgers: &std::sync::Mutex<HashMap<String, Arc<DeliveryLedgerRecord>>>,
    owner: &str,
    request: &FeatureOperationRequest,
    limits: super::FacadeFamilyLimits,
) -> Result<WireValue, EchoSdkError> {
    let wire = |value: serde_json::Value| {
        WireValue::from_json(value).map_err(|error| invalid(DELIVERY_METHOD, error.to_string()))
    };
    let arguments: Vec<serde_json::Value> = request
        .arguments
        .iter()
        .enumerate()
        .map(|(position, value)| json_of(DELIVERY_METHOD, value, position))
        .collect::<Result<_, _>>()?;
    match request.operation.as_str() {
        "delivery.ledger.open" => {
            let journal = Arc::new(MemoryEventJournal::<DeliveryEvent>::new());
            let checkpoints = Arc::new(MemoryCheckpointStore::new());
            let (resource, _record) = handles.register_facade_resource(
                limits.max_resources,
                "delivery",
                "delivery.ledger",
                Some(owner),
                DELIVERY_METHOD,
            )?;
            ledgers
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(
                    resource.id.clone(),
                    Arc::new(DeliveryLedgerRecord {
                        ledger: DeliveryLedger::new(
                            journal,
                            checkpoints,
                            DeliveryLedgerConfig::default(),
                            DELIVERY_CHECKPOINT_EVERY,
                        ),
                    }),
                );
            wire(serde_json::json!({"resource": resource}))
        }
        "delivery.enqueue" => {
            let resource = super::resource_handle_at(
                &arguments,
                0,
                "a delivery ledger resource",
                DELIVERY_METHOD,
            )?;
            let message_id = string_at(DELIVERY_METHOD, &arguments, 1, "a message id")?;
            let route = string_at(DELIVERY_METHOD, &arguments, 2, "a route")?;
            let payload = arguments.get(3).ok_or_else(|| {
                invalid(
                    DELIVERY_METHOD,
                    "delivery.enqueue requires a payload at argument 3",
                )
            })?;
            let record = ledger_record(handles, ledgers, &resource, owner)?;
            let envelope = DeliveryEnvelope::new(message_id, route, payload.clone());
            envelope
                .validate()
                .map_err(|error| framework(DELIVERY_METHOD, error))?;
            record
                .ledger
                .enqueue(envelope)
                .map_err(|error| framework(DELIVERY_METHOD, ledger_error(error)))?;
            wire(serde_json::json!({"ok": true}))
        }
        "delivery.claim_next" => {
            let resource = super::resource_handle_at(
                &arguments,
                0,
                "a delivery ledger resource",
                DELIVERY_METHOD,
            )?;
            let record = ledger_record(handles, ledgers, &resource, owner)?;
            let claim = record
                .ledger
                .claim_next()
                .map_err(|error| framework(DELIVERY_METHOD, ledger_error(error)))?;
            let claim = claim
                .map(|claim| {
                    serde_json::json!({
                        "message_id": claim.message_id,
                        "route": claim.route,
                        "payload": claim.payload,
                        "attempt_id": claim.attempt_id,
                        "attempt": claim.attempt,
                    })
                })
                .unwrap_or(serde_json::Value::Null);
            wire(serde_json::json!({"claim": claim}))
        }
        "delivery.transition" => {
            let resource = super::resource_handle_at(
                &arguments,
                0,
                "a delivery ledger resource",
                DELIVERY_METHOD,
            )?;
            let message_id = string_at(DELIVERY_METHOD, &arguments, 1, "a message id")?;
            let kind = string_at(DELIVERY_METHOD, &arguments, 2, "a transition kind")?;
            let turn_id = string_at(DELIVERY_METHOD, &arguments, 3, "a turn id")?;
            let record = ledger_record(handles, ledgers, &resource, owner)?;
            let claim = claim_of(&record, &message_id)?;
            let transition = match kind.as_str() {
                "effect_started" => DeliveryTransition::effect_started(turn_id),
                "mailbox_accepted" => DeliveryTransition::mailbox_accepted(turn_id),
                "drained" => DeliveryTransition::drained(turn_id),
                other => {
                    return Err(invalid(
                        DELIVERY_METHOD,
                        format!(
                            "unknown delivery transition {other}; expected effect_started|mailbox_accepted|drained"
                        ),
                    ));
                }
            };
            record
                .ledger
                .transition(&claim, transition)
                .map_err(|error| framework(DELIVERY_METHOD, ledger_error(error)))?;
            wire(serde_json::json!({"ok": true}))
        }
        "delivery.defer" => {
            let resource = super::resource_handle_at(
                &arguments,
                0,
                "a delivery ledger resource",
                DELIVERY_METHOD,
            )?;
            let message_id = string_at(DELIVERY_METHOD, &arguments, 1, "a message id")?;
            let reason = string_at(DELIVERY_METHOD, &arguments, 2, "a reason")?;
            let next_attempt_at = string_at(DELIVERY_METHOD, &arguments, 3, "an RFC3339 deadline")?;
            let record = ledger_record(handles, ledgers, &resource, owner)?;
            let claim = claim_of(&record, &message_id)?;
            let parsed =
                chrono::DateTime::parse_from_rfc3339(&next_attempt_at).map_err(|error| {
                    invalid(
                        DELIVERY_METHOD,
                        format!("next_attempt_at is not RFC3339: {error}"),
                    )
                })?;
            let deadline = parsed.with_timezone(&chrono::Utc);
            record
                .ledger
                .defer(&claim, reason, deadline)
                .map_err(|error| framework(DELIVERY_METHOD, ledger_error(error)))?;
            wire(serde_json::json!({"ok": true}))
        }
        "delivery.settle" => {
            let resource = super::resource_handle_at(
                &arguments,
                0,
                "a delivery ledger resource",
                DELIVERY_METHOD,
            )?;
            let message_id = string_at(DELIVERY_METHOD, &arguments, 1, "a message id")?;
            let outcome = string_at(DELIVERY_METHOD, &arguments, 2, "an outcome")?;
            let reason = arguments.get(3).and_then(serde_json::Value::as_str);
            let turn_id = arguments.get(4).and_then(serde_json::Value::as_str);
            let record = ledger_record(handles, ledgers, &resource, owner)?;
            let claim = claim_of(&record, &message_id)?;
            let settlement = DeliverySettlement::terminal(
                turn_id.map(str::to_string),
                outcome_of(&outcome)?,
                None,
                reason.map(str::to_string),
                None,
            );
            record
                .ledger
                .settle(&claim, settlement)
                .map_err(|error| framework(DELIVERY_METHOD, ledger_error(error)))?;
            wire(serde_json::json!({"ok": true}))
        }
        "delivery.recover" => {
            let resource = super::resource_handle_at(
                &arguments,
                0,
                "a delivery ledger resource",
                DELIVERY_METHOD,
            )?;
            let record = ledger_record(handles, ledgers, &resource, owner)?;
            let receipt = record
                .ledger
                .recover()
                .map_err(|error| framework(DELIVERY_METHOD, error))?;
            wire(serde_json::json!({
                "last_applied_sequence": receipt.last_applied_sequence,
            }))
        }
        "delivery.snapshot" => {
            let resource = super::resource_handle_at(
                &arguments,
                0,
                "a delivery ledger resource",
                DELIVERY_METHOD,
            )?;
            let record = ledger_record(handles, ledgers, &resource, owner)?;
            let page = record.ledger.with_projection(|projection| {
                projection
                    .records()
                    .take(limits.page)
                    .map(|record| serde_json::to_value(record).unwrap_or_default())
                    .collect::<Vec<_>>()
            });
            wire(serde_json::json!({"records": page}))
        }
        other => Err(invalid(
            DELIVERY_METHOD,
            format!("unknown delivery operation {other}; the family surface is closed"),
        )),
    }
}

/// Drop every delivery ledger owned by one session (session close).
pub(crate) fn drop_session_ledgers(
    ledgers: &std::sync::Mutex<HashMap<String, Arc<DeliveryLedgerRecord>>>,
    closed: &[String],
) {
    ledgers
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .retain(|id, _| !closed.iter().any(|closed_id| closed_id == id));
}
