//! Product-neutral durable delivery ledger primitives.
//!
//! The ledger models an ordered delivery lifecycle without knowing what an
//! application route, Agent, workspace, or payload means. Consumers provide
//! their route and payload types, while the framework owns lifecycle identity,
//! attempt ordering, terminal state, and a bounded replayable projection.

use crate::journal::{
    ApplyBatchReceipt, ApplyReceipt, CheckpointStore, CheckpointedApplyError, CheckpointedReducer,
    EventJournal, EventReducer, JournalBatchAppendResult, JournalBatchLookup, JournalRecord,
    PreparedJournalBatch, RecoveryReceipt,
};
use chrono::{DateTime, Utc};
use echo_core::error::{ReactError, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fmt;
use std::sync::Arc;

/// Version of the framework delivery event contract.
pub const DELIVERY_LEDGER_SCHEMA_VERSION: u16 = 1;

/// Route values accepted by a typed delivery ledger.
///
/// The framework only needs a stable, serializable route with a small amount
/// of validation. Product code can implement this contract for its own
/// address type without converting that address into a framework-owned string.
pub trait DeliveryRoute:
    Clone + PartialEq + Serialize + for<'de> Deserialize<'de> + Send + Sync + fmt::Debug + 'static
{
    /// Reject an unusable route before it enters the durable ledger.
    fn validate(&self) -> Result<()>;
}

impl DeliveryRoute for String {
    fn validate(&self) -> Result<()> {
        if self.trim().is_empty() {
            return Err(ReactError::Other("delivery route is empty".to_string()));
        }
        Ok(())
    }
}

/// Payload values retained by a typed delivery ledger.
pub trait DeliveryPayload:
    Clone + PartialEq + Serialize + for<'de> Deserialize<'de> + Send + Sync + fmt::Debug + 'static
{
}

impl<T> DeliveryPayload for T where
    T: Clone
        + PartialEq
        + Serialize
        + for<'de> Deserialize<'de>
        + Send
        + Sync
        + fmt::Debug
        + 'static
{
}

/// Terminal outcome of one delivery attempt or message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryOutcome {
    Completed,
    Failed,
    Cancelled,
    Dropped,
    OutcomeUnknown,
}

impl DeliveryOutcome {
    /// Stable snake-case spelling for logs and wire clients.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Dropped => "dropped",
            Self::OutcomeUnknown => "outcome_unknown",
        }
    }
}

/// Durable phase of one delivery record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryPhase {
    Persisted,
    Claimed,
    EffectStarted,
    MailboxAccepted,
    Drained,
    Deferred,
    TurnSettled,
}

impl DeliveryPhase {
    /// Stable snake-case spelling for logs and wire clients.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Persisted => "persisted",
            Self::Claimed => "claimed",
            Self::EffectStarted => "effect_started",
            Self::MailboxAccepted => "mailbox_accepted",
            Self::Drained => "drained",
            Self::Deferred => "deferred",
            Self::TurnSettled => "turn_settled",
        }
    }
}

/// Product-neutral typed message envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryEnvelope<Route = String, Payload = Value> {
    /// Stable idempotency identity chosen by the application.
    pub message_id: String,
    /// Caller-owned route identity.
    pub route: Route,
    /// Caller-owned payload retained without a JSON round trip.
    pub payload: Payload,
    /// Stable metadata used for correlation and caller policy.
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
    pub correlation_id: Option<String>,
    pub causation_id: Option<String>,
}

impl<Route, Payload> DeliveryEnvelope<Route, Payload>
where
    Route: DeliveryRoute,
{
    /// Construct a typed envelope without duplicating route or payload values.
    pub fn new(message_id: impl Into<String>, route: Route, payload: Payload) -> Self {
        Self {
            message_id: message_id.into(),
            route,
            payload,
            metadata: BTreeMap::new(),
            correlation_id: None,
            causation_id: None,
        }
    }

    /// Validate the identity and route without applying product policy.
    pub fn validate(&self) -> Result<()> {
        if self.message_id.trim().is_empty() {
            return Err(ReactError::Other(
                "delivery message_id is empty".to_string(),
            ));
        }
        self.route.validate()
    }
}

/// Durable lifecycle facts for one envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case", tag = "kind")]
pub enum DeliveryEvent<Route = String, Payload = Value> {
    Persisted {
        envelope: DeliveryEnvelope<Route, Payload>,
        persisted_at: DateTime<Utc>,
    },
    Claimed {
        message_id: String,
        attempt_id: String,
        attempt: u32,
        claimed_at: DateTime<Utc>,
    },
    EffectStarted {
        message_id: String,
        attempt_id: String,
        turn_id: String,
        started_at: DateTime<Utc>,
    },
    MailboxAccepted {
        message_id: String,
        attempt_id: String,
        turn_id: String,
        accepted_at: DateTime<Utc>,
    },
    Drained {
        message_id: String,
        attempt_id: String,
        turn_id: String,
        drained_at: DateTime<Utc>,
    },
    Deferred {
        message_id: String,
        attempt_id: String,
        reason: String,
        deferred_at: DateTime<Utc>,
        next_attempt_at: DateTime<Utc>,
    },
    TurnSettled {
        message_id: String,
        attempt_id: String,
        turn_id: Option<String>,
        outcome: DeliveryOutcome,
        drained: Option<bool>,
        reason: Option<String>,
        retryable: bool,
        next_attempt_at: Option<DateTime<Utc>>,
        reply_message_id: Option<String>,
        settled_at: DateTime<Utc>,
    },
}

/// Logical bounds for terminal records retained in the hot projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryLedgerConfig {
    pub terminal_retention: usize,
    pub terminal_retention_bytes: usize,
}

impl Default for DeliveryLedgerConfig {
    fn default() -> Self {
        Self {
            terminal_retention: 256,
            terminal_retention_bytes: 256 * 1024,
        }
    }
}

/// One projected delivery record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryRecord<Route = String, Payload = Value> {
    pub message_id: String,
    pub route: Route,
    pub payload: Payload,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
    pub correlation_id: Option<String>,
    pub causation_id: Option<String>,
    pub persisted_at: DateTime<Utc>,
    pub phase: DeliveryPhase,
    pub outcome: Option<DeliveryOutcome>,
    pub drained: bool,
    pub reason: Option<String>,
    pub attempt_id: Option<String>,
    pub attempt: u32,
    pub claimed_at: Option<DateTime<Utc>>,
    pub effect_started_at: Option<DateTime<Utc>>,
    pub mailbox_accepted_at: Option<DateTime<Utc>>,
    pub drained_at: Option<DateTime<Utc>>,
    pub turn_settled_at: Option<DateTime<Utc>>,
    pub turn_id: Option<String>,
    pub reply_message_id: Option<String>,
    pub next_attempt_at: Option<DateTime<Utc>>,
    pub terminal: bool,
    pub retained_bytes: usize,
}

impl<Route, Payload> DeliveryRecord<Route, Payload>
where
    Route: DeliveryRoute,
    Payload: DeliveryPayload,
{
    fn new(
        envelope: DeliveryEnvelope<Route, Payload>,
        persisted_at: DateTime<Utc>,
    ) -> std::result::Result<Self, String> {
        let retained_bytes = serde_json::to_vec(&envelope)
            .map_err(|error| format!("delivery envelope serialization failed: {error}"))?
            .len();
        Ok(Self {
            message_id: envelope.message_id,
            route: envelope.route,
            payload: envelope.payload,
            metadata: envelope.metadata,
            correlation_id: envelope.correlation_id,
            causation_id: envelope.causation_id,
            persisted_at,
            phase: DeliveryPhase::Persisted,
            outcome: None,
            drained: false,
            reason: None,
            attempt_id: None,
            attempt: 0,
            claimed_at: None,
            effect_started_at: None,
            mailbox_accepted_at: None,
            drained_at: None,
            turn_settled_at: None,
            turn_id: None,
            reply_message_id: None,
            next_attempt_at: None,
            terminal: false,
            retained_bytes,
        })
    }

    fn matches_attempt(&self, message_id: &str, attempt_id: &str) -> bool {
        self.message_id == message_id && self.attempt_id.as_deref() == Some(attempt_id)
    }

    /// Reconstruct the event envelope when a caller needs the event shape.
    pub fn envelope(&self) -> DeliveryEnvelope<Route, Payload> {
        DeliveryEnvelope {
            message_id: self.message_id.clone(),
            route: self.route.clone(),
            payload: self.payload.clone(),
            metadata: self.metadata.clone(),
            correlation_id: self.correlation_id.clone(),
            causation_id: self.causation_id.clone(),
        }
    }
}

