//! Progress tracking for long-running tasks.
//!
//! Provides weighted phase-based progress calculation and real-time
//! progress broadcasting via `tokio::sync::watch` channels.
//!
//! # Example
//!
//! ```rust,ignore
//! use echo_orchestration::tasks::progress::{Phase, PhasePlan, ProgressReporter};
//!
//! let plan = PhasePlan::new(vec![
//!     Phase::new("search", "Search", 2.0),
//!     Phase::new("analyze", "Analyze", 3.0),
//!     Phase::new("report", "Report", 1.0),
//! ]);
//!
//! let mut reporter = ProgressReporter::new("task-1".into(), plan);
//! reporter.enter_phase(0, Some("Starting search".into()));
//! let sub = reporter.subscribe();
//! ```

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::time::Instant;
use tokio::sync::watch;

// ── Phase ──────────────────────────────────────────────────────────

/// A single phase in a long-running pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Phase {
    /// Unique phase identifier.
    pub id: String,
    /// Human-readable phase name.
    pub name: String,
    /// Relative weight for progress calculation (higher = takes longer proportionally).
    pub weight: f64,
    /// Maximum retry attempts for this phase (0 = no retries).
    pub max_retries: u32,
    /// Per-phase timeout in seconds (0 = no timeout).
    pub timeout_secs: u64,
    /// Whether this phase requires human approval before proceeding.
    pub human_checkpoint: bool,
}

impl Phase {
    /// Create a simple phase with just id, name, and weight.
    pub fn new(id: impl Into<String>, name: impl Into<String>, weight: f64) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            weight,
            max_retries: 0,
            timeout_secs: 0,
            human_checkpoint: false,
        }
    }

    /// Set max retries for this phase.
    pub fn with_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }

    /// Set timeout for this phase.
    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }

    /// Mark this phase as requiring human checkpoint.
    pub fn with_human_checkpoint(mut self, checkpoint: bool) -> Self {
        self.human_checkpoint = checkpoint;
        self
    }
}

// ── PhasePlan ──────────────────────────────────────────────────────

/// Ordered list of phases with cumulative weighted progress calculation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhasePlan {
    /// The phases in execution order.
    pub phases: Vec<Phase>,
    /// Pre-computed total weight (sum of all phase weights).
    total_weight: f64,
}

impl PhasePlan {
    /// Create a new phase plan from an ordered list of phases.
    pub fn new(phases: Vec<Phase>) -> Self {
        let total_weight = phases.iter().map(|p| p.weight).sum();
        Self { phases, total_weight }
    }

    /// Calculate overall progress percentage (0.0–100.0) given the current
    /// phase index and the intra-phase progress (0.0–1.0).
    ///
    /// All phases before `current_phase_idx` are considered 100% complete.
    /// The current phase contributes `phase_internal_pct * current_weight`.
    pub fn progress_pct(&self, current_phase_idx: usize, phase_internal_pct: f64) -> f64 {
        if self.total_weight <= 0.0 || self.phases.is_empty() {
            return 0.0;
        }
        let completed_weight: f64 = self.phases[..current_phase_idx.min(self.phases.len())]
            .iter()
            .map(|p| p.weight)
            .sum();
        let current_weight = self
            .phases
            .get(current_phase_idx)
            .map(|p| p.weight)
            .unwrap_or(0.0);
        let partial = current_weight * phase_internal_pct.clamp(0.0, 1.0);
        ((completed_weight + partial) / self.total_weight * 100.0).clamp(0.0, 100.0)
    }

    /// Get the name of the phase at the given index, or `""` if out of bounds.
    pub fn phase_name(&self, idx: usize) -> &str {
        self.phases.get(idx).map(|p| p.name.as_str()).unwrap_or("")
    }

    /// Number of phases.
    pub fn len(&self) -> usize {
        self.phases.len()
    }

    /// Whether the plan has no phases.
    pub fn is_empty(&self) -> bool {
        self.phases.is_empty()
    }
}

// ── TaskProgress ───────────────────────────────────────────────────

/// Real-time progress snapshot for a long-running task.
///
/// Serializable for transmission over SSE, WebSocket, or other channels.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskProgress {
    /// The task this progress belongs to.
    pub task_id: String,
    /// Overall progress percentage (0.0–100.0).
    pub percentage: f64,
    /// Name of the currently executing phase.
    pub current_phase: String,
    /// Zero-based index of the current phase.
    pub phase_index: usize,
    /// Total number of phases in the plan.
    pub total_phases: usize,
    /// Optional phase-internal message (e.g., "Searching papers: 12/20 found").
    pub message: Option<String>,
    /// Estimated seconds remaining, based on elapsed time and progress.
    pub eta_secs: Option<u64>,
    /// Timestamp of this progress update.
    pub updated_at: DateTime<Utc>,
}

// ── ProgressReporter ───────────────────────────────────────────────

/// Broadcasts progress updates to subscribers via a `watch` channel.
///
/// Uses latest-value semantics: subscribers always get the freshest snapshot.
/// This complements the `TaskEventBus` which uses broadcast (all-events) semantics.
pub struct ProgressReporter {
    sender: watch::Sender<TaskProgress>,
    receiver: watch::Receiver<TaskProgress>,
    plan: PhasePlan,
    task_start: Instant,
    phase_start: Instant,
}

