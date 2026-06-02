//! Composite task execution — chain heterogeneous steps with sequential or parallel strategy.
//!
//! A [`CompositePlan`] defines an ordered list of steps, each with its own
//! [`TaskExecuteFn`], and executes them either sequentially (passing upstream
//! results between steps) or in parallel.
//!
//! # Example
//!
//! ```rust,ignore
//! use echo_orchestration::tasks::composite::{CompositePlan, CompositeStep, CompositeStrategy, execute_composite};
//! use echo_orchestration::tasks::executor::TaskExecuteFn;
//! use std::sync::Arc;
//!
//! let plan = CompositePlan {
//!     steps: vec![
//!         CompositeStep {
//!             id: "search".into(),
//!             name: "Search Papers".into(),
//!             execute_fn: Arc::new(|ctx| Box::pin(async move {
//!                 Ok(format!("Found papers for: {}", ctx.description))
//!             })),
//!             input_from: vec![],
//!         },
//!         CompositeStep {
//!             id: "summarize".into(),
//!             name: "Summarize".into(),
//!             execute_fn: Arc::new(|ctx| Box::pin(async move {
//!                 let upstream = ctx.upstream_results.iter()
//!                     .map(|(_, r)| r.as_str())
//!                     .collect::<Vec<_>>()
//!                     .join("\n");
//!                 Ok(format!("Summary based on: {upstream}"))
//!             })),
//!             input_from: vec!["search".into()],
//!         },
//!     ],
//!     strategy: CompositeStrategy::Sequential,
//! };
//!
//! let results = execute_composite(plan).await?;
//! ```

use super::executor::{TaskContext, TaskExecuteFn};
use echo_core::error::{ReactError, Result};
use futures::future::BoxFuture;
use std::sync::Arc;

// ── Types ──────────────────────────────────────────────────────────

/// Execution strategy for composite plans.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompositeStrategy {
    /// Execute steps one at a time, passing upstream results to each step.
    Sequential,
    /// Execute all steps concurrently via `tokio::spawn`.
    Parallel,
}

/// A single step in a composite plan.
pub struct CompositeStep {
    /// Unique step identifier (used for `input_from` references).
    pub id: String,
    /// Human-readable step name.
    pub name: String,
    /// The execution function for this step.
    pub execute_fn: TaskExecuteFn,
    /// IDs of steps whose output should be passed as `upstream_results`.
    pub input_from: Vec<String>,
}

/// A plan for executing multiple heterogeneous steps.
pub struct CompositePlan {
    /// Steps in execution order.
    pub steps: Vec<CompositeStep>,
    /// How to execute the steps.
    pub strategy: CompositeStrategy,
}

// ── Execution ──────────────────────────────────────────────────────

/// Execute a composite plan, returning `(step_id, output)` pairs.
///
/// - **Sequential**: Steps run one at a time. Each step receives the outputs
///   of prior steps listed in its `input_from` field as `upstream_results`.
/// - **Parallel**: All steps are spawned concurrently. `input_from` is
///   ignored (no upstream results are available at spawn time).
pub async fn execute_composite(
    plan: CompositePlan,
) -> Result<Vec<(String, String)>> {
    match plan.strategy {
        CompositeStrategy::Sequential => execute_sequential(plan.steps).await,
        CompositeStrategy::Parallel => execute_parallel(plan.steps).await,
    }
}

/// Sequential execution: run each step, collect results, pass upstream.
async fn execute_sequential(
    steps: Vec<CompositeStep>,
) -> Result<Vec<(String, String)>> {
    let mut results: Vec<(String, String)> = Vec::with_capacity(steps.len());

    for step in &steps {
        // Collect upstream results based on input_from
        let upstream: Vec<(String, String)> = step
            .input_from
            .iter()
            .filter_map(|dep_id| {
                results
                    .iter()
                    .find(|(id, _)| id == dep_id)
                    .cloned()
            })
            .collect();

        let ctx = TaskContext::with_upstream(
            step.id.clone(),
            step.name.clone(),
            upstream,
        );

        let output = (step.execute_fn)(ctx).await.map_err(|e| {
            ReactError::Other(format!(
                "Composite step '{}' failed: {e}",
                step.id
            ))
        })?;

        results.push((step.id.clone(), output));
    }

    Ok(results)
}

/// Parallel execution: spawn all steps concurrently, collect results.
async fn execute_parallel(
    steps: Vec<CompositeStep>,
) -> Result<Vec<(String, String)>> {
    let mut handles = Vec::with_capacity(steps.len());

    for step in steps {
        let ctx = TaskContext::new(step.id.clone(), step.name.clone());
        let execute_fn = step.execute_fn.clone();
        let step_id = step.id.clone();

        handles.push(tokio::spawn(async move {
            let output = execute_fn(ctx).await.map_err(|e| {
                ReactError::Other(format!(
                    "Composite step '{step_id}' failed: {e}"
                ))
            })?;
            Ok::<(String, String), ReactError>((step_id, output))
        }));
    }

    let mut results = Vec::with_capacity(handles.len());
    for handle in handles {
        let result = handle
            .await
            .map_err(|e| ReactError::Other(format!("Composite step panicked: {e}")))?;
        results.push(result?);
    }

    Ok(results)
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_step(id: &str, output: &str) -> CompositeStep {
        let output = output.to_string();
        CompositeStep {
            id: id.into(),
            name: id.into(),
            execute_fn: Arc::new(move |_ctx| {
                let output = output.clone();
                Box::pin(async move { Ok(output) })
            }),
            input_from: vec![],
        }
    }

    fn make_upstream_step(id: &str, input_from: Vec<String>) -> CompositeStep {
        let id_owned = id.to_string();
        CompositeStep {
            id: id.into(),
            name: id.into(),
            execute_fn: Arc::new(move |ctx| {
                let id_owned = id_owned.clone();
                let upstream = ctx
                    .upstream_results
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join(",");
                Box::pin(async move {
                    Ok(format!("step_{id_owned} saw: [{upstream}]"))
                })
            }),
            input_from,
        }
    }

    #[tokio::test]
    async fn test_sequential_basic() {
        let plan = CompositePlan {
            steps: vec![
                make_step("a", "result_a"),
                make_step("b", "result_b"),
            ],
            strategy: CompositeStrategy::Sequential,
        };
        let results = execute_composite(plan).await.unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0], ("a".into(), "result_a".into()));
        assert_eq!(results[1], ("b".into(), "result_b".into()));
    }

    #[tokio::test]
    async fn test_sequential_with_upstream() {
        let plan = CompositePlan {
            steps: vec![
                make_step("search", "papers found"),
                make_upstream_step("summarize", vec!["search".into()]),
            ],
            strategy: CompositeStrategy::Sequential,
        };
        let results = execute_composite(plan).await.unwrap();
        assert_eq!(results[1].1, "step_summarize saw: [search=papers found]");
    }

    #[tokio::test]
    async fn test_parallel_basic() {
        let plan = CompositePlan {
            steps: vec![
                make_step("x", "result_x"),
                make_step("y", "result_y"),
            ],
            strategy: CompositeStrategy::Parallel,
        };
        let results = execute_composite(plan).await.unwrap();
        assert_eq!(results.len(), 2);
        // Parallel doesn't guarantee order, but both should be present
        let ids: Vec<&str> = results.iter().map(|(id, _)| id.as_str()).collect();
        assert!(ids.contains(&"x"));
        assert!(ids.contains(&"y"));
    }
}
