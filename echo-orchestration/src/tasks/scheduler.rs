//! Parallel execution strategies for task scheduling
//!
//! This module provides different parallel execution strategies
//! based on task characteristics (read-only vs write operations).

use crate::tasks::ChangeType;
use serde::{Deserialize, Serialize};

/// Parallel execution strategy
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParallelStrategy {
    /// Read-only tasks can run in parallel
    ReadOnlyParallel,
    /// Write tasks must run serially
    WriteSerial,
    /// Write tasks must use worktree isolation
    WriteWorktreeIsolated,
    /// Background tasks run separately
    BackgroundSeparate,
}

impl Default for ParallelStrategy {
    fn default() -> Self {
        Self::ReadOnlyParallel
    }
}

/// File change record for conflict detection
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileChangeRecord {
    /// File path
    pub path: String,
    /// Change type
    pub change_type: ChangeType,
    /// Task ID that made the change
    pub task_id: String,
}

/// Conflict between tasks
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskConflict {
    /// First task ID
    pub task1_id: String,
    /// Second task ID
    pub task2_id: String,
    /// Overlapping files
    pub overlapping_files: Vec<String>,
}

/// Conflict detector for write operations
pub struct ConflictDetector {
    /// Write tasks to check for conflicts
    write_tasks: Vec<crate::tasks::Task>,
}

impl ConflictDetector {
    /// Create a new conflict detector
    pub fn new(write_tasks: Vec<crate::tasks::Task>) -> Self {
        Self { write_tasks }
    }

    /// Detect conflicts between write tasks
    pub fn detect_conflicts(&self) -> Vec<TaskConflict> {
        let mut conflicts = Vec::new();

        for i in 0..self.write_tasks.len() {
            for j in (i + 1)..self.write_tasks.len() {
                let task1 = &self.write_tasks[i];
                let task2 = &self.write_tasks[j];

                // Check if they're independent (no dependency between them)
                let has_dependency = task1.dependencies.contains(&task2.id)
                    || task2.dependencies.contains(&task1.id);

                if !has_dependency {
                    // Check for file overlap
                    let overlapping = self.find_overlapping_files(task1, task2);
                    if !overlapping.is_empty() {
                        conflicts.push(TaskConflict {
                            task1_id: task1.id.clone(),
                            task2_id: task2.id.clone(),
                            overlapping_files: overlapping,
                        });
                    }
                }
            }
        }

        conflicts
    }

    /// Find overlapping files between two tasks
    fn find_overlapping_files(
        &self,
        task1: &crate::tasks::Task,
        task2: &crate::tasks::Task,
    ) -> Vec<String> {
        let mut overlapping = Vec::new();

        // Check changed_files overlap
        let task1_files: std::collections::HashSet<_> = task1
            .changed_files
            .iter()
            .map(|fc| fc.path.clone())
            .collect();

        let task2_files: std::collections::HashSet<_> = task2
            .changed_files
            .iter()
            .map(|fc| fc.path.clone())
            .collect();

        for file in &task1_files {
            if task2_files.contains(file) {
                overlapping.push(file.clone());
            }
        }

        overlapping
    }
}

/// Task scheduler based on parallel strategy
pub struct TaskScheduler {
    /// Parallel strategy to use
    strategy: ParallelStrategy,
    /// Conflict detector
    conflict_detector: ConflictDetector,
}

impl TaskScheduler {
    /// Create a new task scheduler
    pub fn new(strategy: ParallelStrategy, write_tasks: Vec<crate::tasks::Task>) -> Self {
        Self {
            strategy,
            conflict_detector: ConflictDetector::new(write_tasks),
        }
    }

    /// Schedule tasks based on strategy
    pub fn schedule(&self, tasks: &[crate::tasks::Task]) -> SchedulePlan {
        match self.strategy {
            ParallelStrategy::ReadOnlyParallel => self.schedule_read_only_parallel(tasks),
            ParallelStrategy::WriteSerial => self.schedule_write_serial(tasks),
            ParallelStrategy::WriteWorktreeIsolated => self.schedule_write_worktree_isolated(tasks),
            ParallelStrategy::BackgroundSeparate => self.schedule_background_separate(tasks),
        }
    }

