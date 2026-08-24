//! Same-turn user steering for an active agent run.

use crate::llm::types::Message;
use echo_core::agent::{AgentSteerReceipt, AgentSteerState, AgentSteerTurnOutcome};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

pub type TurnSteerError = echo_core::agent::AgentSteerError;

#[derive(Default)]
struct ActiveTurn {
    id: String,
    incarnation: Arc<()>,
    steerable: bool,
    pending: VecDeque<TrackedSteer>,
    lifecycles: Vec<Arc<SteerLifecycle>>,
}

struct TrackedSteer {
    message: Message,
    lifecycle: Arc<SteerLifecycle>,
}

struct SteerLifecycle {
    state: tokio::sync::watch::Sender<AgentSteerState>,
}

impl SteerLifecycle {
    fn mark_drained(&self) {
        self.state.send_if_modified(|state| {
            if matches!(state, AgentSteerState::Accepted) {
                *state = AgentSteerState::Drained;
                true
            } else {
                false
            }
        });
    }

    fn settle(&self, outcome: AgentSteerTurnOutcome) {
        self.state.send_if_modified(|state| {
            if matches!(state, AgentSteerState::TurnSettled { .. }) {
                false
            } else {
                let drained = state.was_drained();
                *state = AgentSteerState::TurnSettled { outcome, drained };
                true
            }
        });
    }
}

pub(crate) struct SteerDrainBatch {
    pending: Vec<TrackedSteer>,
}

impl SteerDrainBatch {
    pub(crate) fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    pub(crate) fn len(&self) -> usize {
        self.pending.len()
    }

    pub(crate) fn messages(&self) -> impl Iterator<Item = &Message> {
        self.pending.iter().map(|steer| &steer.message)
    }

    /// Publish `Drained` only after the caller has synchronously inserted all
    /// messages into the active turn context.
    pub(crate) fn mark_drained(self) {
        for steer in self.pending {
            steer.lifecycle.mark_drained();
        }
    }
}

#[derive(Default)]
pub(crate) struct TurnSteerMailbox {
    active: Mutex<Option<ActiveTurn>>,
}

impl TurnSteerMailbox {
    pub(crate) fn begin(self: &Arc<Self>, turn_id: String) -> ActiveTurnLease {
        let incarnation = Arc::new(());
        let replaced = if let Ok(mut active) = self.active.lock() {
            active.replace(ActiveTurn {
                id: turn_id.clone(),
                incarnation: incarnation.clone(),
                steerable: false,
                pending: VecDeque::new(),
                lifecycles: Vec::new(),
            })
        } else {
            None
        };
        settle_active_turn(replaced, AgentSteerTurnOutcome::Dropped);
        ActiveTurnLease {
            mailbox: Arc::clone(self),
            turn_id,
            incarnation,
            settled: false,
        }
    }

    pub(crate) fn set_steerable(&self, turn_id: &str, incarnation: &Arc<()>, steerable: bool) {
        if let Ok(mut active) = self.active.lock()
            && let Some(turn) = active.as_mut()
            && turn.id == turn_id
            && Arc::ptr_eq(&turn.incarnation, incarnation)
        {
            turn.steerable = steerable;
        }
    }

    pub(crate) fn steer(
        &self,
        expected_turn_id: Option<&str>,
        message: Message,
    ) -> Result<String, TurnSteerError> {
        self.steer_tracked(expected_turn_id, message)
            .map(|receipt| receipt.turn_id().to_string())
    }

    pub(crate) fn steer_tracked(
        &self,
        expected_turn_id: Option<&str>,
        message: Message,
    ) -> Result<AgentSteerReceipt, TurnSteerError> {
        let has_content = message
            .content
            .as_text()
            .is_some_and(|text| !text.trim().is_empty())
            || message
                .content
                .parts()
                .is_some_and(|parts| !parts.is_empty());
        if !has_content {
            return Err(TurnSteerError::EmptyInput);
        }
        let mut active = self
            .active
            .lock()
            .map_err(|_| TurnSteerError::StateUnavailable)?;
        let turn = active.as_mut().ok_or(TurnSteerError::NoActiveTurn)?;
        if let Some(expected) = expected_turn_id
            && expected != turn.id
        {
            return Err(TurnSteerError::TurnMismatch {
                expected: expected.to_string(),
                actual: turn.id.clone(),
            });
        }
        if !turn.steerable {
            return Err(TurnSteerError::NotSteerable {
                turn_id: turn.id.clone(),
            });
        }
        let steer_id = uuid::Uuid::new_v4().to_string();
        let turn_id = turn.id.clone();
        let (state, receiver) = tokio::sync::watch::channel(AgentSteerState::Accepted);
        let lifecycle = Arc::new(SteerLifecycle { state });
        turn.pending.push_back(TrackedSteer {
            message,
            lifecycle: lifecycle.clone(),
        });
        turn.lifecycles.push(lifecycle);
        Ok(AgentSteerReceipt::new(steer_id, turn_id, receiver))
    }