/// One exact delivery attempt selected from the FIFO frontier.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryClaim<Route = String, Payload = Value> {
    /// The caller-owned payload selected from the FIFO frontier.
    pub payload: Payload,
    pub message_id: String,
    pub route: Route,
    pub attempt_id: String,
    pub attempt: u32,
    pub claimed_at: DateTime<Utc>,
}

/// A claim and its validated lifecycle event before physical commit.
///
/// Hosts with custom durability can append `event` through
/// [`DeliveryLedger::apply_prepared_with`] and then return `claim` to their
/// caller. The framework has already selected the FIFO item and assigned its
/// exact attempt identity.
#[derive(Debug, Clone, PartialEq)]
pub struct DeliveryClaimDraft<Route = String, Payload = Value> {
    pub claim: DeliveryClaim<Route, Payload>,
    pub event: DeliveryEvent<Route, Payload>,
}

/// Durable state needed to reconcile an attempt whose effect has started.
#[derive(Debug, Clone, PartialEq)]
pub struct DeliveryInFlight<Route = String, Payload = Value> {
    pub claim: DeliveryClaim<Route, Payload>,
    pub phase: DeliveryPhase,
    pub effect_started: bool,
    pub turn_id: String,
}

/// Terminal facts for one delivery attempt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeliverySettlement {
    pub turn_id: Option<String>,
    pub outcome: DeliveryOutcome,
    pub drained: Option<bool>,
    pub reason: Option<String>,
    pub retryable: bool,
    pub next_attempt_at: Option<DateTime<Utc>>,
    pub reply_message_id: Option<String>,
}

impl DeliverySettlement {
    /// Construct a terminal settlement that does not requeue the delivery.
    pub fn terminal(
        turn_id: Option<String>,
        outcome: DeliveryOutcome,
        drained: Option<bool>,
        reason: Option<String>,
        reply_message_id: Option<String>,
    ) -> Self {
        Self {
            turn_id,
            outcome,
            drained,
            reason,
            retryable: false,
            next_attempt_at: None,
            reply_message_id,
        }
    }

    /// Construct a retryable settlement with the next eligible attempt time.
    pub fn retry(
        turn_id: Option<String>,
        outcome: DeliveryOutcome,
        drained: Option<bool>,
        reason: Option<String>,
        next_attempt_at: DateTime<Utc>,
    ) -> Self {
        Self {
            turn_id,
            outcome,
            drained,
            reason,
            retryable: true,
            next_attempt_at: Some(next_attempt_at),
            reply_message_id: None,
        }
    }
}

/// One lifecycle transition for an exact delivery claim.
///
/// Applications can pass this value through their own physical journal
/// authority without defining a second enum that mirrors the framework
/// lifecycle. The transition describes only framework facts; route policy,
/// wake-up behavior, and surface receipts remain application concerns.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum DeliveryTransition {
    EffectStarted {
        turn_id: String,
    },
    MailboxAccepted {
        turn_id: String,
    },
    Drained {
        turn_id: String,
    },
    Deferred {
        reason: String,
        next_attempt_at: DateTime<Utc>,
    },
    Settled {
        settlement: DeliverySettlement,
    },
}

impl DeliveryTransition {
    pub fn effect_started(turn_id: impl Into<String>) -> Self {
        Self::EffectStarted {
            turn_id: turn_id.into(),
        }
    }

    pub fn mailbox_accepted(turn_id: impl Into<String>) -> Self {
        Self::MailboxAccepted {
            turn_id: turn_id.into(),
        }
    }

    pub fn drained(turn_id: impl Into<String>) -> Self {
        Self::Drained {
            turn_id: turn_id.into(),
        }
    }

    pub fn deferred(reason: impl Into<String>, next_attempt_at: DateTime<Utc>) -> Self {
        Self::Deferred {
            reason: reason.into(),
            next_attempt_at,
        }
    }

    pub fn settled(settlement: DeliverySettlement) -> Self {
        Self::Settled { settlement }
    }
}

/// Rebuildable FIFO and lifecycle projection for a delivery ledger.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryLedgerProjection<Route = String, Payload = Value> {
    order: VecDeque<String>,
    frontier: VecDeque<String>,
    entries: HashMap<String, DeliveryRecord<Route, Payload>>,
    terminal_retained_bytes: usize,
    invalid: Option<String>,
}

impl<Route, Payload> Default for DeliveryLedgerProjection<Route, Payload> {
    fn default() -> Self {
        Self {
            order: VecDeque::new(),
            frontier: VecDeque::new(),
            entries: HashMap::new(),
            terminal_retained_bytes: 0,
            invalid: None,
        }
    }
}

impl<Route, Payload> EventReducer for DeliveryLedgerProjection<Route, Payload>
where
    Route: DeliveryRoute,
    Payload: DeliveryPayload,
{
    type Event = DeliveryEvent<Route, Payload>;

    fn apply(&mut self, event: &Self::Event) {
        if self.invalid.is_none()
            && let Err(error) = self.apply_checked(event)
        {
            self.invalid = Some(error);
        }
    }

    fn apply_record(&mut self, record: &JournalRecord<Self::Event>) {
        self.apply(&record.event);
    }
}

