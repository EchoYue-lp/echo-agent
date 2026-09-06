//! Bounded live event delivery for negotiated streams (supreme plan 05,
//! todo `deliver-events-replay-recovery`).
//!
//! Contract: every committed envelope is delivered live while the ACK
//! window is open. When the outstanding count/bytes bound is reached, the
//! stream sends exactly one [`GapNotification`], pauses live delivery, and
//! the Client recovers through bounded `_echo_agent/run/replay` plus
//! `_echo_agent/event/ack`. Host memory stays bounded because live delivery
//! is admission-controlled — the durable journal is the authority, not the
//! outgoing queue. A single event whose serialized size exceeds the live
//! event bound is announced as a one-sequence gap (it remains in the journal
//! for snapshot/watermark semantics) instead of failing the run.

use agent_client_protocol::{Client, ConnectionTo};
use echo_agent::acp::RunEventObserver;
use echo_agent::agent::EventEnvelope;
use echo_agent::error::{ReactError, Result as EchoResult};
use echo_agent::state::journal::EventJournal;
use echo_sdk_protocol::error::{EchoSdkError, Retryability};
use echo_sdk_protocol::event::{EventAck, EventNotification, GapNotification};
use echo_sdk_protocol::scalar::WireNonZeroU64;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Mutex as AsyncMutex;

use super::wire::{sdk_error, wire_envelope};

/// Reasons sent inside gap notifications. Stable diagnostic strings, not
/// error codes.
pub(crate) const GAP_REASON_ACK_WINDOW: &str = "ack window exceeded; live delivery paused";
pub(crate) const GAP_REASON_EVENT_TOO_LARGE: &str = "event exceeds the live delivery bound";

/// Shared delivery state of one stream.
struct DeliveryInner {
    /// Sent-but-unacknowledged `(sequence, serialized bytes)` FIFO.
    unacked: VecDeque<(u64, usize)>,
    outstanding_bytes: usize,
    paused: bool,
    last_sent_sequence: u64,
    /// Highest committed sequence covered by a live delivery or gap. A gap's
    /// watermark is ACK-able even though its event was not sent live; replay
    /// still resumes from `last_sent_sequence` so no facts are skipped.
    ackable_sequence: u64,
}

impl DeliveryInner {
    fn outstanding(&self) -> usize {
        self.unacked.len()
    }
}

/// Live delivery of one run's committed events, gated by the negotiated ACK
/// window. The observer is registered on the shared run sink, so it observes
/// exactly the envelopes the ledger committed, in order.
pub(crate) struct StreamDelivery {
    state: Arc<super::state::CoreProfileState>,
    run_id: String,
    connection: ConnectionTo<Client>,
    stream_handle: echo_sdk_protocol::handle::WireHandle,
    inner: Mutex<DeliveryInner>,
    send_serial: AsyncMutex<()>,
    max_outstanding: usize,
    max_buffer_bytes: usize,
    max_event_bytes: usize,
}

impl StreamDelivery {
    pub(crate) fn new(
        state: Arc<super::state::CoreProfileState>,
        run_id: &str,
        connection: ConnectionTo<Client>,
        stream_handle: echo_sdk_protocol::handle::WireHandle,
        max_outstanding: usize,
        max_buffer_bytes: usize,
        max_event_bytes: usize,
    ) -> Arc<Self> {
        Arc::new(Self {
            state,
            run_id: run_id.to_string(),
            connection,
            stream_handle,
            inner: Mutex::new(DeliveryInner {
                unacked: VecDeque::new(),
                outstanding_bytes: 0,
                paused: false,
                last_sent_sequence: 0,
                ackable_sequence: 0,
            }),
            send_serial: AsyncMutex::new(()),
            max_outstanding,
            max_buffer_bytes,
            max_event_bytes,
        })
    }

    pub(crate) fn stream_handle(&self) -> &echo_sdk_protocol::handle::WireHandle {
        &self.stream_handle
    }

    pub(crate) fn run_id(&self) -> &str {
        &self.run_id
    }

