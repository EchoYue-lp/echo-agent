//! Traits, generics, trait objects, and the builder pattern.

use crate::basics::{LearningTask, TaskState};
use crate::errors::LearningError;

pub trait TaskFormatter: Send + Sync {
    fn format(&self, task: &LearningTask) -> String;
}

#[derive(Debug, Default)]
pub struct PlainFormatter;

impl TaskFormatter for PlainFormatter {
    fn format(&self, task: &LearningTask) -> String {
        format!("task: {}", task.title)
    }
}

#[derive(Debug, Default)]
pub struct StatusFormatter;

impl TaskFormatter for StatusFormatter {
    fn format(&self, task: &LearningTask) -> String {
        let status = match task.state {
            TaskState::Pending => "pending",
            TaskState::Running => "running",
            TaskState::Completed { .. } => "completed",
        };
        format!("{} [{status}]", task.title)
    }
}

/// Static dispatch: the compiler creates code for the concrete formatter type.
pub fn format_static<F: TaskFormatter>(formatter: &F, task: &LearningTask) -> String {
    formatter.format(task)
}

/// Dynamic dispatch: different formatter implementations share one collection.
pub fn format_dynamic(formatters: &[Box<dyn TaskFormatter>], task: &LearningTask) -> Vec<String> {
    formatters
        .iter()
        .map(|formatter| formatter.format(task))
        .collect()
}

#[derive(Debug, Default)]
pub struct LearningTaskBuilder {
    title: Option<String>,
    running: bool,
}

impl LearningTaskBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn running(mut self, running: bool) -> Self {
        self.running = running;
        self
    }

    pub fn build(self) -> Result<LearningTask, LearningError> {
        let title = self.title.ok_or(LearningError::EmptyTitle)?;
        let mut task = LearningTask::new(title)?;
        if self.running {
            task.state = TaskState::Running;
        }
        Ok(task)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supports_static_and_dynamic_dispatch() -> Result<(), LearningError> {
        let task = LearningTaskBuilder::new().title("read traits").build()?;
        assert_eq!(format_static(&PlainFormatter, &task), "task: read traits");
        let formatters: Vec<Box<dyn TaskFormatter>> =
            vec![Box::new(PlainFormatter), Box::new(StatusFormatter)];
        assert_eq!(format_dynamic(&formatters, &task).len(), 2);
        Ok(())
    }
}