impl<Route, Payload> DeliveryLedgerProjection<Route, Payload>
where
    Route: DeliveryRoute,
    Payload: DeliveryPayload,
{
    /// Rehydrate a projection from retained records.
    pub fn from_records(
        records: impl IntoIterator<Item = DeliveryRecord<Route, Payload>>,
        config: DeliveryLedgerConfig,
    ) -> Result<Self> {
        let mut projection = Self::default();
        for record in records {
            record.route.validate()?;
            let message_id = record.message_id.clone();
            if projection.entries.contains_key(&message_id) {
                return Err(ReactError::Other(format!(
                    "duplicate delivery checkpoint record {message_id}"
                )));
            }
            projection.order.push_back(message_id.clone());
            if !record.terminal {
                projection.frontier.push_back(message_id.clone());
            } else {
                projection.terminal_retained_bytes = projection
                    .terminal_retained_bytes
                    .saturating_add(record.retained_bytes);
            }
            projection.entries.insert(message_id, record);
        }
        projection.validate(config)?;
        Ok(projection)
    }

    fn enforce_retention(&mut self, config: DeliveryLedgerConfig) {
        loop {
            let terminal_count = self.entries.values().filter(|entry| entry.terminal).count();
            if terminal_count <= config.terminal_retention
                && self.terminal_retained_bytes <= config.terminal_retention_bytes
            {
                return;
            }
            let Some(message_id) = self
                .order
                .iter()
                .find(|id| self.entries.get(*id).is_some_and(|entry| entry.terminal))
                .cloned()
            else {
                return;
            };
            self.order.retain(|id| id != &message_id);
            if let Some(entry) = self.entries.remove(&message_id) {
                self.terminal_retained_bytes = self
                    .terminal_retained_bytes
                    .saturating_sub(entry.retained_bytes);
            }
        }
    }

    fn entry_mut(
        &mut self,
        message_id: &str,
    ) -> std::result::Result<&mut DeliveryRecord<Route, Payload>, String> {
        self.entries
            .get_mut(message_id)
            .ok_or_else(|| format!("delivery message {message_id} is unknown"))
    }

    fn check_attempt(
        entry: &DeliveryRecord<Route, Payload>,
        message_id: &str,
        attempt_id: &str,
    ) -> std::result::Result<(), String> {
        if !entry.matches_attempt(message_id, attempt_id) {
            return Err(format!("stale delivery claim for message {message_id}"));
        }
        Ok(())
    }

    fn apply_checked(
        &mut self,
        event: &DeliveryEvent<Route, Payload>,
    ) -> std::result::Result<(), String> {
        match event {
            DeliveryEvent::Persisted {
                envelope,
                persisted_at,
            } => {
                envelope.validate().map_err(|error| error.to_string())?;
                if self.entries.contains_key(&envelope.message_id) {
                    return Err(format!(
                        "duplicate delivery message {}",
                        envelope.message_id
                    ));
                }
                let record = DeliveryRecord::new(envelope.clone(), *persisted_at)?;
                let message_id = envelope.message_id.clone();
                self.order.push_back(message_id.clone());
                self.frontier.push_back(message_id.clone());
                self.entries.insert(message_id, record);
            }
            DeliveryEvent::Claimed {
                message_id,
                attempt_id,
                attempt,
                claimed_at,
            } => {
                if attempt_id.trim().is_empty() || *attempt == 0 {
                    return Err(format!("invalid claim identity for message {message_id}"));
                }
                if self.frontier.front().map(String::as_str) != Some(message_id.as_str()) {
                    return Err(format!(
                        "message {message_id} is not the next FIFO delivery frontier"
                    ));
                }
                let entry = self.entry_mut(message_id)?;
                if entry.terminal
                    || !matches!(
                        entry.phase,
                        DeliveryPhase::Persisted | DeliveryPhase::Deferred
                    ) && !(entry.phase == DeliveryPhase::Claimed
                        && entry.effect_started_at.is_none())
                {
                    return Err(format!(
                        "message {message_id} cannot be claimed from {:?}",
                        entry.phase
                    ));
                }
                if *attempt <= entry.attempt {
                    return Err(format!(
                        "delivery attempt {} is not newer than {}",
                        attempt, entry.attempt
                    ));
                }
                entry.phase = DeliveryPhase::Claimed;
                entry.outcome = None;
                entry.drained = false;
                entry.reason = None;
                entry.attempt_id = Some(attempt_id.clone());
                entry.attempt = *attempt;
                entry.claimed_at = Some(*claimed_at);
                entry.effect_started_at = None;
                entry.mailbox_accepted_at = None;
                entry.drained_at = None;
                entry.turn_settled_at = None;
                entry.turn_id = None;
                entry.reply_message_id = None;
                entry.next_attempt_at = None;
            }
            DeliveryEvent::EffectStarted {
                message_id,
                attempt_id,
                turn_id,
                started_at,
            } => {
                if turn_id.trim().is_empty() {
                    return Err(format!(
                        "effect for message {message_id} has no turn identity"
                    ));
                }
                let entry = self.entry_mut(message_id)?;
                Self::check_attempt(entry, message_id, attempt_id)?;
                if entry.phase != DeliveryPhase::Claimed || entry.effect_started_at.is_some() {
                    return Err(format!(
                        "message {message_id} cannot start an effect from {:?}",
                        entry.phase
                    ));
                }
                entry.phase = DeliveryPhase::EffectStarted;
                entry.effect_started_at = Some(*started_at);
                entry.turn_id = Some(turn_id.clone());
            }
            DeliveryEvent::MailboxAccepted {
                message_id,
                attempt_id,
                turn_id,
                accepted_at,
            } => {
                let entry = self.entry_mut(message_id)?;
                Self::check_attempt(entry, message_id, attempt_id)?;
                if entry.phase != DeliveryPhase::EffectStarted
                    || entry.turn_id.as_deref() != Some(turn_id)
                {
                    return Err(format!(
                        "message {message_id} mailbox acceptance is out of order"
                    ));
                }
                entry.phase = DeliveryPhase::MailboxAccepted;
                entry.mailbox_accepted_at = Some(*accepted_at);
            }
            DeliveryEvent::Drained {
                message_id,
                attempt_id,
                turn_id,
                drained_at,
            } => {
                let entry = self.entry_mut(message_id)?;
                Self::check_attempt(entry, message_id, attempt_id)?;
                if entry.phase != DeliveryPhase::MailboxAccepted
                    || entry.turn_id.as_deref() != Some(turn_id)
                {
                    return Err(format!("message {message_id} drain is out of order"));
                }
                entry.phase = DeliveryPhase::Drained;
                entry.drained = true;
                entry.drained_at = Some(*drained_at);
            }
            DeliveryEvent::Deferred {
                message_id,
                attempt_id,
                reason,
                deferred_at: _deferred_at,
                next_attempt_at,
            } => {
                let entry = self.entry_mut(message_id)?;
                Self::check_attempt(entry, message_id, attempt_id)?;
                if !matches!(
                    entry.phase,
                    DeliveryPhase::Claimed | DeliveryPhase::EffectStarted
                ) || entry.mailbox_accepted_at.is_some()
                    || entry.drained
                {
                    return Err(format!(
                        "message {message_id} cannot defer from {:?}",
                        entry.phase
                    ));
                }
                entry.phase = DeliveryPhase::Deferred;
                entry.reason = Some(reason.clone());
                entry.next_attempt_at = Some(*next_attempt_at);
                entry.effect_started_at = None;
                entry.mailbox_accepted_at = None;
                entry.drained_at = None;
                entry.turn_id = None;
            }
            DeliveryEvent::TurnSettled {
                message_id,
                attempt_id,
                turn_id,
                outcome,
                drained,
                reason,
                retryable,
                next_attempt_at,
                reply_message_id,
                settled_at,
            } => {
                let retained_bytes = {
                    let entry = self.entry_mut(message_id)?;
                    Self::check_attempt(entry, message_id, attempt_id)?;
                    if entry.terminal
                        || !matches!(
                            entry.phase,
                            DeliveryPhase::Claimed
                                | DeliveryPhase::EffectStarted
                                | DeliveryPhase::MailboxAccepted
                                | DeliveryPhase::Drained
                        )
                    {
                        return Err(format!(
                            "message {message_id} cannot settle from {:?}",
                            entry.phase
                        ));
                    }
                    if let Some(turn_id) = turn_id
                        && entry.turn_id.as_deref() != Some(turn_id)
                    {
                        return Err(format!(
                            "message {message_id} settlement turn identity is stale"
                        ));
                    }
                    entry.outcome = Some(*outcome);
                    entry.drained = drained.unwrap_or(entry.drained);
                    entry.reason = reason.clone();
                    entry.turn_settled_at = Some(*settled_at);
                    entry.reply_message_id = reply_message_id.clone();
                    entry.next_attempt_at = *next_attempt_at;
                    if *retryable {
                        if next_attempt_at.is_none() {
                            return Err(format!(
                                "retryable message {message_id} has no next attempt time"
                            ));
                        }
                        entry.phase = DeliveryPhase::Deferred;
                        entry.effect_started_at = None;
                        entry.mailbox_accepted_at = None;
                        entry.drained_at = None;
                        entry.turn_id = None;
                        return Ok(());
                    }
                    entry.phase = DeliveryPhase::TurnSettled;
                    entry.terminal = true;
                    entry.retained_bytes
                };
                if !self.frontier.iter().any(|id| id == message_id) {
                    return Err(format!(
                        "message {message_id} is missing from delivery frontier"
                    ));
                }
                self.frontier.retain(|id| id != message_id);
                self.terminal_retained_bytes =
                    self.terminal_retained_bytes.saturating_add(retained_bytes);
            }
        }
        Ok(())
    }

    /// Validate projection invariants and logical retention bounds.
    pub fn validate(&self, config: DeliveryLedgerConfig) -> Result<()> {
        if let Some(error) = &self.invalid {
            return Err(ReactError::Other(error.clone()));
        }
        if self.entries.len() != self.order.len() {
            return Err(ReactError::Other(
                "delivery order and entry counts differ".to_string(),
            ));
        }
        let mut ordered = HashSet::new();
        for message_id in &self.order {
            if !ordered.insert(message_id) {
                return Err(ReactError::Other(format!(
                    "delivery order duplicates {message_id}"
                )));
            }
            let entry = self.entries.get(message_id).ok_or_else(|| {
                ReactError::Other(format!("delivery order references unknown {message_id}"))
            })?;
            if entry.message_id != *message_id {
                return Err(ReactError::Other(format!(
                    "delivery order identity disagrees for {message_id}"
                )));
            }
            entry.route.validate()?;
            validate_record_state(entry)?;
        }
        if ordered.len() != self.entries.len() {
            return Err(ReactError::Other(
                "delivery entries contain an id absent from order".to_string(),
            ));
        }

        let mut seen = HashSet::new();
        for message_id in &self.frontier {
            if !seen.insert(message_id) {
                return Err(ReactError::Other(format!(
                    "delivery frontier duplicates {message_id}"
                )));
            }
            let entry = self.entries.get(message_id).ok_or_else(|| {
                ReactError::Other(format!("delivery frontier references unknown {message_id}"))
            })?;
            if entry.terminal {
                return Err(ReactError::Other(format!(
                    "terminal delivery {message_id} remains in frontier"
                )));
            }
        }
        let expected_frontier = self
            .order
            .iter()
            .filter(|message_id| {
                self.entries
                    .get(*message_id)
                    .is_some_and(|entry| !entry.terminal)
            })
            .collect::<Vec<_>>();
        if expected_frontier != self.frontier.iter().collect::<Vec<_>>() {
            return Err(ReactError::Other(
                "delivery frontier order does not match non-terminal order".to_string(),
            ));
        }
        let expected_bytes = self
            .entries
            .values()
            .filter(|entry| entry.terminal)
            .map(|entry| entry.retained_bytes)
            .fold(0_usize, usize::saturating_add);
        if expected_bytes != self.terminal_retained_bytes {
            return Err(ReactError::Other(
                "delivery terminal byte accounting diverged".to_string(),
            ));
        }
        let terminal_count = self.entries.values().filter(|entry| entry.terminal).count();
        if terminal_count > config.terminal_retention
            || self.terminal_retained_bytes > config.terminal_retention_bytes
        {
            return Err(ReactError::Other(
                "delivery terminal retention exceeds configured bounds".to_string(),
            ));
        }
        Ok(())
    }

    /// Return the current FIFO frontier in order.
    pub fn frontier(&self) -> impl Iterator<Item = &DeliveryRecord<Route, Payload>> {
        self.frontier.iter().filter_map(|id| self.entries.get(id))
    }

    /// Return an exact record if its identity is retained.
    pub fn record(&self, message_id: &str) -> Option<&DeliveryRecord<Route, Payload>> {
        self.entries.get(message_id)
    }

    /// Return all retained records in FIFO order.
    pub fn records(&self) -> impl Iterator<Item = &DeliveryRecord<Route, Payload>> {
        self.order.iter().filter_map(|id| self.entries.get(id))
    }
}

