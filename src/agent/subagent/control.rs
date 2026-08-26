//! Attempt-scoped, in-process control for active subagent dispatches.
//!
//! Durable command identity and recovery belong to framework consumers. This
//! registry only binds an exact execution attempt to its cancellation token and
//! the agent's existing live-steering safe point.

use echo_core::agent::{
    Agent, AgentSteerError, AgentSteerReceipt, AgentSteerState, CancellationToken,
};
use echo_core::llm::types::Message;
use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

use super::types::SubagentStatus;

const SETTLED_RETENTION: usize = 256;

/// Stable identity for one attempt of a logical subagent task.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SubagentAttemptIdentity {
    pub task_id: String,
    pub execution_id: String,
    pub attempt: u32,
}

impl SubagentAttemptIdentity {
    pub fn new(
        task_id: impl Into<String>,
        execution_id: impl Into<String>,
        attempt: u32,
    ) -> Result<Self, SubagentControlError> {
        let identity = Self {
            task_id: task_id.into(),
            execution_id: execution_id.into(),
            attempt,
        };
        identity.validate()?;
        Ok(identity)
    }

    fn validate(&self) -> Result<(), SubagentControlError> {
        if self.task_id.trim().is_empty() {
            return Err(SubagentControlError::InvalidIdentity { field: "task_id" });
        }
        if self.execution_id.trim().is_empty() {
            return Err(SubagentControlError::InvalidIdentity {
                field: "execution_id",
            });
        }
        if self.attempt == 0 {
            return Err(SubagentControlError::InvalidIdentity { field: "attempt" });
        }
        Ok(())
    }

    fn task_attempt(&self) -> (String, u32) {
        (self.task_id.clone(), self.attempt)
    }
}

/// Control phase observed before or while settling an interrupt request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubagentControlPhase {
    Starting,
    Running,
    InterruptRequested,
    Settled,
}

/// Exact-attempt envelope for one live message accepted by the active Agent
/// mailbox. The nested framework receipt is the sole lifecycle authority.
#[derive(Debug, Clone)]
pub struct SubagentMessageReceipt {
    pub execution_id: String,
    pub attempt: u32,
    receipt: AgentSteerReceipt,
}

impl SubagentMessageReceipt {
    /// The framework-owned tracked receipt for this exact message.
    pub fn receipt(&self) -> &AgentSteerReceipt {
        &self.receipt
    }

    /// Consume this delivery and return its framework-owned tracked receipt.
    pub fn into_receipt(self) -> AgentSteerReceipt {
        self.receipt
    }

    /// Wait until the message reaches model context or the owning turn settles.
    pub async fn wait_for_drained(&mut self) -> AgentSteerState {
        self.receipt.wait_for_drained().await
    }

    /// Wait until the owning turn reaches its typed terminal outcome.
    pub async fn wait_for_turn_settled(&mut self) -> AgentSteerState {
        self.receipt.wait_for_turn_settled().await
    }
}

/// Receipt for guidance queued for one exact future attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentGuidanceQueueReceipt {
    pub task_id: String,
    pub attempt: u32,
    pub queued_count: usize,
}

/// Result returned after an exact-attempt interrupt reaches settlement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentInterruptOutcome {
    pub execution_id: String,
    pub attempt: u32,
    pub requested: bool,
    pub settled: bool,
    pub previous_status: SubagentControlPhase,
    pub terminal_status: Option<SubagentStatus>,
}

/// Typed rejection from the process-scoped Subagent control plane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubagentControlError {
    InvalidIdentity {
        field: &'static str,
    },
    EmptyInstruction,
    StateUnavailable,
    DuplicateExecution {
        execution_id: String,
    },
    ExecutionIdentityMismatch {
        expected: String,
        actual: String,
    },
    AttemptAlreadyStarted {
        task_id: String,
        attempt: u32,
    },
    UnknownExecution {
        execution_id: String,
    },
    AttemptMismatch {
        execution_id: String,
        expected: u32,
        actual: u32,
    },
    InterruptPending {
        execution_id: String,
        attempt: u32,
    },
    AttemptSettled {
        execution_id: String,
        attempt: u32,
        status: SubagentStatus,
    },
    Steer(AgentSteerError),
}

