//! Agent turn — formalizes the lifecycle of one user-input-to-answer cycle.
//!
//! A turn progresses through explicit phases: ReceiveInput → Recall → Think → Act → Finalize.
//! Phase transitions can be observed via hooks for debugging, tracing, and self-improvement.

use chrono::{DateTime, Utc};

/// Phases of a single agent turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnPhase {
    /// Input is received but not yet processed (guard checks, UserPromptSubmit hook).
    ReceiveInput,
    /// Long-term memory recall and context assembly.
    Recall,
    /// LLM is thinking (reasoning, planning, deciding which tools to call).
    Think,
    /// Tools are being executed.
    Act,
    /// Final answer is produced, validated, and persisted.
    Finalize,
}

impl TurnPhase {
    /// Human-readable name.
    pub fn name(&self) -> &str {
        match self {
            Self::ReceiveInput => "receive_input",
            Self::Recall => "recall",
            Self::Think => "think",
            Self::Act => "act",
            Self::Finalize => "finalize",
        }
    }

    /// Whether this is a "stable point" suitable for checkpointing.
    pub fn is_checkpoint(&self) -> bool {
        matches!(self, Self::Think | Self::Act | Self::Finalize)
    }
}

/// A single agent turn — one complete input-to-answer lifecycle.
#[derive(Debug, Clone)]
pub struct AgentTurn {
    /// Unique turn identifier.
    pub turn_id: String,
    /// The user's input for this turn.
    pub input: String,
    /// Current phase.
    pub phase: TurnPhase,
    /// Number of think-act iterations so far.
    pub iteration_count: usize,
    /// When the turn started.
    pub started_at: DateTime<Utc>,
}

impl AgentTurn {
    /// Create a new turn in the ReceiveInput phase.
    pub fn new(input: &str) -> Self {
        Self {
            turn_id: format!("turn_{}", uuid::Uuid::new_v4()),
            input: input.to_string(),
            phase: TurnPhase::ReceiveInput,
            iteration_count: 0,
            started_at: Utc::now(),
        }
    }

    /// Advance to a new phase.
    pub fn advance(&mut self, phase: TurnPhase) {
        self.phase = phase;
    }

    /// Record a think-act iteration.
    pub fn record_iteration(&mut self) {
        self.iteration_count += 1;
    }

    /// Total elapsed time since the turn started.
    pub fn elapsed(&self) -> chrono::Duration {
        Utc::now() - self.started_at
    }
}