fn validate_record_state<Route, Payload>(entry: &DeliveryRecord<Route, Payload>) -> Result<()>
where
    Route: DeliveryRoute,
    Payload: DeliveryPayload,
{
    let invalid = |detail: &str| {
        ReactError::Other(format!(
            "delivery record {} is invalid: {detail}",
            entry.message_id
        ))
    };
    let has_claim = entry.attempt_id.is_some() && entry.claimed_at.is_some();
    if entry.attempt_id.is_some() != entry.claimed_at.is_some() {
        return Err(invalid("attempt identity and claimed_at must be paired"));
    }
    if entry.attempt_id.as_deref().is_some_and(str::is_empty) {
        return Err(invalid("attempt identity is empty"));
    }
    if entry.phase != DeliveryPhase::Persisted && !has_claim {
        return Err(invalid("non-persisted phase has no claim identity"));
    }
    if entry.phase == DeliveryPhase::Persisted && entry.attempt != 0 {
        return Err(invalid("persisted phase has a non-zero attempt"));
    }
    if entry.phase != DeliveryPhase::Persisted && entry.attempt == 0 {
        return Err(invalid("active phase has a zero attempt"));
    }

    match entry.phase {
        DeliveryPhase::Persisted => {
            if entry.terminal
                || entry.outcome.is_some()
                || entry.drained
                || entry.reason.is_some()
                || entry.next_attempt_at.is_some()
                || entry.effect_started_at.is_some()
                || entry.mailbox_accepted_at.is_some()
                || entry.drained_at.is_some()
                || entry.turn_settled_at.is_some()
                || entry.turn_id.is_some()
                || entry.reply_message_id.is_some()
            {
                return Err(invalid("persisted phase contains lifecycle state"));
            }
        }
        DeliveryPhase::Claimed => {
            if entry.terminal
                || entry.outcome.is_some()
                || entry.drained
                || entry.reason.is_some()
                || entry.next_attempt_at.is_some()
                || entry.effect_started_at.is_some()
                || entry.mailbox_accepted_at.is_some()
                || entry.drained_at.is_some()
                || entry.turn_settled_at.is_some()
                || entry.turn_id.is_some()
                || entry.reply_message_id.is_some()
            {
                return Err(invalid("claimed phase contains a later lifecycle state"));
            }
        }
        DeliveryPhase::EffectStarted => {
            if entry.terminal
                || entry.outcome.is_some()
                || entry.drained
                || entry.reason.is_some()
                || entry.next_attempt_at.is_some()
                || entry.effect_started_at.is_none()
                || entry.mailbox_accepted_at.is_some()
                || entry.drained_at.is_some()
                || entry.turn_settled_at.is_some()
                || entry.turn_id.as_deref().is_none_or(str::is_empty)
                || entry.reply_message_id.is_some()
            {
                return Err(invalid(
                    "effect-started phase has inconsistent lifecycle state",
                ));
            }
        }
        DeliveryPhase::MailboxAccepted => {
            if entry.terminal
                || entry.outcome.is_some()
                || entry.drained
                || entry.reason.is_some()
                || entry.next_attempt_at.is_some()
                || entry.effect_started_at.is_none()
                || entry.mailbox_accepted_at.is_none()
                || entry.drained_at.is_some()
                || entry.turn_settled_at.is_some()
                || entry.turn_id.as_deref().is_none_or(str::is_empty)
                || entry.reply_message_id.is_some()
            {
                return Err(invalid(
                    "mailbox-accepted phase has inconsistent lifecycle state",
                ));
            }
        }
        DeliveryPhase::Drained => {
            if entry.terminal
                || entry.outcome.is_some()
                || !entry.drained
                || entry.reason.is_some()
                || entry.next_attempt_at.is_some()
                || entry.effect_started_at.is_none()
                || entry.mailbox_accepted_at.is_none()
                || entry.drained_at.is_none()
                || entry.turn_settled_at.is_some()
                || entry.turn_id.as_deref().is_none_or(str::is_empty)
                || entry.reply_message_id.is_some()
            {
                return Err(invalid("drained phase has inconsistent lifecycle state"));
            }
        }
        DeliveryPhase::Deferred => {
            if entry.terminal
                || entry.next_attempt_at.is_none()
                || entry.effect_started_at.is_some()
                || entry.mailbox_accepted_at.is_some()
                || entry.drained_at.is_some()
                || entry.turn_id.is_some()
            {
                return Err(invalid("deferred phase has inconsistent retry state"));
            }
        }
        DeliveryPhase::TurnSettled => {
            if !entry.terminal
                || entry.outcome.is_none()
                || entry.turn_settled_at.is_none()
                || entry.next_attempt_at.is_some()
            {
                return Err(invalid("settled phase has inconsistent terminal state"));
            }
        }
    }
    if entry.phase == DeliveryPhase::TurnSettled && entry.terminal != (entry.outcome.is_some()) {
        return Err(invalid("terminal flag and outcome disagree"));
    }
    Ok(())
}

/// Generic journal-backed delivery ledger.
///
/// # Example
///
/// A framework consumer keeps its domain route and payload types throughout
/// the lifecycle. The payload only needs `PartialEq`, so ordinary values such
/// as floating-point fields remain usable.
///
/// ```
/// use echo_state::delivery::{
///     DeliveryEnvelope, DeliveryLedger, DeliveryLedgerConfig, DeliveryLedgerProjection,
///     DeliveryOutcome, DeliveryRoute, DeliverySettlement, DeliveryEvent,
/// };
/// use echo_state::journal::{CheckpointStore, MemoryCheckpointStore, MemoryEventJournal};
/// use echo_core::error::{ReactError, Result};
/// use serde::{Deserialize, Serialize};
/// use std::sync::Arc;
///
/// #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
/// struct Route(String);
///
/// impl DeliveryRoute for Route {
///     fn validate(&self) -> Result<()> {
///         if self.0.trim().is_empty() {
///             return Err(ReactError::Other("route is empty".to_string()));
///         }
///         Ok(())
///     }
/// }
///
/// #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
/// struct Payload {
///     text: String,
///     score: f64,
/// }
///
/// # fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
/// type Event = DeliveryEvent<Route, Payload>;
/// type Journal = MemoryEventJournal<Event>;
/// type Projection = DeliveryLedgerProjection<Route, Payload>;
/// type Ledger = DeliveryLedger<Journal, Route, Payload>;
///
/// let journal = Arc::new(Journal::new());
/// let checkpoints: Arc<dyn CheckpointStore<Projection>> =
///     Arc::new(MemoryCheckpointStore::new());
/// let ledger = Ledger::new(
///     journal,
///     checkpoints,
///     DeliveryLedgerConfig::default(),
///     16,
/// );
/// ledger.enqueue(DeliveryEnvelope::new(
///     "message-1",
///     Route("conversation-1".to_string()),
///     Payload { text: "hello".to_string(), score: 0.95 },
/// ))?;
/// let claim = ledger
///     .claim_next()?
///     .ok_or_else(|| ReactError::Other("delivery frontier is empty".to_string()))?;
/// ledger.begin_effect(&claim, "turn-1")?;
/// ledger.accept_mailbox(&claim, "turn-1")?;
/// ledger.mark_drained(&claim, "turn-1")?;
/// ledger.settle(
///     &claim,
///     DeliverySettlement::terminal(
///         Some("turn-1".to_string()),
///         DeliveryOutcome::Completed,
///         Some(true),
///         None,
///         None,
///     ),
/// )?;
/// assert!(ledger.with_projection(|projection| projection.frontier().next().is_none()));
/// # Ok(())
/// # }
/// ```
pub struct DeliveryLedger<J, Route = String, Payload = Value> {
    reducer: CheckpointedReducer<J, DeliveryLedgerProjection<Route, Payload>>,
    journal: Arc<J>,
    config: DeliveryLedgerConfig,
    operation: std::sync::Mutex<()>,
}

