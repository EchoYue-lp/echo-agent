//! ProgressBridge — connects Agent callbacks to TaskEventBus.
//!
//! Translates `AgentCallback` events (on_iteration, on_tool_start, etc.)
//! into `TaskEvent::Progress` emissions, allowing background task tracking
//! to observe agent execution in real time.

use crate::agent::{AgentCallback, StepType};
use crate::error::ReactError;
use crate::llm::types::Message;
use crate::tasks::{TaskEvent, TaskEventBus, TaskProgress};
use chrono::Utc;
use serde_json::Value;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

/// Bridges agent execution callbacks to a [`TaskEventBus`] as progress events.
///
/// Attach this as a callback before starting execution, then call [`disable()`](Self::disable)
/// and remove it via [`remove_callbacks_by_type_name`](crate::agent::ReactAgent::remove_callbacks_by_type_name)
/// when execution completes.
///
/// # Progress Estimation
///
/// When `max_iterations` is known, progress is calculated linearly.
/// When unknown (0), a diminishing curve is used that asymptotically
/// approaches 95% — ensuring the task never reports "complete" until
/// `on_final_answer` fires.
pub struct ProgressBridge {
    task_id: String,
    event_bus: Arc<TaskEventBus>,
    max_iterations: usize,
    task_start: Instant,
    current_iteration: AtomicUsize,
    enabled: AtomicBool,
}

impl ProgressBridge {
    /// Create a new progress bridge.
    ///
    /// # Parameters
    /// - `task_id`: The task ID to emit progress events for
    /// - `event_bus`: The event bus to emit progress events on
    /// - `max_iterations`: Maximum expected iterations (0 = unknown, use diminishing curve)
    pub fn new(task_id: String, event_bus: Arc<TaskEventBus>, max_iterations: usize) -> Self {
        Self {
            task_id,
            event_bus,
            max_iterations,
            task_start: Instant::now(),
            current_iteration: AtomicUsize::new(0),
            enabled: AtomicBool::new(true),
        }
    }

    /// Stop emitting progress events without removing the callback.
    ///
    /// Useful when execution completes but the callback hasn't been
    /// removed yet — prevents stale progress events.
    pub fn disable(&self) {
        self.enabled.store(false, Ordering::SeqCst);
    }

    fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    fn emit_progress(&self, pct: f64, phase: &str, message: Option<String>) {
        if !self.is_enabled() {
            return;
        }
        let eta = self.estimate_eta(pct);
        let progress = TaskProgress {
            task_id: self.task_id.clone(),
            percentage: pct.clamp(0.0, 100.0),
            current_phase: phase.to_string(),
            phase_index: self.current_iteration.load(Ordering::Relaxed),
            total_phases: self.max_iterations,
            message,
            eta_secs: eta,
            updated_at: Utc::now(),
        };
        self.event_bus.emit(TaskEvent::Progress {
            task_id: self.task_id.clone(),
            progress,
        });
    }

    fn estimate_eta(&self, pct: f64) -> Option<u64> {
        if pct <= 0.0 || pct >= 100.0 {
            return None;
        }
        let elapsed = self.task_start.elapsed().as_secs_f64();
        let total_estimated = elapsed / (pct / 100.0);
        let remaining = total_estimated - elapsed;
        Some(remaining.max(0.0) as u64)
    }
}

impl AgentCallback for ProgressBridge {
    fn on_iteration<'a>(
        &'a self,
        _agent: &'a str,
        iteration: usize,
    ) -> futures::future::BoxFuture<'a, ()> {
        self.current_iteration.store(iteration, Ordering::Relaxed);
        let pct = if self.max_iterations > 0 {
            (iteration as f64 / self.max_iterations as f64) * 100.0
        } else {
            let base = 1.0 - (0.9_f64).powi(iteration as i32);
            (base * 95.0).min(95.0)
        };
        let msg = format!("Iteration {}", iteration + 1);
        self.emit_progress(pct, "thinking", Some(msg));
        Box::pin(async {})
    }

    fn on_think_start<'a>(
        &'a self,
        _agent: &'a str,
        _messages: &'a [Message],
    ) -> futures::future::BoxFuture<'a, ()> {
        Box::pin(async {})
    }

    fn on_think_end<'a>(
        &'a self,
        _agent: &'a str,
        steps: &'a [StepType],
        _prompt_tokens: usize,
        _completion_tokens: usize,
    ) -> futures::future::BoxFuture<'a, ()> {
        let tool_names: Vec<&str> = steps
            .iter()
            .filter_map(|s| match s {
                StepType::Call { function_name, .. } => Some(function_name.as_str()),
                _ => None,
            })
            .collect();
        if !tool_names.is_empty() {
            let iteration = self.current_iteration.load(Ordering::Relaxed);
            let pct = if self.max_iterations > 0 {
                ((iteration as f64 + 0.5) / self.max_iterations as f64) * 100.0
            } else {
                let base = 1.0 - (0.9_f64).powi(iteration as i32);
                (base * 95.0).min(95.0)
            };
            let msg = format!("Planning: {}", tool_names.join(", "));
            self.emit_progress(pct, "thinking", Some(msg));
        }
        Box::pin(async {})
    }

    fn on_tool_start<'a>(
        &'a self,
        _agent: &'a str,
        tool: &'a str,
        _args: &'a Value,
    ) -> futures::future::BoxFuture<'a, ()> {
        let iteration = self.current_iteration.load(Ordering::Relaxed);
        let pct = if self.max_iterations > 0 {
            ((iteration as f64 + 0.7) / self.max_iterations as f64) * 100.0
        } else {
            let base = 1.0 - (0.9_f64).powi(iteration as i32);
            (base * 95.0).min(95.0)
        };
        let msg = format!("Using: {}", tool);
        self.emit_progress(pct, tool, Some(msg));
        Box::pin(async {})
    }

    fn on_tool_end<'a>(
        &'a self,
        _agent: &'a str,
        _tool: &'a str,
        _result: &'a str,
    ) -> futures::future::BoxFuture<'a, ()> {
        Box::pin(async {})
    }

    fn on_tool_error<'a>(
        &'a self,
        _agent: &'a str,
        tool: &'a str,
        _err: &'a ReactError,
    ) -> futures::future::BoxFuture<'a, ()> {
        let iteration = self.current_iteration.load(Ordering::Relaxed);
        let pct = if self.max_iterations > 0 {
            ((iteration as f64 + 0.7) / self.max_iterations as f64) * 100.0
        } else {
            let base = 1.0 - (0.9_f64).powi(iteration as i32);
            (base * 95.0).min(95.0)
        };
        let msg = format!("Tool error: {}", tool);
        self.emit_progress(pct, "error", Some(msg));
        Box::pin(async {})
    }

    fn on_final_answer<'a>(
        &'a self,
        _agent: &'a str,
        _answer: &'a str,
    ) -> futures::future::BoxFuture<'a, ()> {
        self.emit_progress(100.0, "completed", Some("Task completed".to_string()));
        Box::pin(async {})
    }
}