    /// Acknowledge one contiguous cursor: retire outstanding entries, and —
    /// if delivery was paused — resume by flushing the events the Client has
    /// not yet seen (bounded by the window, from the durable journal).
    pub(crate) async fn acknowledge(self: &Arc<Self>, ack: &EventAck) -> EchoResult<()> {
        let _serial = self.send_serial.lock().await;
        let acked = ack.last_processed_sequence.to_u64().unwrap_or_default();
        let resume_from = {
            let mut inner = self.inner.lock().map_err(|_| lock_poisoned())?;
            if acked > inner.ackable_sequence {
                return Err(ReactError::Other(
                    "acknowledgement is ahead of the last sent sequence".to_string(),
                ));
            }
            let mut released_bytes = 0usize;
            while let Some((sequence, bytes)) = inner.unacked.front() {
                if *sequence > acked {
                    break;
                }
                released_bytes = released_bytes.saturating_add(*bytes);
                inner.unacked.pop_front();
            }
            inner.outstanding_bytes = inner.outstanding_bytes.saturating_sub(released_bytes);
            let resume = inner.paused && inner.outstanding() < self.max_outstanding;
            if resume {
                inner.paused = false;
                Some(inner.last_sent_sequence)
            } else {
                None
            }
        };
        let Some(after_sequence) = resume_from else {
            return Ok(());
        };
        let journal = self
            .state
            .journal(&self.run_id)
            .map_err(|error| ReactError::Other(error.to_string()))?;
        let records = journal
            .replay_after(after_sequence, self.max_outstanding)
            .map_err(|error| ReactError::Other(format!("failed to resume live stream: {error}")))?;
        for record in records {
            let envelope = record.event.as_ref();
            match self.reserve(envelope.sequence, self.envelope_bytes(envelope)?)? {
                Reserve::Deliver => self.send_event(envelope)?,
                Reserve::GapAndPause {
                    from_sequence,
                    to_sequence,
                    snapshot_watermark,
                } => {
                    self.send_gap(
                        from_sequence,
                        to_sequence,
                        snapshot_watermark,
                        GAP_REASON_ACK_WINDOW,
                    )?;
                    break;
                }
                Reserve::SkipOversized => {
                    self.send_gap(
                        envelope.sequence,
                        envelope.sequence,
                        envelope.sequence,
                        GAP_REASON_EVENT_TOO_LARGE,
                    )?;
                    let mut inner = self.inner.lock().map_err(|_| lock_poisoned())?;
                    inner.last_sent_sequence = inner.last_sent_sequence.max(envelope.sequence);
                    inner.ackable_sequence = inner.ackable_sequence.max(envelope.sequence);
                    inner.paused = false;
                }
                Reserve::AlreadyPaused => break,
                Reserve::AlreadySent => continue,
            }
        }
        Ok(())
    }

    fn reserve(&self, sequence: u64, bytes: usize) -> EchoResult<Reserve> {
        let mut inner = self.inner.lock().map_err(|_| lock_poisoned())?;
        if bytes > self.max_event_bytes {
            if inner.paused {
                return Ok(Reserve::AlreadyPaused);
            }
            inner.paused = true;
            inner.ackable_sequence = sequence;
            return Ok(Reserve::SkipOversized);
        }
        if sequence <= inner.last_sent_sequence {
            return Ok(Reserve::AlreadySent);
        }
        if inner.paused {
            return Ok(Reserve::AlreadyPaused);
        }
        let next_outstanding = inner
            .outstanding()
            .checked_add(1)
            .ok_or_else(|| ReactError::Other("live window count overflow".to_string()))?;
        let next_bytes = inner
            .outstanding_bytes
            .checked_add(bytes)
            .ok_or_else(|| ReactError::Other("live window byte accounting overflow".to_string()))?;
        if next_outstanding > self.max_outstanding {
            inner.paused = true;
            inner.ackable_sequence = sequence;
            let from_sequence = inner
                .last_sent_sequence
                .checked_add(1)
                .ok_or_else(|| ReactError::Other("live sequence overflow".to_string()))?;
            return Ok(Reserve::GapAndPause {
                from_sequence,
                to_sequence: sequence.max(from_sequence),
                snapshot_watermark: sequence.max(from_sequence),
            });
        }
        if next_bytes > self.max_buffer_bytes {
            inner.paused = true;
            inner.ackable_sequence = sequence;
            let from_sequence = inner
                .last_sent_sequence
                .checked_add(1)
                .ok_or_else(|| ReactError::Other("live sequence overflow".to_string()))?;
            return Ok(Reserve::GapAndPause {
                from_sequence,
                to_sequence: sequence.max(from_sequence),
                snapshot_watermark: sequence.max(from_sequence),
            });
        }
        inner.unacked.push_back((sequence, bytes));
        inner.outstanding_bytes = next_bytes;
        inner.last_sent_sequence = sequence;
        inner.ackable_sequence = sequence;
        Ok(Reserve::Deliver)
    }

    fn send_event(&self, envelope: &EventEnvelope) -> EchoResult<()> {
        let wire = wire_envelope(envelope).map_err(|error| ReactError::Other(error.message))?;
        let notification = EventNotification {
            stream: self.stream_handle.clone(),
            envelope: wire,
        };
        notification.validate().map_err(|error| {
            ReactError::Other(format!("event notification is invalid: {error}"))
        })?;
        self.connection
            .send_notification(notification)
            .map_err(|error| {
                ReactError::Other(format!("failed to send _echo_agent/event: {error}"))
            })
    }