    /// Schedule read-only tasks in parallel
    fn schedule_read_only_parallel(&self, tasks: &[crate::tasks::Task]) -> SchedulePlan {
        // All read-only tasks can run in parallel
        let parallel_groups = vec![tasks.iter().map(|t| t.id.clone()).collect()];

        SchedulePlan {
            strategy: ParallelStrategy::ReadOnlyParallel,
            parallel_groups,
            serial_sequence: Vec::new(),
            conflicts: Vec::new(),
        }
    }

    /// Schedule write tasks serially
    fn schedule_write_serial(&self, tasks: &[crate::tasks::Task]) -> SchedulePlan {
        let conflicts = self.conflict_detector.detect_conflicts();

        if conflicts.is_empty() {
            // No conflicts, can run in parallel
            let parallel_groups = vec![tasks.iter().map(|t| t.id.clone()).collect()];
            SchedulePlan {
                strategy: ParallelStrategy::WriteSerial,
                parallel_groups,
                serial_sequence: Vec::new(),
                conflicts: Vec::new(),
            }
        } else {
            // Conflicts detected, must run serially
            let serial_sequence: Vec<String> = tasks.iter().map(|t| t.id.clone()).collect();
            SchedulePlan {
                strategy: ParallelStrategy::WriteSerial,
                parallel_groups: Vec::new(),
                serial_sequence,
                conflicts,
            }
        }
    }

    /// Schedule write tasks with worktree isolation
    fn schedule_write_worktree_isolated(&self, tasks: &[crate::tasks::Task]) -> SchedulePlan {
        let conflicts = self.conflict_detector.detect_conflicts();

        // All tasks can run in parallel with worktree isolation
        let parallel_groups = vec![tasks.iter().map(|t| t.id.clone()).collect()];

        SchedulePlan {
            strategy: ParallelStrategy::WriteWorktreeIsolated,
            parallel_groups,
            serial_sequence: Vec::new(),
            conflicts,
        }
    }

    /// Schedule background tasks separately
    fn schedule_background_separate(&self, tasks: &[crate::tasks::Task]) -> SchedulePlan {
        // Separate background tasks from regular tasks
        let (background, regular): (Vec<_>, Vec<_>) = tasks
            .iter()
            .partition(|t| t.task_type == crate::tasks::TaskType::Background);

        let mut parallel_groups = Vec::new();

        if !regular.is_empty() {
            parallel_groups.push(regular.iter().map(|t| t.id.clone()).collect());
        }

        if !background.is_empty() {
            parallel_groups.push(background.iter().map(|t| t.id.clone()).collect());
        }

        SchedulePlan {
            strategy: ParallelStrategy::BackgroundSeparate,
            parallel_groups,
            serial_sequence: Vec::new(),
            conflicts: Vec::new(),
        }
    }
}

/// Schedule plan result
#[derive(Debug, Clone)]
pub struct SchedulePlan {
    /// Strategy used
    pub strategy: ParallelStrategy,
    /// Groups of tasks that can run in parallel
    pub parallel_groups: Vec<Vec<String>>,
    /// Sequence of tasks that must run serially
    pub serial_sequence: Vec<String>,
    /// Detected conflicts
    pub conflicts: Vec<TaskConflict>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::{ChangeType, FileChange, Task, TaskType};

    #[test]
    fn test_parallel_strategy_default() {
        let strategy = ParallelStrategy::default();
        assert_eq!(strategy, ParallelStrategy::ReadOnlyParallel);
    }

    #[test]
    fn test_conflict_detector_no_conflicts() {
        let task1 = Task::new("task1", "Task 1");
        let task2 = Task::new("task2", "Task 2");

        let detector = ConflictDetector::new(vec![task1, task2]);
        let conflicts = detector.detect_conflicts();

        assert!(conflicts.is_empty());
    }

