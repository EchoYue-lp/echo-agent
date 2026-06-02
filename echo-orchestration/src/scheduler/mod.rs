//! Task scheduler — cron-based and interval-based task scheduling.
//!
//! Provides [`CronTask`] definitions, [`CronTaskStore`] for persistence,
//! and [`SchedulerRunner`] for periodic execution.
//!
//! # Example
//!
//! ```rust,ignore
//! use echo_orchestration::scheduler::{CronTask, CronTaskStore, SchedulerRunner};
//! use tokio_util::sync::CancellationToken;
//!
//! let store = CronTaskStore::new();
//! store.add(CronTask::new("daily-report", "0 9 * * *", "Generate daily report"))?;
//!
//! let cancel = CancellationToken::new();
//! let runner = SchedulerRunner::new(store, cancel, |task| {
//!     Box::pin(async move {
//!         println!("Firing: {}", task.name);
//!         Ok(format!("Executed: {}", task.name))
//!     })
//! });
//! runner.spawn();
//! ```

mod cron_task;
mod runner;

pub use cron_task::{CronTask, CronTaskStatus, CronTaskStore};
pub use runner::{FireFn, SchedulerRunner};