    fn send_gap(
        &self,
        from_sequence: u64,
        to_sequence: u64,
        snapshot_watermark: u64,
        reason: &str,
    ) -> EchoResult<()> {
        let from = echo_sdk_protocol::scalar::WireNonZeroU64::try_from(from_sequence.to_string())
            .map_err(|_| ReactError::Other("gap sequence must be positive".to_string()))?;
        let to = echo_sdk_protocol::scalar::WireNonZeroU64::try_from(to_sequence.to_string())
            .map_err(|_| ReactError::Other("gap sequence must be positive".to_string()))?;
        let watermark =
            echo_sdk_protocol::scalar::WireNonZeroU64::try_from(snapshot_watermark.to_string())
                .map_err(|_| ReactError::Other("gap watermark must be positive".to_string()))?;
        let notification = GapNotification {
            stream: self.stream_handle.clone(),
            gap: echo_sdk_protocol::event::EventGap {
                from_sequence: from,
                to_sequence: to,
                reason: reason.to_string(),
                snapshot_watermark: watermark,
            },
        };
        notification
            .validate()
            .map_err(|error| ReactError::Other(format!("gap notification is invalid: {error}")))?;
        self.connection
            .send_notification(notification)
            .map_err(|error| ReactError::Other(format!("failed to send _echo_agent/gap: {error}")))
    }
}

enum Reserve {
    Deliver,
    GapAndPause {
        from_sequence: u64,
        to_sequence: u64,
        snapshot_watermark: u64,
    },
    SkipOversized,
    AlreadyPaused,
    AlreadySent,
}

impl StreamDelivery {
    fn envelope_bytes(&self, envelope: &EventEnvelope) -> EchoResult<usize> {
        let wire = wire_envelope(envelope).map_err(|error| ReactError::Other(error.message))?;
        serde_json::to_vec(&EventNotification {
            stream: self.stream_handle.clone(),
            envelope: wire,
        })
        .map(|bytes| bytes.len())
        .map_err(|error| ReactError::Other(format!("event notification is not encodable: {error}")))
    }
}

fn lock_poisoned() -> ReactError {
    ReactError::Other("stream delivery state lock poisoned".to_string())
}

#[async_trait::async_trait]
impl RunEventObserver for StreamDelivery {
    async fn on_committed_event(&self, envelope: &EventEnvelope) -> EchoResult<()> {
        let _serial = self.send_serial.lock().await;
        let sequence = envelope.sequence;
        let bytes = self.envelope_bytes(envelope)?;
        match self.reserve(sequence, bytes)? {
            Reserve::Deliver => self.send_event(envelope),
            Reserve::SkipOversized => {
                // One-sequence gap: the event is committed durably but never
                // delivered live. The snapshot watermark equals the missing
                // sequence, so the Client knows to query, not to retry live.
                self.send_gap(sequence, sequence, sequence, GAP_REASON_EVENT_TOO_LARGE)
            }
            Reserve::GapAndPause {
                from_sequence,
                to_sequence,
                snapshot_watermark,
            } => self.send_gap(
                from_sequence,
                to_sequence,
                snapshot_watermark,
                GAP_REASON_ACK_WINDOW,
            ),
            Reserve::AlreadyPaused | Reserve::AlreadySent => Ok(()),
        }
    }
}

/// Bounded wait helper shared by `run/wait`.
pub(crate) async fn wait_with_timeout<T>(
    future: impl std::future::Future<Output = T>,
    timeout: Option<Duration>,
) -> Option<T> {
    match timeout {
        Some(bound) => tokio::time::timeout(bound, future).await.ok(),
        None => Some(future.await),
    }
}