    #[test]
    fn test_conflict_detector_with_conflicts() {
        let mut task1 = Task::new("task1", "Task 1");
        task1.changed_files = vec![FileChange {
            path: "src/main.rs".to_string(),
            change_type: ChangeType::Modified,
            diff: None,
            checksum: None,
        }];

        let mut task2 = Task::new("task2", "Task 2");
        task2.changed_files = vec![FileChange {
            path: "src/main.rs".to_string(),
            change_type: ChangeType::Modified,
            diff: None,
            checksum: None,
        }];

        let detector = ConflictDetector::new(vec![task1, task2]);
        let conflicts = detector.detect_conflicts();

        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].task1_id, "task1");
        assert_eq!(conflicts[0].task2_id, "task2");
        assert_eq!(conflicts[0].overlapping_files, vec!["src/main.rs"]);
    }

    #[test]
    fn test_conflict_detector_with_dependency() {
        let mut task1 = Task::new("task1", "Task 1");
        task1.changed_files = vec![FileChange {
            path: "src/main.rs".to_string(),
            change_type: ChangeType::Modified,
            diff: None,
            checksum: None,
        }];

        let mut task2 = Task::new("task2", "Task 2");
        task2.changed_files = vec![FileChange {
            path: "src/main.rs".to_string(),
            change_type: ChangeType::Modified,
            diff: None,
            checksum: None,
        }];
        task2.dependencies = vec!["task1".to_string()];

        let detector = ConflictDetector::new(vec![task1, task2]);
        let conflicts = detector.detect_conflicts();

        // Should not detect conflict because task2 depends on task1
        assert!(conflicts.is_empty());
    }

    #[test]
    fn test_scheduler_read_only_parallel() {
        let task1 = Task::new("task1", "Task 1");
        let task2 = Task::new("task2", "Task 2");

        let scheduler = TaskScheduler::new(ParallelStrategy::ReadOnlyParallel, vec![]);
        let plan = scheduler.schedule(&[task1, task2]);

        assert_eq!(plan.strategy, ParallelStrategy::ReadOnlyParallel);
        assert_eq!(plan.parallel_groups.len(), 1);
        assert_eq!(plan.parallel_groups[0].len(), 2);
    }

    #[test]
    fn test_scheduler_write_serial_with_conflicts() {
        let mut task1 = Task::new("task1", "Task 1");
        task1.changed_files = vec![FileChange {
            path: "src/main.rs".to_string(),
            change_type: ChangeType::Modified,
            diff: None,
            checksum: None,
        }];

        let mut task2 = Task::new("task2", "Task 2");
        task2.changed_files = vec![FileChange {
            path: "src/main.rs".to_string(),
            change_type: ChangeType::Modified,
            diff: None,
            checksum: None,
        }];

        let scheduler = TaskScheduler::new(
            ParallelStrategy::WriteSerial,
            vec![task1.clone(), task2.clone()],
        );
        let plan = scheduler.schedule(&[task1, task2]);

        assert_eq!(plan.strategy, ParallelStrategy::WriteSerial);
        assert!(plan.parallel_groups.is_empty());
        assert_eq!(plan.serial_sequence.len(), 2);
        assert_eq!(plan.conflicts.len(), 1);
    }

    #[test]
    fn test_scheduler_write_worktree_isolated() {
        let mut task1 = Task::new("task1", "Task 1");
        task1.changed_files = vec![FileChange {
            path: "src/main.rs".to_string(),
            change_type: ChangeType::Modified,
            diff: None,
            checksum: None,
        }];

        let mut task2 = Task::new("task2", "Task 2");
        task2.changed_files = vec![FileChange {
            path: "src/main.rs".to_string(),
            change_type: ChangeType::Modified,
            diff: None,
            checksum: None,
        }];

        let scheduler = TaskScheduler::new(
            ParallelStrategy::WriteWorktreeIsolated,
            vec![task1.clone(), task2.clone()],
        );
        let plan = scheduler.schedule(&[task1, task2]);

        assert_eq!(plan.strategy, ParallelStrategy::WriteWorktreeIsolated);
        assert_eq!(plan.parallel_groups.len(), 1);
        assert_eq!(plan.parallel_groups[0].len(), 2);
        assert_eq!(plan.conflicts.len(), 1);
    }

    #[test]
    fn test_scheduler_background_separate() {
        let mut task1 = Task::new("task1", "Task 1");
        task1.task_type = TaskType::Implementation;

        let mut task2 = Task::new("task2", "Task 2");
        task2.task_type = TaskType::Background;

        let scheduler = TaskScheduler::new(
            ParallelStrategy::BackgroundSeparate,
            vec![task1.clone(), task2.clone()],
        );
        let plan = scheduler.schedule(&[task1, task2]);

        assert_eq!(plan.strategy, ParallelStrategy::BackgroundSeparate);
        assert_eq!(plan.parallel_groups.len(), 2);
    }
}