impl fmt::Display for SubagentControlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidIdentity { field } => {
                write!(f, "invalid Subagent identity field: {field}")
            }
            Self::EmptyInstruction => f.write_str("Subagent instruction must not be empty"),
            Self::StateUnavailable => f.write_str("Subagent control state is unavailable"),
            Self::DuplicateExecution { execution_id } => {
                write!(f, "Subagent execution already exists: {execution_id}")
            }
            Self::ExecutionIdentityMismatch { expected, actual } => write!(
                f,
                "Subagent execution identity mismatch: expected {expected}, actual {actual}"
            ),
            Self::AttemptAlreadyStarted { task_id, attempt } => {
                write!(
                    f,
                    "Subagent task {task_id} attempt {attempt} already started"
                )
            }
            Self::UnknownExecution { execution_id } => {
                write!(f, "unknown Subagent execution: {execution_id}")
            }
            Self::AttemptMismatch {
                execution_id,
                expected,
                actual,
            } => write!(
                f,
                "Subagent execution {execution_id} attempt mismatch: expected {expected}, actual {actual}"
            ),
            Self::InterruptPending {
                execution_id,
                attempt,
            } => write!(
                f,
                "Subagent execution {execution_id} attempt {attempt} is settling an interrupt"
            ),
            Self::AttemptSettled {
                execution_id,
                attempt,
                status,
            } => write!(
                f,
                "Subagent execution {execution_id} attempt {attempt} already settled as {}",
                status.as_str()
            ),
            Self::Steer(error) => write!(f, "Subagent steering rejected: {error}"),
        }
    }
}

impl std::error::Error for SubagentControlError {}

#[derive(Clone)]
pub(crate) struct SubagentAttemptBinding {
    registry: Arc<SubagentControlRegistry>,
    identity: SubagentAttemptIdentity,
}

impl SubagentAttemptBinding {
    pub(crate) fn identity(&self) -> &SubagentAttemptIdentity {
        &self.identity
    }

    pub(crate) fn attach(
        &self,
        agent: Arc<dyn Agent>,
        turn_id: String,
    ) -> Result<SubagentSteeringLease, SubagentControlError> {
        self.registry.attach(&self.identity, agent, turn_id)?;
        Ok(SubagentSteeringLease {
            binding: self.clone(),
        })
    }
}

pub(crate) struct SubagentAttemptAdmission {
    pub(crate) binding: SubagentAttemptBinding,
    pub(crate) guidance: Vec<String>,
    settled: bool,
}

impl SubagentAttemptAdmission {
    pub(crate) fn settle(mut self, status: SubagentStatus) {
        self.binding.registry.settle(&self.binding.identity, status);
        self.settled = true;
    }
}

impl Drop for SubagentAttemptAdmission {
    fn drop(&mut self) {
        if !self.settled {
            self.binding
                .registry
                .settle(&self.binding.identity, SubagentStatus::Failed);
        }
    }
}

pub(crate) struct SubagentSteeringLease {
    binding: SubagentAttemptBinding,
}

impl Drop for SubagentSteeringLease {
    fn drop(&mut self) {
        self.binding.registry.detach(&self.binding.identity);
    }
}

struct ActiveAttempt {
    identity: SubagentAttemptIdentity,
    cancel: CancellationToken,
    phase: SubagentControlPhase,
    agent: Option<Arc<dyn Agent>>,
    turn_id: Option<String>,
    ready_tx: watch::Sender<bool>,
    settled_tx: watch::Sender<Option<SubagentStatus>>,
}

#[derive(Debug, Clone)]
struct SettledAttempt {
    identity: SubagentAttemptIdentity,
    status: SubagentStatus,
}

#[derive(Default)]
struct ControlState {
    active: HashMap<String, ActiveAttempt>,
    active_by_task_attempt: HashMap<(String, u32), String>,
    queued_guidance: HashMap<(String, u32), VecDeque<String>>,
    settled: HashMap<String, SettledAttempt>,
    settled_by_task_attempt: HashMap<(String, u32), String>,
    settled_order: VecDeque<String>,
}