/// Bounded journal replay with typed gap (todo `deliver-events-replay-
/// recovery` step 2). Reads strictly after `after_sequence` from the
/// retained floor, validates journal/envelope sequence alignment, and
/// enforces the event and byte bounds. The response is validated with the
/// contract's own `ReplayResponse::validate` before it is returned.
pub(crate) fn bounded_replay(
    limits_max_replay_bytes: usize,
    journal: &echo_agent::state::journal::SegmentedFileEventJournal<EventEnvelope>,
    stream: &echo_sdk_protocol::handle::WireHandle,
    after_sequence: u64,
    max_events: usize,
) -> std::result::Result<echo_sdk_protocol::event::ReplayResponse, EchoSdkError> {
    use echo_sdk_protocol::{error::ExtensionErrorCode, event::EventCursor};

    let operation = "_echo_agent/run/replay";
    let floor = journal.retention_metadata().retained_floor;
    let next_sequence = after_sequence.saturating_add(1);
    let gap = (after_sequence < u64::MAX && next_sequence < floor)
        .then(|| {
            let missing_to = floor.saturating_sub(1).max(next_sequence);
            Ok(echo_sdk_protocol::event::EventGap {
                from_sequence: nonzero(next_sequence)?,
                to_sequence: nonzero(missing_to)?,
                reason: "replay below the journal retention floor".to_string(),
                snapshot_watermark: nonzero(missing_to)?,
            })
        })
        .transpose()
        .map_err(|error: ReactError| {
            sdk_error(
                ExtensionErrorCode::SerializationViolation,
                error.to_string(),
                Retryability::Never,
                "_echo_agent/run/replay",
            )
        })?;
    let read_from = gap.as_ref().map_or(after_sequence, |gap| {
        gap.to_sequence.to_u64().unwrap_or_default()
    });
    let records = journal
        .replay_after(read_from, max_events)
        .map_err(|error| {
            sdk_error(
                ExtensionErrorCode::ReplayUnavailable,
                format!("journal replay failed: {error}"),
                Retryability::AfterDelay,
                operation,
            )
        })?;
    let mut events = Vec::with_capacity(records.len());
    let mut bytes = 0usize;
    let mut last_sequence = read_from;
    for record in records {
        let envelope = record.event.as_ref().clone();
        if record.sequence != envelope.sequence {
            return Err(sdk_error(
                ExtensionErrorCode::SerializationViolation,
                format!(
                    "journal sequence {} does not align with envelope sequence {}",
                    record.sequence, envelope.sequence
                ),
                Retryability::Never,
                operation,
            ));
        }
        let wire_event = wire_envelope(&envelope).map_err(|error| {
            sdk_error(
                ExtensionErrorCode::SerializationViolation,
                error.message,
                Retryability::Never,
                operation,
            )
        })?;
        let envelope_size = serde_json::to_vec(&wire_event)
            .map(|bytes| bytes.len())
            .map_err(|error| {
                sdk_error(
                    ExtensionErrorCode::SerializationViolation,
                    error.to_string(),
                    Retryability::Never,
                    operation,
                )
            })?;
        let next_bytes = bytes.checked_add(envelope_size).ok_or_else(|| {
            sdk_error(
                ExtensionErrorCode::PayloadTooLarge,
                "replay byte accounting overflow",
                Retryability::Never,
                operation,
            )
        })?;
        if events.len() >= max_events || next_bytes > limits_max_replay_bytes {
            if events.is_empty() {
                return Err(sdk_error(
                    ExtensionErrorCode::PayloadTooLarge,
                    "first replay event exceeds max_replay_bytes",
                    Retryability::Never,
                    operation,
                ));
            }
            break;
        }
        bytes = next_bytes;
        last_sequence = envelope.sequence;
        events.push(wire_event);
    }
    // Contract: with a gap and no deliverable events, the cursor equals the
    // gap watermark; otherwise it is the last delivered sequence.
    let cursor_sequence = match &gap {
        Some(gap) => events
            .last()
            .and_then(|event| event.sequence.to_u64())
            .unwrap_or_else(|| gap.snapshot_watermark.to_u64().unwrap_or_default()),
        None => last_sequence,
    };
    let response = echo_sdk_protocol::event::ReplayResponse {
        requested_after_sequence: echo_sdk_protocol::scalar::WireU64::from_u64(after_sequence),
        events,
        next_cursor: EventCursor {
            stream_id: stream.id.clone(),
            last_processed_sequence: echo_sdk_protocol::scalar::WireU64::from_u64(cursor_sequence),
        },
        gap,
    };
    response.validate().map_err(|error| {
        sdk_error(
            ExtensionErrorCode::SerializationViolation,
            format!("replay response failed contract validation: {error}"),
            Retryability::Never,
            operation,
        )
    })?;
    Ok(response)
}

/// Positive sequences are the only legal gap components, and the non-zero
/// scalar contract accepts exactly their decimal strings; failures become
/// typed errors instead of panicking in the delivery path.
fn nonzero(value: u64) -> EchoResult<WireNonZeroU64> {
    WireNonZeroU64::try_from(value.max(1).to_string())
        .map_err(|_| ReactError::Other("gap sequence must be a positive integer".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gap_reasons_are_stable_strings() {
        assert!(!GAP_REASON_ACK_WINDOW.is_empty());
        assert!(!GAP_REASON_EVENT_TOO_LARGE.is_empty());
    }
}
