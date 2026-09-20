use crate::agent::CancellationToken;
use crate::error::{AgentError, ReactError, Result};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::sync::Notify;

struct CloseState {
    accepting: bool,
    next_turn_id: u64,
    turns: HashMap<u64, CancellationToken>,
    unsettled_turns: usize,
}

impl Default for CloseState {
    fn default() -> Self {
        Self {
            accepting: true,
            next_turn_id: 0,
            turns: HashMap::new(),
            unsettled_turns: 0,
        }
    }
}

/// Admission and cancellation owner for turns accepted by one ReactAgent.
///
/// Turn outcomes remain owned by the existing execution path. This authority
/// only fences new work, requests cancellation, and lets `Agent::close` wait
/// until every accepted turn releases its lease.
pub(crate) struct ReactAgentCloseAuthority {
    state: Mutex<CloseState>,
    idle: Notify,
}

impl Default for ReactAgentCloseAuthority {
    fn default() -> Self {
        Self {
            state: Mutex::new(CloseState::default()),
            idle: Notify::new(),
        }
    }
}

impl ReactAgentCloseAuthority {
    fn lock_state(&self) -> MutexGuard<'_, CloseState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn admit(
        self: &Arc<Self>,
        cancel: Option<CancellationToken>,
    ) -> Result<(ReactAgentTurnLease, CancellationToken)> {
        let mut state = self.lock_state();
        if !state.accepting {
            return Err(ReactError::Agent(Box::new(AgentError::Cancelled(
                "Agent is closed to new turns".to_string(),
            ))));
        }
        let next_turn_id = state.next_turn_id.checked_add(1).ok_or_else(|| {
            ReactError::Other("Agent turn admission identity exhausted".to_string())
        })?;
        let turn_id = state.next_turn_id;
        state.next_turn_id = next_turn_id;
        // Never cancel the caller's scope when closing one Agent. A child
        // still observes caller cancellation, while Agent close remains local.
        let cancel = cancel
            .map(|parent| parent.child_token())
            .unwrap_or_default();
        state.turns.insert(turn_id, cancel.clone());
        Ok((
            ReactAgentTurnLease {
                authority: Arc::clone(self),
                turn_id,
                active: true,
                started: false,
            },
            cancel,
        ))
    }

    pub(crate) fn begin_close(&self) {
        let turns = {
            let mut state = self.lock_state();
            state.accepting = false;
            state.turns.values().cloned().collect::<Vec<_>>()
        };
        for cancel in turns {
            cancel.cancel();
        }
    }

    pub(crate) async fn wait_until_idle(&self) -> Result<()> {
        loop {
            let notified = self.idle.notified();
            let unsettled_turns = {
                let state = self.lock_state();
                state.turns.is_empty().then_some(state.unsettled_turns)
            };
            if let Some(unsettled_turns) = unsettled_turns {
                if unsettled_turns > 0 {
                    return Err(ReactError::Other(format!(
                        "Agent close blocked by {unsettled_turns} turn(s) that ended without canonical settlement"
                    )));
                }
                return Ok(());
            }
            notified.await;
        }
    }
}

#[must_use]
pub(crate) struct ReactAgentTurnLease {
    authority: Arc<ReactAgentCloseAuthority>,
    turn_id: u64,
    active: bool,
    started: bool,
}

impl ReactAgentTurnLease {
    /// Mark that the canonical turn authority now owes a terminal settlement.
    pub(crate) fn mark_started(&mut self) {
        self.started = true;
    }

    /// Release the close lease only after canonical terminal settlement.
    pub(crate) fn settle(mut self) {
        self.release(false);
    }

    fn release(&mut self, record_debt: bool) {
        if !self.active {
            return;
        }
        self.active = false;
        let idle = {
            let mut state = self.authority.lock_state();
            state.turns.remove(&self.turn_id);
            if record_debt {
                state.unsettled_turns = state.unsettled_turns.saturating_add(1);
            }
            state.turns.is_empty()
        };
        if idle {
            self.authority.idle.notify_waiters();
        }
    }
}

impl Drop for ReactAgentTurnLease {
    fn drop(&mut self) {
        self.release(self.started);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn close_cancels_only_the_agent_child_scope() -> Result<()> {
        let authority = Arc::new(ReactAgentCloseAuthority::default());
        let parent = CancellationToken::new();
        let (lease, child) = authority.admit(Some(parent.clone()))?;

        authority.begin_close();

        assert!(child.is_cancelled());
        assert!(!parent.is_cancelled());
        lease.settle();
        authority.wait_until_idle().await
    }

    #[tokio::test]
    async fn started_turn_drop_is_reported_as_close_debt() -> Result<()> {
        let authority = Arc::new(ReactAgentCloseAuthority::default());
        let (mut lease, _) = authority.admit(None)?;
        lease.mark_started();
        drop(lease);

        authority.begin_close();
        let error = authority.wait_until_idle().await.err().ok_or_else(|| {
            ReactError::Other("unsettled turn did not block Agent close".to_string())
        })?;
        assert!(error.to_string().contains("without canonical settlement"));
        Ok(())
    }
}