/// Errors returned by a delivery ledger before or after journal commit.
#[derive(Debug)]
pub enum DeliveryLedgerError<Route = String, Payload = Value> {
    /// The event violates the current projection and was not persisted.
    InvalidEvent {
        event: Box<DeliveryEvent<Route, Payload>>,
        error: String,
    },
    /// The prepared batch violates the current projection and was not persisted.
    InvalidBatch {
        batch: Box<PreparedJournalBatch<DeliveryEvent<Route, Payload>>>,
        error: String,
    },
    /// The underlying journal/checkpoint authority rejected the append.
    Apply(Box<CheckpointedApplyError<DeliveryEvent<Route, Payload>>>),
    /// A committed receipt did not satisfy projection invariants.
    CommittedInvariant { batch_id: String, error: String },
}

impl<Route, Payload> fmt::Display for DeliveryLedgerError<Route, Payload>
where
    Route: DeliveryRoute,
    Payload: DeliveryPayload,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidEvent { error, .. } => {
                write!(formatter, "invalid delivery event: {error}")
            }
            Self::InvalidBatch { batch, error } => {
                write!(
                    formatter,
                    "invalid delivery batch {}: {error}",
                    batch.batch_id()
                )
            }
            Self::Apply(error) => error.fmt(formatter),
            Self::CommittedInvariant { batch_id, error } => {
                write!(
                    formatter,
                    "committed delivery batch {batch_id} violated invariant: {error}"
                )
            }
        }
    }
}

impl<Route, Payload> std::error::Error for DeliveryLedgerError<Route, Payload>
where
    Route: DeliveryRoute,
    Payload: DeliveryPayload,
{
}

impl<Route, Payload> From<CheckpointedApplyError<DeliveryEvent<Route, Payload>>>
    for DeliveryLedgerError<Route, Payload>
{
    fn from(error: CheckpointedApplyError<DeliveryEvent<Route, Payload>>) -> Self {
        Self::Apply(Box::new(error))
    }
}