    pub(crate) fn take_pending(&self, turn_id: &str, incarnation: &Arc<()>) -> SteerDrainBatch {
        let Ok(mut active) = self.active.lock() else {
            return SteerDrainBatch {
                pending: Vec::new(),
            };
        };
        let Some(turn) = active.as_mut() else {
            return SteerDrainBatch {
                pending: Vec::new(),
            };
        };
        if turn.id != turn_id || !Arc::ptr_eq(&turn.incarnation, incarnation) {
            return SteerDrainBatch {
                pending: Vec::new(),
            };
        }
        SteerDrainBatch {
            pending: turn.pending.drain(..).collect(),
        }
    }

    fn finish(&self, turn_id: &str, incarnation: &Arc<()>, outcome: AgentSteerTurnOutcome) {
        let finished = if let Ok(mut active) = self.active.lock()
            && active.as_ref().is_some_and(|turn| {
                turn.id == turn_id && Arc::ptr_eq(&turn.incarnation, incarnation)
            }) {
            active.take()
        } else {
            None
        };
        settle_active_turn(finished, outcome);
    }
}

fn settle_active_turn(active: Option<ActiveTurn>, outcome: AgentSteerTurnOutcome) {
    let Some(active) = active else {
        return;
    };
    for lifecycle in active.lifecycles {
        lifecycle.settle(outcome);
    }
}

pub(crate) struct ActiveTurnLease {
    mailbox: Arc<TurnSteerMailbox>,
    turn_id: String,
    incarnation: Arc<()>,
    settled: bool,
}

impl ActiveTurnLease {
    pub(crate) fn incarnation(&self) -> Arc<()> {
        self.incarnation.clone()
    }

    pub(crate) fn set_steerable(&self, steerable: bool) {
        self.mailbox
            .set_steerable(&self.turn_id, &self.incarnation, steerable);
    }

    pub(crate) fn settle(mut self, outcome: AgentSteerTurnOutcome) {
        self.mailbox
            .finish(&self.turn_id, &self.incarnation, outcome);
        self.settled = true;
    }
}

