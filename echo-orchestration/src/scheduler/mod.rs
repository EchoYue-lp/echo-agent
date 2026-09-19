//! Task scheduler — cron-based and interval-based task scheduling.
//!
//! Provides [`CronTask`] definitions, [`CronTaskStore`] for persistence,
//! and [`SchedulerRunner`] for periodic execution.
//!
//! # Example
//!
//! ```rust,ignore
//! use echo_orchestration::scheduler::{
//!     CronTask, CronTaskStore, OccurrenceFireFn, SchedulerRunner,
//! };
//! use std::sync::Arc;
//! use tokio_util::sync::CancellationToken;
//!
//! let store = CronTaskStore::new();
//! store.add(CronTask::new("daily-report", "0 9 * * *", "Generate daily report")).await?;
//!
//! let cancel = CancellationToken::new();
//! let fire: OccurrenceFireFn = Arc::new(|invocation| {
//!     Box::pin(async move {
//!         // Use occurrence_id as the idempotency key for external effects.
//!         println!("Firing: {}", invocation.occurrence_id);
//!         Ok(format!("Executed: {}", invocation.task.name))
//!     })
//! });
//! let runner = SchedulerRunner::new_with_occurrence_context(store, cancel, fire).await?;
//! runner.spawn();
//! ```

mod cron_task;
mod runner;

pub use cron_task::{CronTask, CronTaskStatus, CronTaskStore};
pub use runner::{
    FireFn, OccurrenceFireFn, SchedulerHandle, SchedulerInvocation, SchedulerRunner,
    SchedulerTrigger,
};