impl<J, Route, Payload> fmt::Debug for DeliveryLedger<J, Route, Payload> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeliveryLedger")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl<J, Route, Payload> DeliveryLedger<J, Route, Payload>
where
    Route: DeliveryRoute,
    Payload: DeliveryPayload,
    J: EventJournal<DeliveryEvent<Route, Payload>>,
{
    /// Create a ledger over an existing framework journal and checkpoint store.
    pub fn new(
        journal: Arc<J>,
        checkpoints: Arc<dyn CheckpointStore<DeliveryLedgerProjection<Route, Payload>>>,
        config: DeliveryLedgerConfig,
        checkpoint_every: u64,
    ) -> Self {
        Self {
            reducer: CheckpointedReducer::new(Arc::clone(&journal), checkpoints, checkpoint_every),
            journal,
            config,
            operation: std::sync::Mutex::new(()),
        }
    }

    /// Recover the projection from its checkpoint and journal tail.
    pub fn recover(&self) -> Result<RecoveryReceipt> {
        let _operation = self
            .operation
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let receipt = self.reducer.recover()?;
        self.reducer
            .with_state_mut(|projection| projection.enforce_retention(self.config));
        self.validate()?;
        Ok(receipt)
    }

    /// Enqueue one typed envelope at the current time.
    pub fn enqueue(
        &self,
        envelope: DeliveryEnvelope<Route, Payload>,
    ) -> std::result::Result<ApplyReceipt, DeliveryLedgerError<Route, Payload>> {
        self.apply(self.prepare_enqueue(envelope)?)
    }

    /// Build and preflight an enqueue event without physically committing it.
    pub fn prepare_enqueue(
        &self,
        envelope: DeliveryEnvelope<Route, Payload>,
    ) -> std::result::Result<DeliveryEvent<Route, Payload>, DeliveryLedgerError<Route, Payload>>
    {
        self.prepare_event(DeliveryEvent::Persisted {
            envelope,
            persisted_at: Utc::now(),
        })
    }

    /// Claim the next eligible FIFO delivery and assign its attempt identity.
    pub fn claim_next(
        &self,
    ) -> std::result::Result<
        Option<DeliveryClaim<Route, Payload>>,
        DeliveryLedgerError<Route, Payload>,
    > {
        let Some(draft) = self.prepare_claim_next()? else {
            return Ok(None);
        };
        self.apply(draft.event)?;
        Ok(Some(draft.claim))
    }

    /// Select and preflight the next eligible FIFO claim without committing it.
    pub fn prepare_claim_next(
        &self,
    ) -> std::result::Result<
        Option<DeliveryClaimDraft<Route, Payload>>,
        DeliveryLedgerError<Route, Payload>,
    > {
        let candidate = self.with_projection(|projection| projection.frontier().next().cloned());
        let Some(candidate) = candidate else {
            return Ok(None);
        };
        if candidate.effect_started_at.is_some()
            || matches!(
                candidate.phase,
                DeliveryPhase::EffectStarted
                    | DeliveryPhase::MailboxAccepted
                    | DeliveryPhase::Drained
            )
            || candidate
                .next_attempt_at
                .is_some_and(|deadline| deadline > Utc::now())
        {
            return Ok(None);
        }
        let attempt = candidate.attempt.saturating_add(1);
        let attempt_id = uuid::Uuid::new_v4().to_string();
        let claimed_at = Utc::now();
        let event = self.prepare_event(DeliveryEvent::Claimed {
            message_id: candidate.message_id.clone(),
            attempt_id: attempt_id.clone(),
            attempt,
            claimed_at,
        })?;
        Ok(Some(DeliveryClaimDraft {
            claim: DeliveryClaim {
                payload: candidate.payload,
                message_id: candidate.message_id,
                route: candidate.route,
                attempt_id,
                attempt,
                claimed_at,
            },
            event,
        }))
    }

    /// Record that the claimed effect has started for a concrete turn.
    pub fn begin_effect(
        &self,
        claim: &DeliveryClaim<Route, Payload>,
        turn_id: impl Into<String>,
    ) -> std::result::Result<ApplyReceipt, DeliveryLedgerError<Route, Payload>> {
        self.apply(self.prepare_begin_effect(claim, turn_id)?)
    }

    /// Build and preflight an effect-start event for one exact claim.
    pub fn prepare_begin_effect(
        &self,
        claim: &DeliveryClaim<Route, Payload>,
        turn_id: impl Into<String>,
    ) -> std::result::Result<DeliveryEvent<Route, Payload>, DeliveryLedgerError<Route, Payload>>
    {
        self.prepare_transition(claim, DeliveryTransition::effect_started(turn_id))
    }

    /// Record that the effect's input was accepted by the mailbox.
    pub fn accept_mailbox(
        &self,
        claim: &DeliveryClaim<Route, Payload>,
        turn_id: impl Into<String>,
    ) -> std::result::Result<ApplyReceipt, DeliveryLedgerError<Route, Payload>> {
        self.apply(self.prepare_accept_mailbox(claim, turn_id)?)
    }

    /// Build and preflight a mailbox-accepted event for one exact claim.
    pub fn prepare_accept_mailbox(
        &self,
        claim: &DeliveryClaim<Route, Payload>,
        turn_id: impl Into<String>,
    ) -> std::result::Result<DeliveryEvent<Route, Payload>, DeliveryLedgerError<Route, Payload>>
    {
        self.prepare_transition(claim, DeliveryTransition::mailbox_accepted(turn_id))
    }

    /// Record that the effect's input was consumed by the context.
    pub fn mark_drained(
        &self,
        claim: &DeliveryClaim<Route, Payload>,
        turn_id: impl Into<String>,
    ) -> std::result::Result<ApplyReceipt, DeliveryLedgerError<Route, Payload>> {
        self.apply(self.prepare_mark_drained(claim, turn_id)?)
    }

    /// Build and preflight a drained event for one exact claim.
    pub fn prepare_mark_drained(
        &self,
        claim: &DeliveryClaim<Route, Payload>,
        turn_id: impl Into<String>,
    ) -> std::result::Result<DeliveryEvent<Route, Payload>, DeliveryLedgerError<Route, Payload>>
    {
        self.prepare_transition(claim, DeliveryTransition::drained(turn_id))
    }

    /// Defer an unaccepted effect until the supplied retry time.
    pub fn defer(
        &self,
        claim: &DeliveryClaim<Route, Payload>,
        reason: impl Into<String>,
        next_attempt_at: DateTime<Utc>,
    ) -> std::result::Result<ApplyReceipt, DeliveryLedgerError<Route, Payload>> {
        self.apply(self.prepare_defer(claim, reason, next_attempt_at)?)
    }

    /// Build and preflight a deferred event for one exact claim.
    pub fn prepare_defer(
        &self,
        claim: &DeliveryClaim<Route, Payload>,
        reason: impl Into<String>,
        next_attempt_at: DateTime<Utc>,
    ) -> std::result::Result<DeliveryEvent<Route, Payload>, DeliveryLedgerError<Route, Payload>>
    {
        self.prepare_transition(claim, DeliveryTransition::deferred(reason, next_attempt_at))
    }

    /// Settle one claimed delivery, optionally requeueing it for retry.
    pub fn settle(
        &self,
        claim: &DeliveryClaim<Route, Payload>,
        settlement: DeliverySettlement,
    ) -> std::result::Result<ApplyReceipt, DeliveryLedgerError<Route, Payload>> {
        self.apply(self.prepare_settle(claim, settlement)?)
    }

    /// Build and preflight terminal facts for one exact claim.
    pub fn prepare_settle(
        &self,
        claim: &DeliveryClaim<Route, Payload>,
        settlement: DeliverySettlement,
    ) -> std::result::Result<DeliveryEvent<Route, Payload>, DeliveryLedgerError<Route, Payload>>
    {
        self.prepare_transition(claim, DeliveryTransition::settled(settlement))
    }

    /// Apply one lifecycle transition through the journal authority.
    pub fn transition(
        &self,
        claim: &DeliveryClaim<Route, Payload>,
        transition: DeliveryTransition,
    ) -> std::result::Result<ApplyReceipt, DeliveryLedgerError<Route, Payload>> {
        self.apply(self.prepare_transition(claim, transition)?)
    }

    /// Build and preflight any lifecycle transition for one exact claim.
    pub fn prepare_transition(
        &self,
        claim: &DeliveryClaim<Route, Payload>,
        transition: DeliveryTransition,
    ) -> std::result::Result<DeliveryEvent<Route, Payload>, DeliveryLedgerError<Route, Payload>>
    {
        let event = match transition {
            DeliveryTransition::EffectStarted { turn_id } => DeliveryEvent::EffectStarted {
                message_id: claim.message_id.clone(),
                attempt_id: claim.attempt_id.clone(),
                turn_id,
                started_at: Utc::now(),
            },
            DeliveryTransition::MailboxAccepted { turn_id } => DeliveryEvent::MailboxAccepted {
                message_id: claim.message_id.clone(),
                attempt_id: claim.attempt_id.clone(),
                turn_id,
                accepted_at: Utc::now(),
            },
            DeliveryTransition::Drained { turn_id } => DeliveryEvent::Drained {
                message_id: claim.message_id.clone(),
                attempt_id: claim.attempt_id.clone(),
                turn_id,
                drained_at: Utc::now(),
            },
            DeliveryTransition::Deferred {
                reason,
                next_attempt_at,
            } => DeliveryEvent::Deferred {
                message_id: claim.message_id.clone(),
                attempt_id: claim.attempt_id.clone(),
                reason,
                deferred_at: Utc::now(),
                next_attempt_at,
            },
            DeliveryTransition::Settled { settlement } => DeliveryEvent::TurnSettled {
                message_id: claim.message_id.clone(),
                attempt_id: claim.attempt_id.clone(),
                turn_id: settlement.turn_id,
                outcome: settlement.outcome,
                drained: settlement.drained,
                reason: settlement.reason,
                retryable: settlement.retryable,
                next_attempt_at: settlement.next_attempt_at,
                reply_message_id: settlement.reply_message_id,
                settled_at: Utc::now(),
            },
        };
        self.prepare_event(event)
    }

    fn prepare_event(
        &self,
        event: DeliveryEvent<Route, Payload>,
    ) -> std::result::Result<DeliveryEvent<Route, Payload>, DeliveryLedgerError<Route, Payload>>
    {
        let mut candidate = self.with_projection(Clone::clone);
        candidate.apply(&event);
        candidate.enforce_retention(self.config);
        candidate
            .validate(self.config)
            .map_err(|error| DeliveryLedgerError::InvalidEvent {
                event: Box::new(event.clone()),
                error: error.to_string(),
            })?;
        Ok(event)
    }

    /// Apply one lifecycle event through the journal authority.
    pub fn apply(
        &self,
        event: DeliveryEvent<Route, Payload>,
    ) -> std::result::Result<ApplyReceipt, DeliveryLedgerError<Route, Payload>> {
        let batch = PreparedJournalBatch::new(vec![event]).map_err(|error| {
            DeliveryLedgerError::Apply(Box::new(CheckpointedApplyError::Prepare(error)))
        })?;
        let receipt = self.apply_prepared(batch)?;
        let sequence = receipt.first_sequence;
        if receipt.record_count != 1 || receipt.last_sequence != sequence {
            return Err(DeliveryLedgerError::CommittedInvariant {
                batch_id: receipt.batch_id,
                error: "single delivery event produced a non-single receipt".to_string(),
            });
        }
        Ok(ApplyReceipt {
            batch_id: receipt.batch_id,
            sequence,
            journal: receipt.journal,
            commit: receipt.commit,
            checkpoint: receipt.checkpoint,
        })
    }

    /// Apply a prevalidated batch while retaining its stable identity for
    /// retry and reconciliation by the caller.
    pub fn apply_prepared(
        &self,
        batch: PreparedJournalBatch<DeliveryEvent<Route, Payload>>,
    ) -> std::result::Result<ApplyBatchReceipt, DeliveryLedgerError<Route, Payload>> {
        self.apply_prepared_with(batch, |batch| self.journal.append_batch(batch))
    }

    /// Apply a prepared batch using a caller-owned physical commit authority.
    ///
    /// The callback may provide custom retry, reopen, or durability handling,
    /// but it must return the framework journal receipt for the exact prepared
    /// identity. Projection preflight, folding, checkpointing, retention, and
    /// post-commit validation remain owned by this ledger.
    pub fn apply_prepared_with<F>(
        &self,
        batch: PreparedJournalBatch<DeliveryEvent<Route, Payload>>,
        append: F,
    ) -> std::result::Result<ApplyBatchReceipt, DeliveryLedgerError<Route, Payload>>
    where
        F: FnOnce(
            PreparedJournalBatch<DeliveryEvent<Route, Payload>>,
        ) -> JournalBatchAppendResult<DeliveryEvent<Route, Payload>>,
    {
        let _operation = self
            .operation
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let preflight = self.reducer.with_state(|projection| {
            let mut candidate = projection.clone();
            for event in batch.events() {
                candidate.apply(event.as_ref());
            }
            candidate.enforce_retention(self.config);
            candidate.validate(self.config)
        });
        if let Err(error) = preflight {
            return Err(DeliveryLedgerError::InvalidBatch {
                batch: Box::new(batch),
                error: error.to_string(),
            });
        }
        let expected = batch.clone();
        let appended = append(batch).map_err(|error| {
            DeliveryLedgerError::Apply(Box::new(CheckpointedApplyError::Journal(error)))
        })?;
        if !expected.matches_receipt(&appended).map_err(|error| {
            DeliveryLedgerError::CommittedInvariant {
                batch_id: expected.batch_id().to_string(),
                error: error.to_string(),
            }
        })? {
            return Err(DeliveryLedgerError::CommittedInvariant {
                batch_id: expected.batch_id().to_string(),
                error: "physical journal receipt does not match the prepared delivery batch"
                    .to_string(),
            });
        }
        let receipt = self.reducer.apply_committed(&expected, appended)?;
        self.reducer
            .with_state_mut(|projection| projection.enforce_retention(self.config));
        if let Err(error) = self.validate() {
            return Err(DeliveryLedgerError::CommittedInvariant {
                batch_id: receipt.batch_id.clone(),
                error: error.to_string(),
            });
        }
        Ok(receipt)
    }

    /// Reconcile a prepared identity without writing or invoking durability.
    pub fn lookup_batch(
        &self,
        batch: &PreparedJournalBatch<DeliveryEvent<Route, Payload>>,
    ) -> Result<JournalBatchLookup<DeliveryEvent<Route, Payload>>> {
        self.journal.lookup_batch(batch)
    }

    /// Persist the current projected state so a caller can safely prune old
    /// physical journal segments after logical retention has been applied.
    pub fn checkpoint(&self) -> Result<()> {
        self.reducer.checkpoint()
    }

    /// Read the current projection without exposing reducer internals.
    pub fn with_projection<T>(
        &self,
        operation: impl FnOnce(&DeliveryLedgerProjection<Route, Payload>) -> T,
    ) -> T {
        self.reducer.with_state(operation)
    }

    /// Validate the current projection against configured retention bounds.
    pub fn validate(&self) -> Result<()> {
        self.reducer
            .with_state(|projection| projection.validate(self.config))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::{MemoryCheckpointStore, MemoryEventJournal};

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct TestRoute(String);

    impl DeliveryRoute for TestRoute {
        fn validate(&self) -> Result<()> {
            if self.0.trim().is_empty() {
                return Err(ReactError::Other("test route is empty".to_string()));
            }
            Ok(())
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct TestPayload {
        text: String,
    }

    fn envelope(id: &str) -> DeliveryEnvelope {
        DeliveryEnvelope {
            message_id: id.to_string(),
            route: "opaque-route".to_string(),
            payload: serde_json::json!({"text": id}),
            metadata: BTreeMap::new(),
            correlation_id: Some("correlation".to_string()),
            causation_id: Some("causation".to_string()),
        }
    }

    fn ledger() -> DeliveryLedger<MemoryEventJournal<DeliveryEvent>> {
        DeliveryLedger::new(
            Arc::new(MemoryEventJournal::new()),
            Arc::new(MemoryCheckpointStore::new()),
            DeliveryLedgerConfig::default(),
            2,
        )
    }

    #[test]
    fn typed_ledger_preserves_route_and_payload_without_json_mapping() -> Result<()> {
        type TypedEvent = DeliveryEvent<TestRoute, TestPayload>;
        type TypedJournal = MemoryEventJournal<TypedEvent>;
        type TypedLedger = DeliveryLedger<TypedJournal, TestRoute, TestPayload>;

        let journal = Arc::new(MemoryEventJournal::<TypedEvent>::new());
        let checkpoints = Arc::new(MemoryCheckpointStore::new());
        let ledger = TypedLedger::new(
            Arc::clone(&journal),
            checkpoints
                as Arc<dyn CheckpointStore<DeliveryLedgerProjection<TestRoute, TestPayload>>>,
            DeliveryLedgerConfig::default(),
            0,
        );
        let envelope = DeliveryEnvelope::new(
            "typed-message",
            TestRoute("conversation".to_string()),
            TestPayload {
                text: "hello".to_string(),
            },
        );
        ledger
            .apply(TypedEvent::Persisted {
                envelope,
                persisted_at: Utc::now(),
            })
            .map_err(|error| ReactError::Other(error.to_string()))?;

        ledger.with_projection(|projection| {
            let record = projection.record("typed-message");
            assert_eq!(
                record.map(|record| &record.route),
                Some(&TestRoute("conversation".to_string()))
            );
            assert_eq!(
                record.map(|record| &record.payload),
                Some(&TestPayload {
                    text: "hello".to_string()
                })
            );
        });
        Ok(())
    }

    #[test]
    fn lifecycle_keeps_fifo_and_rejects_stale_claim() -> Result<()> {
        let ledger = ledger();
        let now = Utc::now();
        ledger
            .apply(DeliveryEvent::Persisted {
                envelope: envelope("first"),
                persisted_at: now,
            })
            .map_err(|error| ReactError::Other(error.to_string()))?;
        ledger
            .apply(DeliveryEvent::Persisted {
                envelope: envelope("second"),
                persisted_at: now,
            })
            .map_err(|error| ReactError::Other(error.to_string()))?;
        assert!(
            ledger
                .apply(DeliveryEvent::Claimed {
                    message_id: "second".to_string(),
                    attempt_id: "attempt-2".to_string(),
                    attempt: 1,
                    claimed_at: now,
                })
                .is_err()
        );
        ledger
            .apply(DeliveryEvent::Claimed {
                message_id: "first".to_string(),
                attempt_id: "attempt-1".to_string(),
                attempt: 1,
                claimed_at: now,
            })
            .map_err(|error| ReactError::Other(error.to_string()))?;
        assert!(
            ledger
                .apply(DeliveryEvent::EffectStarted {
                    message_id: "first".to_string(),
                    attempt_id: "stale".to_string(),
                    turn_id: "turn-1".to_string(),
                    started_at: now,
                })
                .is_err()
        );
        let first = ledger.with_projection(|projection| {
            projection
                .frontier()
                .next()
                .map(|entry| entry.message_id.clone())
        });
        assert_eq!(first.as_deref(), Some("first"));
        Ok(())
    }

    #[test]
    fn completed_delivery_leaves_frontier_and_reopens_after_retry() -> Result<()> {
        let ledger = ledger();
        let now = Utc::now();
        ledger
            .apply(DeliveryEvent::Persisted {
                envelope: envelope("message"),
                persisted_at: now,
            })
            .map_err(|error| ReactError::Other(error.to_string()))?;
        ledger
            .apply(DeliveryEvent::Claimed {
                message_id: "message".to_string(),
                attempt_id: "attempt-1".to_string(),
                attempt: 1,
                claimed_at: now,
            })
            .map_err(|error| ReactError::Other(error.to_string()))?;
        ledger
            .apply(DeliveryEvent::TurnSettled {
                message_id: "message".to_string(),
                attempt_id: "attempt-1".to_string(),
                turn_id: None,
                outcome: DeliveryOutcome::Failed,
                drained: Some(false),
                reason: Some("retry".to_string()),
                retryable: true,
                next_attempt_at: Some(now),
                reply_message_id: None,
                settled_at: now,
            })
            .map_err(|error| ReactError::Other(error.to_string()))?;
        assert_eq!(
            ledger.with_projection(|projection| projection.frontier().count()),
            1
        );
        ledger
            .apply(DeliveryEvent::Claimed {
                message_id: "message".to_string(),
                attempt_id: "attempt-2".to_string(),
                attempt: 2,
                claimed_at: now,
            })
            .map_err(|error| ReactError::Other(error.to_string()))?;
        ledger
            .apply(DeliveryEvent::TurnSettled {
                message_id: "message".to_string(),
                attempt_id: "attempt-2".to_string(),
                turn_id: None,
                outcome: DeliveryOutcome::Completed,
                drained: Some(true),
                reason: None,
                retryable: false,
                next_attempt_at: None,
                reply_message_id: Some("reply".to_string()),
                settled_at: now,
            })
            .map_err(|error| ReactError::Other(error.to_string()))?;
        assert_eq!(
            ledger.with_projection(|projection| projection.frontier().count()),
            0
        );
        Ok(())
    }

    #[test]
    fn typed_rejection_can_defer_effect_admission_before_mailbox_acceptance() -> Result<()> {
        let ledger = ledger();
        let now = Utc::now();
        let apply = |event| {
            ledger
                .apply(event)
                .map(|_| ())
                .map_err(|error| ReactError::Other(error.to_string()))
        };
        apply(DeliveryEvent::Persisted {
            envelope: envelope("rejected-effect"),
            persisted_at: now,
        })?;
        apply(DeliveryEvent::Claimed {
            message_id: "rejected-effect".to_string(),
            attempt_id: "attempt-1".to_string(),
            attempt: 1,
            claimed_at: now,
        })?;
        apply(DeliveryEvent::EffectStarted {
            message_id: "rejected-effect".to_string(),
            attempt_id: "attempt-1".to_string(),
            turn_id: "candidate-turn".to_string(),
            started_at: now,
        })?;
        apply(DeliveryEvent::Deferred {
            message_id: "rejected-effect".to_string(),
            attempt_id: "attempt-1".to_string(),
            reason: "typed steer rejection".to_string(),
            deferred_at: now,
            next_attempt_at: now,
        })?;
        let record =
            ledger.with_projection(|projection| projection.record("rejected-effect").cloned());
        let record =
            record.ok_or_else(|| ReactError::Other("deferred record missing".to_string()))?;
        assert_eq!(record.phase, DeliveryPhase::Deferred);
        assert!(record.effect_started_at.is_none());
        assert!(record.turn_id.is_none());
        apply(DeliveryEvent::Claimed {
            message_id: "rejected-effect".to_string(),
            attempt_id: "attempt-2".to_string(),
            attempt: 2,
            claimed_at: now,
        })?;
        let record = ledger
            .with_projection(|projection| projection.record("rejected-effect").cloned())
            .ok_or_else(|| ReactError::Other("reclaimed record missing".to_string()))?;
        assert_eq!(record.phase, DeliveryPhase::Claimed);
        assert_eq!(record.attempt_id.as_deref(), Some("attempt-2"));
        assert_eq!(record.attempt, 2);
        Ok(())
    }

    #[test]
    fn typed_rejection_cannot_defer_after_mailbox_acceptance() -> Result<()> {
        let ledger = ledger();
        let now = Utc::now();
        let apply = |event| {
            ledger
                .apply(event)
                .map(|_| ())
                .map_err(|error| ReactError::Other(error.to_string()))
        };
        apply(DeliveryEvent::Persisted {
            envelope: envelope("accepted-effect"),
            persisted_at: now,
        })?;
        apply(DeliveryEvent::Claimed {
            message_id: "accepted-effect".to_string(),
            attempt_id: "attempt-1".to_string(),
            attempt: 1,
            claimed_at: now,
        })?;
        apply(DeliveryEvent::EffectStarted {
            message_id: "accepted-effect".to_string(),
            attempt_id: "attempt-1".to_string(),
            turn_id: "accepted-turn".to_string(),
            started_at: now,
        })?;
        apply(DeliveryEvent::MailboxAccepted {
            message_id: "accepted-effect".to_string(),
            attempt_id: "attempt-1".to_string(),
            turn_id: "accepted-turn".to_string(),
            accepted_at: now,
        })?;
        let deferred = ledger.apply(DeliveryEvent::Deferred {
            message_id: "accepted-effect".to_string(),
            attempt_id: "attempt-1".to_string(),
            reason: "late typed steer rejection".to_string(),
            deferred_at: now,
            next_attempt_at: now,
        });
        assert!(deferred.is_err());
        let record = ledger
            .with_projection(|projection| projection.record("accepted-effect").cloned())
            .ok_or_else(|| ReactError::Other("accepted record missing".to_string()))?;
        assert_eq!(record.phase, DeliveryPhase::MailboxAccepted);
        assert!(record.mailbox_accepted_at.is_some());
        assert_eq!(record.turn_id.as_deref(), Some("accepted-turn"));
        Ok(())
    }

    #[test]
    fn effect_mailbox_drain_and_settlement_keep_one_attempt_identity() -> Result<()> {
        let ledger = ledger();
        let now = Utc::now();
        let apply = |event| {
            ledger
                .apply(event)
                .map(|_| ())
                .map_err(|error| ReactError::Other(error.to_string()))
        };
        apply(DeliveryEvent::Persisted {
            envelope: envelope("lifecycle"),
            persisted_at: now,
        })?;
        apply(DeliveryEvent::Claimed {
            message_id: "lifecycle".to_string(),
            attempt_id: "attempt-1".to_string(),
            attempt: 1,
            claimed_at: now,
        })?;
        apply(DeliveryEvent::EffectStarted {
            message_id: "lifecycle".to_string(),
            attempt_id: "attempt-1".to_string(),
            turn_id: "turn-1".to_string(),
            started_at: now,
        })?;
        apply(DeliveryEvent::MailboxAccepted {
            message_id: "lifecycle".to_string(),
            attempt_id: "attempt-1".to_string(),
            turn_id: "turn-1".to_string(),
            accepted_at: now,
        })?;
        apply(DeliveryEvent::Drained {
            message_id: "lifecycle".to_string(),
            attempt_id: "attempt-1".to_string(),
            turn_id: "turn-1".to_string(),
            drained_at: now,
        })?;
        apply(DeliveryEvent::TurnSettled {
            message_id: "lifecycle".to_string(),
            attempt_id: "attempt-1".to_string(),
            turn_id: Some("turn-1".to_string()),
            outcome: DeliveryOutcome::Completed,
            drained: Some(true),
            reason: None,
            retryable: false,
            next_attempt_at: None,
            reply_message_id: Some("reply-1".to_string()),
            settled_at: now,
        })?;
        let record = ledger.with_projection(|projection| projection.record("lifecycle").cloned());
        let record =
            record.ok_or_else(|| ReactError::Other("settled record disappeared".to_string()))?;
        assert_eq!(record.phase, DeliveryPhase::TurnSettled);
        assert!(record.terminal);
        assert_eq!(record.attempt_id.as_deref(), Some("attempt-1"));
        assert_eq!(record.turn_id.as_deref(), Some("turn-1"));
        assert_eq!(record.reply_message_id.as_deref(), Some("reply-1"));
        Ok(())
    }

    #[test]
    fn recovery_replays_delivery_projection_from_journal() -> Result<()> {
        let journal = Arc::new(MemoryEventJournal::new());
        let checkpoints = Arc::new(MemoryCheckpointStore::new());
        let ledger = DeliveryLedger::new(
            Arc::clone(&journal),
            Arc::clone(&checkpoints) as Arc<dyn CheckpointStore<DeliveryLedgerProjection>>,
            DeliveryLedgerConfig::default(),
            1,
        );
        let now = Utc::now();
        ledger
            .apply(DeliveryEvent::Persisted {
                envelope: envelope("recover"),
                persisted_at: now,
            })
            .map_err(|error| ReactError::Other(error.to_string()))?;
        let recovered =
            DeliveryLedger::new(journal, checkpoints, DeliveryLedgerConfig::default(), 1);
        let receipt = recovered.recover()?;
        assert_eq!(receipt.last_applied_sequence, 1);
        assert!(recovered.with_projection(|projection| projection.record("recover").is_some()));
        Ok(())
    }

    #[test]
    fn terminal_retention_is_bounded_by_count_and_bytes() -> Result<()> {
        let config = DeliveryLedgerConfig {
            terminal_retention: 1,
            terminal_retention_bytes: 1024,
        };
        let ledger = DeliveryLedger::new(
            Arc::new(MemoryEventJournal::new()),
            Arc::new(MemoryCheckpointStore::new()),
            config,
            0,
        );
        let now = Utc::now();
        for id in ["first", "second"] {
            ledger
                .apply(DeliveryEvent::Persisted {
                    envelope: envelope(id),
                    persisted_at: now,
                })
                .map_err(|error| ReactError::Other(error.to_string()))?;
            ledger
                .apply(DeliveryEvent::Claimed {
                    message_id: id.to_string(),
                    attempt_id: format!("{id}-attempt"),
                    attempt: 1,
                    claimed_at: now,
                })
                .map_err(|error| ReactError::Other(error.to_string()))?;
            ledger
                .apply(DeliveryEvent::TurnSettled {
                    message_id: id.to_string(),
                    attempt_id: format!("{id}-attempt"),
                    turn_id: None,
                    outcome: DeliveryOutcome::Completed,
                    drained: Some(false),
                    reason: None,
                    retryable: false,
                    next_attempt_at: None,
                    reply_message_id: None,
                    settled_at: now,
                })
                .map_err(|error| ReactError::Other(error.to_string()))?;
        }
        let records = ledger.with_projection(|projection| {
            projection
                .records()
                .map(|record| record.message_id.clone())
                .collect::<Vec<_>>()
        });
        assert_eq!(records, vec!["second"]);
        Ok(())
    }

    #[test]
    fn prepared_batch_keeps_one_identity_for_reconciliation() -> Result<()> {
        let journal = Arc::new(MemoryEventJournal::new());
        let checkpoints = Arc::new(MemoryCheckpointStore::new());
        let ledger = DeliveryLedger::new(
            Arc::clone(&journal),
            checkpoints,
            DeliveryLedgerConfig::default(),
            2,
        );
        let now = Utc::now();
        let prepared = PreparedJournalBatch::new(vec![DeliveryEvent::Persisted {
            envelope: envelope("prepared"),
            persisted_at: now,
        }])
        .map_err(|error| ReactError::Other(error.to_string()))?;
        let batch_id = prepared.batch_id().to_string();
        let receipt = ledger
            .apply_prepared(prepared)
            .map_err(|error| ReactError::Other(error.to_string()))?;
        assert_eq!(receipt.batch_id, batch_id);
        assert_eq!(receipt.record_count, 1);
        assert!(ledger.validate().is_ok());
        Ok(())
    }
}