impl Drop for ActiveTurnLease {
    fn drop(&mut self) {
        if !self.settled {
            self.mailbox.finish(
                &self.turn_id,
                &self.incarnation,
                AgentSteerTurnOutcome::Dropped,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn lease_scopes_active_turn_and_preserves_fifo() {
        let mailbox = Arc::new(TurnSteerMailbox::default());
        let lease = mailbox.begin("turn-1".to_string());
        lease.set_steerable(true);
        assert!(
            mailbox
                .steer(Some("turn-1"), Message::user("first".to_string()))
                .is_ok()
        );
        assert!(
            mailbox
                .steer(Some("turn-1"), Message::user("second".to_string()))
                .is_ok()
        );
        let drained = mailbox.take_pending("turn-1", &lease.incarnation());
        assert_eq!(drained.len(), 2);
        assert_eq!(
            drained
                .messages()
                .next()
                .and_then(|message| message.content.as_text()),
            Some("first".to_string())
        );
        assert_eq!(
            drained
                .messages()
                .nth(1)
                .and_then(|message| message.content.as_text()),
            Some("second".to_string())
        );
        drained.mark_drained();
        drop(lease);
        assert_eq!(
            mailbox.steer(Some("turn-1"), Message::user("late".to_string())),
            Err(TurnSteerError::NoActiveTurn)
        );
    }

    #[test]
    fn rejects_mismatch_and_non_steerable_turns() {
        let mailbox = Arc::new(TurnSteerMailbox::default());
        let lease = mailbox.begin("turn-2".to_string());
        assert!(matches!(
            mailbox.steer(Some("turn-1"), Message::user("x".to_string())),
            Err(TurnSteerError::TurnMismatch { .. })
        ));
        assert!(matches!(
            mailbox.steer(Some("turn-2"), Message::user("x".to_string())),
            Err(TurnSteerError::NotSteerable { .. })
        ));
        drop(lease);
    }

    #[tokio::test]
    async fn tracked_receipt_is_accepted_before_real_drain_then_settles() -> Result<(), String> {
        let mailbox = Arc::new(TurnSteerMailbox::default());
        let lease = mailbox.begin("tracked-turn".to_string());
        lease.set_steerable(true);
        let mut receipt = mailbox
            .steer_tracked(
                Some("tracked-turn"),
                Message::user("tracked input".to_string()),
            )
            .map_err(|error| error.to_string())?;

        assert_eq!(receipt.state(), AgentSteerState::Accepted);
        assert!(!receipt.state().was_drained());
        let drained = mailbox.take_pending("tracked-turn", &lease.incarnation());
        assert_eq!(drained.len(), 1);
        drained.mark_drained();
        assert_eq!(receipt.wait_for_drained().await, AgentSteerState::Drained);

        lease.settle(AgentSteerTurnOutcome::Completed);
        assert_eq!(
            receipt.wait_for_turn_settled().await,
            AgentSteerState::TurnSettled {
                outcome: AgentSteerTurnOutcome::Completed,
                drained: true,
            }
        );
        Ok(())
    }

    #[tokio::test]
    async fn tracked_terminal_before_drain_preserves_non_consumption() -> Result<(), String> {
        let mailbox = Arc::new(TurnSteerMailbox::default());
        let lease = mailbox.begin("cancelled-turn".to_string());
        lease.set_steerable(true);
        let mut receipt = mailbox
            .steer_tracked(
                Some("cancelled-turn"),
                Message::user("not drained".to_string()),
            )
            .map_err(|error| error.to_string())?;

        lease.settle(AgentSteerTurnOutcome::Cancelled);
        let terminal = receipt.wait_for_drained().await;
        assert_eq!(
            terminal,
            AgentSteerState::TurnSettled {
                outcome: AgentSteerTurnOutcome::Cancelled,
                drained: false,
            }
        );
        assert!(!terminal.was_drained());
        Ok(())
    }

    #[tokio::test]
    async fn dropped_owner_terminalizes_every_tracked_input() -> Result<(), String> {
        let mailbox = Arc::new(TurnSteerMailbox::default());
        let lease = mailbox.begin("dropped-turn".to_string());
        lease.set_steerable(true);
        let mut receipt = mailbox
            .steer_tracked(Some("dropped-turn"), Message::user("input".to_string()))
            .map_err(|error| error.to_string())?;
        drop(lease);

        assert_eq!(
            receipt.wait_for_turn_settled().await,
            AgentSteerState::TurnSettled {
                outcome: AgentSteerTurnOutcome::Dropped,
                drained: false,
            }
        );
        Ok(())
    }

    #[tokio::test]
    async fn tracked_concurrent_acceptance_has_unique_receipts_and_one_fifo_drain()
    -> Result<(), String> {
        let mailbox = Arc::new(TurnSteerMailbox::default());
        let lease = mailbox.begin("concurrent-turn".to_string());
        lease.set_steerable(true);
        let mut tasks = Vec::new();
        for index in 0..16_u8 {
            let mailbox = mailbox.clone();
            tasks.push(tokio::spawn(async move {
                mailbox.steer_tracked(
                    Some("concurrent-turn"),
                    Message::user(format!("input-{index}")),
                )
            }));
        }

        let mut receipts = Vec::new();
        for task in tasks {
            receipts.push(
                task.await
                    .map_err(|error| format!("steer task failed: {error}"))?
                    .map_err(|error| error.to_string())?,
            );
        }
        let ids = receipts
            .iter()
            .map(|receipt| receipt.steer_id().to_string())
            .collect::<HashSet<_>>();
        assert_eq!(ids.len(), receipts.len());
        let drained = mailbox.take_pending("concurrent-turn", &lease.incarnation());
        assert_eq!(drained.len(), receipts.len());
        drained.mark_drained();
        assert!(
            receipts
                .iter()
                .all(|receipt| receipt.state() == AgentSteerState::Drained)
        );
        lease.settle(AgentSteerTurnOutcome::Completed);
        assert!(receipts.iter().all(|receipt| {
            receipt.state()
                == AgentSteerState::TurnSettled {
                    outcome: AgentSteerTurnOutcome::Completed,
                    drained: true,
                }
        }));
        Ok(())
    }

    #[tokio::test]
    async fn tracked_same_id_stale_lease_cannot_mutate_new_incarnation() -> Result<(), String> {
        let mailbox = Arc::new(TurnSteerMailbox::default());
        let stale_settle = mailbox.begin("same-turn".to_string());
        stale_settle.set_steerable(true);
        let mut first_receipt = mailbox
            .steer_tracked(
                Some("same-turn"),
                Message::user("first generation".to_string()),
            )
            .map_err(|error| error.to_string())?;

        let stale_drop = mailbox.begin("same-turn".to_string());
        stale_drop.set_steerable(true);
        assert_eq!(
            first_receipt.wait_for_turn_settled().await,
            AgentSteerState::TurnSettled {
                outcome: AgentSteerTurnOutcome::Dropped,
                drained: false,
            }
        );
        stale_settle.set_steerable(false);
        stale_settle.settle(AgentSteerTurnOutcome::Completed);
        let mut second_receipt = mailbox
            .steer_tracked(
                Some("same-turn"),
                Message::user("second generation".to_string()),
            )
            .map_err(|error| error.to_string())?;
        assert_eq!(second_receipt.state(), AgentSteerState::Accepted);

        let current = mailbox.begin("same-turn".to_string());
        current.set_steerable(true);
        assert_eq!(
            second_receipt.wait_for_turn_settled().await,
            AgentSteerState::TurnSettled {
                outcome: AgentSteerTurnOutcome::Dropped,
                drained: false,
            }
        );
        drop(stale_drop);
        let mut current_receipt = mailbox
            .steer_tracked(
                Some("same-turn"),
                Message::user("current generation".to_string()),
            )
            .map_err(|error| error.to_string())?;
        assert_eq!(current_receipt.state(), AgentSteerState::Accepted);
        current.settle(AgentSteerTurnOutcome::Cancelled);
        assert_eq!(
            current_receipt.wait_for_turn_settled().await,
            AgentSteerState::TurnSettled {
                outcome: AgentSteerTurnOutcome::Cancelled,
                drained: false,
            }
        );
        Ok(())
    }
}
