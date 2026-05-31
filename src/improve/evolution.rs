//! Self-evolution engine — unified entry point for the entire improve loop.
//!
//! One switch to enable everything: eval with LLM grader, A/B comparison,
//! prompt regeneration, HTML report generation, and iterative improvement.
//!
//! # Usage
//!
//! ```rust,ignore
//! let mut evolution = SelfEvolution::new()
//!     .with_agent(agent)
//!     .with_eval_cases(cases)
//!     .with_grader(grader, grading_agent)
//!     .enable(); // ← single switch
//!
//! let result = evolution.run().await;
//! println!("Best score: {:.2} (iteration {})", result.best_score, result.best_iteration);
//! ```

use crate::eval::{EvalCase, generate_html};
use crate::improve::{ImprovementLoop, LoopResult};
use crate::trace::RunStore;
use std::sync::Arc;

/// Unified self-evolution engine. One `.enable()` turns everything on.
///
/// # Usage
///
/// ```rust,ignore
/// let result = SelfEvolution::new()
///     .with_eval_cases(cases)
///     .with_run_store(store)
///     .enable()
///     .run(agent_factory)
///     .await;
/// ```
pub struct SelfEvolution {
    cases: Vec<EvalCase>,
    run_store: Option<Arc<dyn RunStore>>,
    max_iterations: usize,
    report_dir: Option<String>,
    enabled: bool,
}

impl Default for SelfEvolution {
    fn default() -> Self {
        Self::new()
    }
}

impl SelfEvolution {
    pub fn new() -> Self {
        Self {
            cases: Vec::new(),
            run_store: None,
            max_iterations: 5,
            report_dir: None,
            enabled: false,
        }
    }

    pub fn with_eval_cases(mut self, cases: Vec<EvalCase>) -> Self {
        self.cases = cases;
        self
    }
    pub fn with_run_store(mut self, store: Arc<dyn RunStore>) -> Self {
        self.run_store = Some(store);
        self
    }
    pub fn max_iterations(mut self, max: usize) -> Self {
        self.max_iterations = max;
        self
    }
    pub fn with_report_dir(mut self, dir: &str) -> Self {
        self.report_dir = Some(dir.to_string());
        self
    }

    /// Enable self-evolution.
    pub fn enable(mut self) -> Self {
        self.enabled = true;
        self
    }
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Run the full self-evolution pipeline.
    pub async fn run<F>(&self, agent_factory: F) -> Option<LoopResult>
    where
        F: Fn() -> Box<dyn crate::agent::Agent>,
    {
        if !self.enabled || self.cases.is_empty() {
            return None;
        }

        let runner = ImprovementLoop::new();
        let result = runner
            .run(&self.cases, agent_factory, &self.run_store)
            .await;

        if let Some(ref dir) = self.report_dir {
            if let Err(e) = std::fs::create_dir_all(dir) {
                tracing::warn!("Failed to create report directory {dir}: {e}");
            }
            for (i, iter) in result.iterations.iter().enumerate() {
                let html = generate_html(&iter.eval_report, &format!("Iteration {i}"));
                let path = format!("{dir}/iter_{i}.html");
                if let Err(e) = std::fs::write(&path, html) {
                    tracing::warn!("Failed to write iteration report to {path}: {e}");
                }
            }
            if let Some(last) = result.iterations.last() {
                let html = generate_html(&last.eval_report, "Final");
                let path = format!("{dir}/final.html");
                if let Err(e) = std::fs::write(&path, html) {
                    tracing::warn!("Failed to write final report to {path}: {e}");
                }
            }
        }
        Some(result)
    }
}