impl ProgressReporter {
    /// Create a new reporter for the given task with the given phase plan.
    pub fn new(task_id: String, plan: PhasePlan) -> Self {
        let initial = TaskProgress {
            task_id,
            percentage: 0.0,
            current_phase: plan.phase_name(0).to_string(),
            phase_index: 0,
            total_phases: plan.len(),
            message: None,
            eta_secs: None,
            updated_at: Utc::now(),
        };
        let (sender, receiver) = watch::channel(initial);
        let now = Instant::now();
        Self {
            sender,
            receiver,
            plan,
            task_start: now,
            phase_start: now,
        }
    }

    /// Called when entering a new phase.
    pub fn enter_phase(&mut self, phase_idx: usize, message: Option<String>) {
        self.phase_start = Instant::now();
        let pct = self.plan.progress_pct(phase_idx, 0.0);
        let task_id = self.sender.borrow().task_id.clone();
        let _ = self.sender.send(TaskProgress {
            task_id,
            percentage: pct,
            current_phase: self.plan.phase_name(phase_idx).to_string(),
            phase_index: phase_idx,
            total_phases: self.plan.len(),
            message,
            eta_secs: self.calculate_eta(pct),
            updated_at: Utc::now(),
        });
    }

    /// Called for intra-phase progress updates.
    pub fn update_phase_progress(&self, phase_pct: f64, message: Option<String>) {
        let current = self.sender.borrow();
        let pct = self.plan.progress_pct(current.phase_index, phase_pct);
        let task_id = current.task_id.clone();
        let current_phase = current.current_phase.clone();
        let phase_index = current.phase_index;
        let total_phases = current.total_phases;
        drop(current);
        let _ = self.sender.send(TaskProgress {
            task_id,
            percentage: pct,
            current_phase,
            phase_index,
            total_phases,
            message,
            eta_secs: self.calculate_eta(pct),
            updated_at: Utc::now(),
        });
    }

    /// Subscribe to progress updates (for SSE/WebSocket streaming).
    ///
    /// Returns a `watch::Receiver` that always holds the latest snapshot.
    pub fn subscribe(&self) -> watch::Receiver<TaskProgress> {
        self.receiver.clone()
    }

    /// Get the current progress snapshot.
    pub fn current(&self) -> TaskProgress {
        self.sender.borrow().clone()
    }

    /// Calculate ETA based on elapsed time and progress percentage.
    fn calculate_eta(&self, pct: f64) -> Option<u64> {
        if pct <= 0.0 {
            return None;
        }
        let elapsed = self.task_start.elapsed().as_secs();
        let total_estimated = (elapsed as f64 / pct * 100.0) as u64;
        Some(total_estimated.saturating_sub(elapsed))
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_plan() -> PhasePlan {
        PhasePlan::new(vec![
            Phase::new("search", "Search", 2.0),
            Phase::new("analyze", "Analyze", 3.0),
            Phase::new("report", "Report", 1.0),
        ])
    }

    #[test]
    fn test_phase_plan_progress_at_start() {
        let plan = sample_plan();
        let pct = plan.progress_pct(0, 0.0);
        assert!((pct - 0.0).abs() < 0.01, "Expected 0%, got {pct}%");
    }

    #[test]
    fn test_phase_plan_progress_mid_first_phase() {
        let plan = sample_plan();
        let pct = plan.progress_pct(0, 0.5);
        // phase weight 2.0 * 0.5 / 6.0 = 16.67%
        assert!((pct - 16.67).abs() < 0.5, "Expected ~16.67%, got {pct}%");
    }

    #[test]
    fn test_phase_plan_progress_after_first_phase() {
        let plan = sample_plan();
        // Entering phase 1 means phase 0 is 100% complete
        let pct = plan.progress_pct(1, 0.0);
        // 2.0 / 6.0 = 33.33%
        assert!((pct - 33.33).abs() < 0.5, "Expected ~33.33%, got {pct}%");
    }

    #[test]
    fn test_phase_plan_progress_complete() {
        let plan = sample_plan();
        // All phases done = index past end
        let pct = plan.progress_pct(3, 0.0);
        assert!((pct - 100.0).abs() < 0.01, "Expected 100%, got {pct}%");
    }

    #[test]
    fn test_phase_plan_empty() {
        let plan = PhasePlan::new(vec![]);
        assert_eq!(plan.progress_pct(0, 0.5), 0.0);
        assert!(plan.is_empty());
    }

    #[tokio::test]
    async fn test_progress_reporter_subscribe() {
        let plan = sample_plan();
        let mut reporter = ProgressReporter::new("task-1".into(), plan);
        let mut sub = reporter.subscribe();

        reporter.enter_phase(1, Some("Starting analysis".into()));

        // The subscriber should see the update
        assert!(sub.has_changed().unwrap_or(true));
        let progress = sub.borrow().clone();
        assert_eq!(progress.task_id, "task-1");
        assert_eq!(progress.phase_index, 1);
        assert_eq!(progress.current_phase, "Analyze");
        assert!(progress.percentage > 30.0);
    }
}
