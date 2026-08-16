//! Same-turn user steering for an active agent run.

use crate::llm::types::Message;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

pub type TurnSteerError = echo_core::agent::AgentSteerError;

#[derive(Default)]
struct ActiveTurn {
    id: String,
    steerable: bool,
    pending: VecDeque<Message>,
}

#[derive(Default)]
pub(crate) struct TurnSteerMailbox {
    active: Mutex<Option<ActiveTurn>>,
}

impl TurnSteerMailbox {
    pub(crate) fn begin(self: &Arc<Self>, turn_id: String) -> ActiveTurnLease {
        if let Ok(mut active) = self.active.lock() {
            *active = Some(ActiveTurn {
                id: turn_id.clone(),
                steerable: false,
                pending: VecDeque::new(),
            });
        }
        ActiveTurnLease {
            mailbox: Arc::clone(self),
            turn_id,
        }
    }

    pub(crate) fn set_steerable(&self, turn_id: &str, steerable: bool) {
        if let Ok(mut active) = self.active.lock()
            && let Some(turn) = active.as_mut()
            && turn.id == turn_id
        {
            turn.steerable = steerable;
        }
    }

    pub(crate) fn steer(
        &self,
        expected_turn_id: Option<&str>,
        message: Message,
    ) -> Result<String, TurnSteerError> {
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
        turn.pending.push_back(message);
        Ok(turn.id.clone())
    }

    pub(crate) fn drain(&self, turn_id: &str) -> Vec<Message> {
        let Ok(mut active) = self.active.lock() else {
            return Vec::new();
        };
        let Some(turn) = active.as_mut() else {
            return Vec::new();
        };
        if turn.id != turn_id {
            return Vec::new();
        }
        turn.pending.drain(..).collect()
    }

    fn finish(&self, turn_id: &str) {
        if let Ok(mut active) = self.active.lock()
            && active.as_ref().is_some_and(|turn| turn.id == turn_id)
        {
            *active = None;
        }
    }
}

pub(crate) struct ActiveTurnLease {
    mailbox: Arc<TurnSteerMailbox>,
    turn_id: String,
}

impl ActiveTurnLease {
    pub(crate) fn set_steerable(&self, steerable: bool) {
        self.mailbox.set_steerable(&self.turn_id, steerable);
    }
}

impl Drop for ActiveTurnLease {
    fn drop(&mut self) {
        self.mailbox.finish(&self.turn_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let drained = mailbox.drain("turn-1");
        assert_eq!(drained.len(), 2);
        assert_eq!(
            drained.first().and_then(|m| m.content.as_text()),
            Some("first".to_string())
        );
        assert_eq!(
            drained.get(1).and_then(|m| m.content.as_text()),
            Some("second".to_string())
        );
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
}