/// Process-scoped execution control. Durable state remains consumer-owned.
#[derive(Default)]
pub(crate) struct SubagentControlRegistry {
    state: Mutex<ControlState>,
}

impl SubagentControlRegistry {
    pub(crate) fn admit(
        self: &Arc<Self>,
        identity: SubagentAttemptIdentity,
        cancel: CancellationToken,
    ) -> Result<SubagentAttemptAdmission, SubagentControlError> {
        identity.validate()?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| SubagentControlError::StateUnavailable)?;
        if state.active.contains_key(&identity.execution_id)
            || state.settled.contains_key(&identity.execution_id)
        {
            return Err(SubagentControlError::DuplicateExecution {
                execution_id: identity.execution_id,
            });
        }
        let task_attempt = identity.task_attempt();
        if state.active_by_task_attempt.contains_key(&task_attempt)
            || state.settled_by_task_attempt.contains_key(&task_attempt)
        {
            return Err(SubagentControlError::AttemptAlreadyStarted {
                task_id: identity.task_id,
                attempt: identity.attempt,
            });
        }
        let guidance = state
            .queued_guidance
            .remove(&task_attempt)
            .map(VecDeque::into_iter)
            .into_iter()
            .flatten()
            .collect();
        let (ready_tx, _) = watch::channel(false);
        let (settled_tx, _) = watch::channel(None);
        state
            .active_by_task_attempt
            .insert(task_attempt, identity.execution_id.clone());
        state.active.insert(
            identity.execution_id.clone(),
            ActiveAttempt {
                identity: identity.clone(),
                cancel,
                phase: SubagentControlPhase::Starting,
                agent: None,
                turn_id: None,
                ready_tx,
                settled_tx,
            },
        );
        drop(state);
        Ok(SubagentAttemptAdmission {
            binding: SubagentAttemptBinding {
                registry: Arc::clone(self),
                identity,
            },
            guidance,
            settled: false,
        })
    }

    pub(crate) fn queue_guidance(
        &self,
        task_id: &str,
        expected_next_attempt: u32,
        instruction: impl Into<String>,
    ) -> Result<SubagentGuidanceQueueReceipt, SubagentControlError> {
        let task_id = task_id.trim();
        if task_id.is_empty() {
            return Err(SubagentControlError::InvalidIdentity { field: "task_id" });
        }
        if expected_next_attempt == 0 {
            return Err(SubagentControlError::InvalidIdentity { field: "attempt" });
        }
        let instruction = instruction.into();
        if instruction.trim().is_empty() {
            return Err(SubagentControlError::EmptyInstruction);
        }
        let key = (task_id.to_string(), expected_next_attempt);
        let mut state = self
            .state
            .lock()
            .map_err(|_| SubagentControlError::StateUnavailable)?;
        if state.active_by_task_attempt.contains_key(&key)
            || state.settled_by_task_attempt.contains_key(&key)
        {
            return Err(SubagentControlError::AttemptAlreadyStarted {
                task_id: task_id.to_string(),
                attempt: expected_next_attempt,
            });
        }
        let guidance = state.queued_guidance.entry(key).or_default();
        guidance.push_back(instruction);
        Ok(SubagentGuidanceQueueReceipt {
            task_id: task_id.to_string(),
            attempt: expected_next_attempt,
            queued_count: guidance.len(),
        })
    }

    pub(crate) async fn send_message_tracked(
        &self,
        execution_id: &str,
        expected_attempt: u32,
        instruction: impl Into<String>,
    ) -> Result<SubagentMessageReceipt, SubagentControlError> {
        let instruction = instruction.into();
        if instruction.trim().is_empty() {
            return Err(SubagentControlError::EmptyInstruction);
        }
        loop {
            let mut ready_rx = {
                let state = self
                    .state
                    .lock()
                    .map_err(|_| SubagentControlError::StateUnavailable)?;
                let Some(active) = state.active.get(execution_id) else {
                    return Err(self.missing_execution_error(
                        &state,
                        execution_id,
                        expected_attempt,
                    ));
                };
                Self::validate_attempt(&active.identity, expected_attempt)?;
                if active.phase == SubagentControlPhase::InterruptRequested {
                    return Err(SubagentControlError::InterruptPending {
                        execution_id: execution_id.to_string(),
                        attempt: expected_attempt,
                    });
                }
                if let (Some(agent), Some(turn_id)) = (&active.agent, active.turn_id.as_deref()) {
                    let tracked = agent
                        .steer_input_tracked(Some(turn_id), Message::user(instruction.clone()))
                        .map_err(SubagentControlError::Steer)?;
                    return Ok(SubagentMessageReceipt {
                        execution_id: execution_id.to_string(),
                        attempt: expected_attempt,
                        receipt: tracked,
                    });
                }
                active.ready_tx.subscribe()
            };
            if ready_rx.changed().await.is_err() {
                // Settlement drops the readiness sender. Re-read the bounded
                // terminal record and return its exact outcome.
                continue;
            }
        }
    }

    pub(crate) async fn interrupt(
        &self,
        execution_id: &str,
        expected_attempt: u32,
    ) -> Result<SubagentInterruptOutcome, SubagentControlError> {
        let (previous_status, requested, cancel, mut settled_rx) = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| SubagentControlError::StateUnavailable)?;
            if let Some(settled) = state.settled.get(execution_id) {
                Self::validate_attempt(&settled.identity, expected_attempt)?;
                return Ok(SubagentInterruptOutcome {
                    execution_id: execution_id.to_string(),
                    attempt: expected_attempt,
                    requested: false,
                    settled: true,
                    previous_status: SubagentControlPhase::Settled,
                    terminal_status: Some(settled.status),
                });
            }
            let active = state.active.get_mut(execution_id).ok_or_else(|| {
                SubagentControlError::UnknownExecution {
                    execution_id: execution_id.to_string(),
                }
            })?;
            Self::validate_attempt(&active.identity, expected_attempt)?;
            let previous_status = active.phase;
            let requested = active.phase != SubagentControlPhase::InterruptRequested;
            active.phase = SubagentControlPhase::InterruptRequested;
            (
                previous_status,
                requested,
                active.cancel.clone(),
                active.settled_tx.subscribe(),
            )
        };
        cancel.cancel();

        let terminal_status = loop {
            if let Some(status) = *settled_rx.borrow_and_update() {
                break status;
            }
            settled_rx
                .changed()
                .await
                .map_err(|_| SubagentControlError::StateUnavailable)?;
        };
        Ok(SubagentInterruptOutcome {
            execution_id: execution_id.to_string(),
            attempt: expected_attempt,
            requested,
            settled: true,
            previous_status,
            terminal_status: Some(terminal_status),
        })
    }

    fn attach(
        &self,
        identity: &SubagentAttemptIdentity,
        agent: Arc<dyn Agent>,
        turn_id: String,
    ) -> Result<(), SubagentControlError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| SubagentControlError::StateUnavailable)?;
        let active = state
            .active
            .get_mut(&identity.execution_id)
            .ok_or_else(|| SubagentControlError::UnknownExecution {
                execution_id: identity.execution_id.clone(),
            })?;
        Self::validate_attempt(&active.identity, identity.attempt)?;
        active.agent = Some(agent);
        active.turn_id = Some(turn_id);
        active.ready_tx.send_replace(true);
        if active.phase == SubagentControlPhase::Starting {
            active.phase = SubagentControlPhase::Running;
        }
        Ok(())
    }

    fn detach(&self, identity: &SubagentAttemptIdentity) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let Some(active) = state.active.get_mut(&identity.execution_id) else {
            return;
        };
        if active.identity.attempt == identity.attempt {
            active.agent = None;
            active.turn_id = None;
            active.ready_tx.send_replace(false);
            if active.phase == SubagentControlPhase::Running {
                active.phase = SubagentControlPhase::Starting;
            }
        }
    }

    fn settle(&self, identity: &SubagentAttemptIdentity, status: SubagentStatus) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let Some(active) = state.active.remove(&identity.execution_id) else {
            return;
        };
        if active.identity.attempt != identity.attempt {
            state.active.insert(identity.execution_id.clone(), active);
            return;
        }
        state
            .active_by_task_attempt
            .remove(&identity.task_attempt());
        active.settled_tx.send_replace(Some(status));
        state
            .settled_by_task_attempt
            .insert(identity.task_attempt(), identity.execution_id.clone());
        state.settled.insert(
            identity.execution_id.clone(),
            SettledAttempt {
                identity: identity.clone(),
                status,
            },
        );
        state.settled_order.push_back(identity.execution_id.clone());
        while state.settled_order.len() > SETTLED_RETENTION {
            let Some(old_execution_id) = state.settled_order.pop_front() else {
                break;
            };
            if let Some(old) = state.settled.remove(&old_execution_id) {
                state
                    .settled_by_task_attempt
                    .remove(&old.identity.task_attempt());
            }
        }
    }

    fn validate_attempt(
        identity: &SubagentAttemptIdentity,
        expected_attempt: u32,
    ) -> Result<(), SubagentControlError> {
        if identity.attempt != expected_attempt {
            return Err(SubagentControlError::AttemptMismatch {
                execution_id: identity.execution_id.clone(),
                expected: expected_attempt,
                actual: identity.attempt,
            });
        }
        Ok(())
    }

    fn missing_execution_error(
        &self,
        state: &ControlState,
        execution_id: &str,
        expected_attempt: u32,
    ) -> SubagentControlError {
        if let Some(settled) = state.settled.get(execution_id) {
            if settled.identity.attempt != expected_attempt {
                return SubagentControlError::AttemptMismatch {
                    execution_id: execution_id.to_string(),
                    expected: expected_attempt,
                    actual: settled.identity.attempt,
                };
            }
            return SubagentControlError::AttemptSettled {
                execution_id: execution_id.to_string(),
                attempt: expected_attempt,
                status: settled.status,
            };
        }
        SubagentControlError::UnknownExecution {
            execution_id: execution_id.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::agent::AgentEvent;
    use echo_core::agent::AgentSteerTurnOutcome;
    use echo_core::error::Result as AgentResult;
    use futures::future::BoxFuture;
    use futures::stream::BoxStream;

    struct SteerTestAgent {
        active_turn: Mutex<Option<String>>,
        messages: Mutex<Vec<String>>,
        lifecycle: Mutex<Option<watch::Sender<AgentSteerState>>>,
    }

    impl SteerTestAgent {
        fn new(turn_id: &str) -> Self {
            Self {
                active_turn: Mutex::new(Some(turn_id.to_string())),
                messages: Mutex::new(Vec::new()),
                lifecycle: Mutex::new(None),
            }
        }

        fn mark_drained(&self) {
            if let Some(sender) = self
                .lifecycle
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .as_ref()
            {
                let _ = sender.send(AgentSteerState::Drained);
            }
        }

        fn settle(&self, outcome: AgentSteerTurnOutcome) {
            if let Some(sender) = self
                .lifecycle
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .as_ref()
            {
                let _ = sender.send(AgentSteerState::TurnSettled {
                    outcome,
                    drained: matches!(outcome, AgentSteerTurnOutcome::Completed),
                });
            }
        }
    }

    impl Agent for SteerTestAgent {
        fn name(&self) -> &str {
            "steer-test"
        }

        fn model_name(&self) -> &str {
            "test"
        }

        fn system_prompt(&self) -> &str {
            ""
        }

        fn execute<'a>(&'a self, _task: &'a str) -> BoxFuture<'a, AgentResult<String>> {
            Box::pin(async { Ok("done".to_string()) })
        }

        fn execute_stream<'a>(
            &'a self,
            _task: &'a str,
        ) -> BoxFuture<'a, AgentResult<BoxStream<'a, AgentResult<AgentEvent>>>> {
            Box::pin(async {
                Ok(Box::pin(futures::stream::once(async {
                    Ok(AgentEvent::FinalAnswer("done".to_string()))
                }))
                    as BoxStream<'a, AgentResult<AgentEvent>>)
            })
        }

        fn steer_input(
            &self,
            expected_turn_id: Option<&str>,
            message: Message,
        ) -> Result<String, AgentSteerError> {
            let active = self
                .active_turn
                .lock()
                .map_err(|_| AgentSteerError::StateUnavailable)?;
            let actual = active.as_deref().ok_or(AgentSteerError::NoActiveTurn)?;
            if let Some(expected) = expected_turn_id
                && expected != actual
            {
                return Err(AgentSteerError::TurnMismatch {
                    expected: expected.to_string(),
                    actual: actual.to_string(),
                });
            }
            let text = message
                .content
                .as_text()
                .filter(|text| !text.trim().is_empty())
                .ok_or(AgentSteerError::EmptyInput)?;
            self.messages
                .lock()
                .map_err(|_| AgentSteerError::StateUnavailable)?
                .push(text);
            Ok(actual.to_string())
        }

        fn steer_input_tracked(
            &self,
            expected_turn_id: Option<&str>,
            message: Message,
        ) -> Result<AgentSteerReceipt, AgentSteerError> {
            let turn_id = self.steer_input(expected_turn_id, message)?;
            let (sender, receiver) = watch::channel(AgentSteerState::Accepted);
            *self
                .lifecycle
                .lock()
                .map_err(|_| AgentSteerError::StateUnavailable)? = Some(sender);
            Ok(AgentSteerReceipt::new(
                uuid::Uuid::new_v4().to_string(),
                turn_id,
                receiver,
            ))
        }
    }

    #[test]
    fn queued_guidance_is_claimed_once_by_exact_attempt() -> Result<(), SubagentControlError> {
        let registry = Arc::new(SubagentControlRegistry::default());
        let first = registry.queue_guidance("task-1", 2, "inspect the current diff")?;
        let second = registry.queue_guidance("task-1", 2, "run the focused test")?;
        assert_eq!(first.queued_count, 1);
        assert_eq!(second.queued_count, 2);

        let identity = SubagentAttemptIdentity::new("task-1", "execution-2", 2)?;
        let admission = registry.admit(identity, CancellationToken::new())?;
        assert_eq!(
            admission.guidance,
            vec![
                "inspect the current diff".to_string(),
                "run the focused test".to_string()
            ]
        );
        assert!(matches!(
            registry.queue_guidance("task-1", 2, "late"),
            Err(SubagentControlError::AttemptAlreadyStarted { .. })
        ));
        admission.settle(SubagentStatus::Completed);
        Ok(())
    }

    #[test]
    fn invalid_and_empty_inputs_are_rejected() {
        assert!(matches!(
            SubagentAttemptIdentity::new("", "execution", 1),
            Err(SubagentControlError::InvalidIdentity { field: "task_id" })
        ));
        let registry = SubagentControlRegistry::default();
        assert_eq!(
            registry.queue_guidance("task", 1, "  "),
            Err(SubagentControlError::EmptyInstruction)
        );
    }

    #[tokio::test]
    async fn live_message_is_exact_attempt_and_never_crosses_settlement() -> Result<(), String> {
        let registry = Arc::new(SubagentControlRegistry::default());
        let identity = SubagentAttemptIdentity::new("task", "execution-1", 1)
            .map_err(|error| error.to_string())?;
        let admission = registry
            .admit(identity.clone(), CancellationToken::new())
            .map_err(|error| error.to_string())?;
        let concrete = Arc::new(SteerTestAgent::new("turn-1"));
        let agent: Arc<dyn Agent> = concrete.clone();
        let steering = admission
            .binding
            .attach(agent, "turn-1".to_string())
            .map_err(|error| error.to_string())?;

        let mut delivered = registry
            .send_message_tracked("execution-1", 1, "first")
            .await
            .map_err(|error| error.to_string())?;
        assert_eq!(delivered.receipt().turn_id(), "turn-1");
        assert_eq!(delivered.receipt().state(), AgentSteerState::Accepted);
        concrete.mark_drained();
        assert_eq!(delivered.wait_for_drained().await, AgentSteerState::Drained);
        assert!(matches!(
            registry
                .send_message_tracked("execution-1", 2, "stale")
                .await,
            Err(SubagentControlError::AttemptMismatch { .. })
        ));
        let messages = concrete
            .messages
            .lock()
            .map_err(|_| "message state unavailable".to_string())?
            .clone();
        assert_eq!(messages, vec!["first".to_string()]);

        concrete.settle(AgentSteerTurnOutcome::Completed);
        assert_eq!(
            delivered.wait_for_turn_settled().await,
            AgentSteerState::TurnSettled {
                outcome: AgentSteerTurnOutcome::Completed,
                drained: true,
            }
        );
        drop(steering);
        admission.settle(SubagentStatus::Completed);
        assert!(matches!(
            registry
                .send_message_tracked("execution-1", 1, "late")
                .await,
            Err(SubagentControlError::AttemptSettled { .. })
        ));
        assert_eq!(
            concrete
                .messages
                .lock()
                .map_err(|_| "message state unavailable".to_string())?
                .as_slice(),
            ["first".to_string()]
        );
        Ok(())
    }

    #[tokio::test]
    async fn live_message_waits_for_existing_attempt_safe_point() -> Result<(), String> {
        let registry = Arc::new(SubagentControlRegistry::default());
        let identity = SubagentAttemptIdentity::new("task", "execution-starting", 1)
            .map_err(|error| error.to_string())?;
        let admission = registry
            .admit(identity, CancellationToken::new())
            .map_err(|error| error.to_string())?;

        let delivery = registry.send_message_tracked("execution-starting", 1, "wait for ready");
        futures::pin_mut!(delivery);
        assert!(matches!(
            futures::poll!(delivery.as_mut()),
            std::task::Poll::Pending
        ));

        let concrete = Arc::new(SteerTestAgent::new("turn-starting"));
        let agent: Arc<dyn Agent> = concrete.clone();
        let steering = admission
            .binding
            .attach(agent, "turn-starting".to_string())
            .map_err(|error| error.to_string())?;
        let mut delivered = delivery.await.map_err(|error| error.to_string())?;
        assert_eq!(delivered.receipt().turn_id(), "turn-starting");
        concrete.mark_drained();
        assert_eq!(delivered.wait_for_drained().await, AgentSteerState::Drained);
        assert_eq!(
            concrete
                .messages
                .lock()
                .map_err(|_| "message state unavailable".to_string())?
                .as_slice(),
            ["wait for ready".to_string()]
        );
        concrete.settle(AgentSteerTurnOutcome::Completed);
        drop(steering);
        admission.settle(SubagentStatus::Completed);
        Ok(())
    }

    #[tokio::test]
    async fn interrupt_waits_for_exact_attempt_settlement() -> Result<(), String> {
        let registry = Arc::new(SubagentControlRegistry::default());
        let cancel = CancellationToken::new();
        let identity = SubagentAttemptIdentity::new("task", "execution-1", 1)
            .map_err(|error| error.to_string())?;
        let admission = registry
            .admit(identity, cancel.clone())
            .map_err(|error| error.to_string())?;
        let interrupt_registry = registry.clone();
        let interrupt =
            tokio::spawn(async move { interrupt_registry.interrupt("execution-1", 1).await });

        cancel.cancelled().await;
        admission.settle(SubagentStatus::Cancelled);
        let outcome = interrupt
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
        assert!(outcome.requested);
        assert!(outcome.settled);
        assert_eq!(outcome.previous_status, SubagentControlPhase::Starting);
        assert_eq!(outcome.terminal_status, Some(SubagentStatus::Cancelled));

        let replay = registry
            .interrupt("execution-1", 1)
            .await
            .map_err(|error| error.to_string())?;
        assert!(!replay.requested);
        assert!(replay.settled);
        assert_eq!(replay.previous_status, SubagentControlPhase::Settled);
        Ok(())
    }
}
