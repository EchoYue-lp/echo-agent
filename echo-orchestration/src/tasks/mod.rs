//! DAG task planning system
//!
//! Used by planning mode, supports parallel execution of mutually independent subtasks.

pub mod background_state;
mod dag;
mod events;
pub mod executor;
pub mod hooks;
mod manager;
pub mod replanner;
pub mod runtime;
pub mod runtime_executor;
pub mod scheduler;
pub mod store;
mod task;
mod time;
pub mod verifier;

pub mod background_task;
pub mod composite;
pub mod progress;

pub use background_state::{BackgroundTaskState, CheckpointStore as BackgroundCheckpointStore};
pub use background_task::{
    AnyBackgroundTask, BackgroundTask, BackgroundTaskStatus, TaskSpawner, TaskSpawnerConfig,
    TaskSummary,
};
pub use composite::{CompositePlan, CompositeStep, CompositeStrategy, execute_composite};
pub use events::{
    AsyncTaskEventListener, LoggingListener, TaskEvent, TaskEventBus, TaskEventListener,
};
pub use executor::{
    TaskContext, TaskExecuteFn, TaskExecutionResult, TaskExecutor, TaskExecutorConfig,
};
pub use hooks::{
    LoggingHooks, NoopHooks, RetryDecision, TaskHookContext, TaskHookRegistry, TaskHooks,
};
pub use manager::TaskManager;
pub use progress::{Phase, PhasePlan, ProgressReporter, TaskProgress};
pub use replanner::{LlmReplanner, ReplanDecision, ReplanTrigger, Replanner, RuleBasedReplanner};
pub use runtime::{
    ConcurrencyLimits, DagExecutionState, DagRefresh, NestedDelegationPolicy, RuntimeTask,
    RuntimeTaskKind, RuntimeTaskStatus, SuggestedTask, TaskExecutionSummary, TaskId, TaskSubagent,
    TaskSubagentContext,
};
pub use runtime_executor::{
    RuntimeDagController, RuntimeDagExecutor, RuntimeDagExecutorConfig, RuntimeDagOutcome,
    RuntimePlanSnapshot, RuntimeStopDisposition, RuntimeTaskResolution,
};
pub use scheduler::{
    ConflictDetector, FileChangeRecord, ParallelStrategy, SchedulePlan, TaskConflict, TaskScheduler,
};
pub use store::{SqliteTaskStore, TaskStore};
pub use task::{
    Artifact, ArtifactType, AttemptStatus, ChangeType, CheckpointPolicy, CommandRecord,
    ContextScope, Evidence, EvidenceType, FallbackStrategy, FileChange, InputType, OutputType,
    RiskLevel, Task, TaskAttempt, TaskInput, TaskOutput, TaskState, TaskStatus, TaskType,
    VerificationResult, VerificationSpec, VerificationType,
};
pub use verifier::{
    CommandVerifier, DiffCheckVerifier, FileExistsVerifier, HumanReviewVerifier, LlmReviewVerifier,
    NoopVerifier, TestVerifier, Verifier, VerifierFactory,
};

#[cfg(test)]
mod tests {
    use crate::tasks::TaskManager;
    use crate::tasks::{Task, TaskStatus};

    fn create_task(id: &str, description: &str, dependencies: Vec<&str>) -> Task {
        let mut task = Task::new(id, description)
            .with_dependencies(dependencies.into_iter().map(String::from).collect());
        task.created_at = 0;
        task.updated_at = 0;
        task
    }

    #[test]
    fn test_no_circular_dependencies() {
        let manager = TaskManager::new();

        // Create a task chain without cyclic dependencies: task1 -> task2 -> task3
        manager.add_task(create_task("task1", "First task", vec![]));
        manager.add_task(create_task("task2", "Second task", vec!["task1"]));
        manager.add_task(create_task("task3", "Third task", vec!["task2"]));

        let cycles = manager.detect_circular_dependencies();
        assert!(cycles.is_empty(), "should have no cyclic dependencies");
        assert!(!manager.has_circular_dependencies());
    }

    #[test]
    fn test_simple_circular_dependency() {
        let manager = TaskManager::new();

        // Create a simple cycle: task1 -> task2 -> task1
        manager.add_task(create_task("task1", "Task 1", vec!["task2"]));
        manager.add_task(create_task("task2", "Task 2", vec!["task1"]));

        let cycles = manager.detect_circular_dependencies();
        assert_eq!(cycles.len(), 1, "should detect one cycle");
        assert!(manager.has_circular_dependencies());

        // Verify the cycle path
        let cycle = &cycles[0];
        assert!(cycle.contains(&"task1".to_string()));
        assert!(cycle.contains(&"task2".to_string()));
    }

    #[test]
    fn test_complex_circular_dependency() {
        let manager = TaskManager::new();

        // Create a complex cycle: task1 -> task2 -> task3 -> task1
        manager.add_task(create_task("task1", "Task 1", vec!["task3"]));
        manager.add_task(create_task("task2", "Task 2", vec!["task1"]));
        manager.add_task(create_task("task3", "Task 3", vec!["task2"]));

        let cycles = manager.detect_circular_dependencies();
        assert_eq!(cycles.len(), 1, "should detect one cycle");
        assert_eq!(cycles[0].len(), 3, "cycle should contain 3 tasks");
    }

    #[test]
    fn test_multiple_circular_dependencies() {
        let manager = TaskManager::new();

        // Create two independent cycles
        // Cycle 1: task1 -> task2 -> task1
        manager.add_task(create_task("task1", "Task 1", vec!["task2"]));
        manager.add_task(create_task("task2", "Task 2", vec!["task1"]));

        // Cycle 2: task3 -> task4 -> task3
        manager.add_task(create_task("task3", "Task 3", vec!["task4"]));
        manager.add_task(create_task("task4", "Task 4", vec!["task3"]));

        let cycles = manager.detect_circular_dependencies();
        assert_eq!(cycles.len(), 2, "should detect two cycles");
    }

    #[test]
    fn test_self_dependency() {
        let manager = TaskManager::new();

        // Create a self-dependency: task1 -> task1
        manager.add_task(create_task("task1", "Task 1", vec!["task1"]));

        let cycles = manager.detect_circular_dependencies();
        assert_eq!(cycles.len(), 1, "should detect a self-dependency cycle");
        assert_eq!(
            cycles[0].len(),
            1,
            "self-dependency cycle should contain only one task"
        );
    }

    #[test]
    fn test_mixed_dependencies() {
        let manager = TaskManager::new();

        // Mixed case: some parts have cycles, some don't
        // Normal chain: task1 -> task2 -> task3
        manager.add_task(create_task("task1", "Task 1", vec![]));
        manager.add_task(create_task("task2", "Task 2", vec!["task1"]));
        manager.add_task(create_task("task3", "Task 3", vec!["task2"]));

        // Cycle: task4 -> task5 -> task4
        manager.add_task(create_task("task4", "Task 4", vec!["task5"]));
        manager.add_task(create_task("task5", "Task 5", vec!["task4"]));

        let cycles = manager.detect_circular_dependencies();
        assert_eq!(cycles.len(), 1, "should detect only one cycle");
        assert!(cycles[0].contains(&"task4".to_string()));
        assert!(cycles[0].contains(&"task5".to_string()));
    }

    #[test]
    fn test_topological_order_no_cycles() {
        let manager = TaskManager::new();

        // task3 -> task2 -> task1
        manager.add_task(create_task("task1", "Task 1", vec![]));
        manager.add_task(create_task("task2", "Task 2", vec!["task1"]));
        manager.add_task(create_task("task3", "Task 3", vec!["task2"]));

        let order = manager.get_topological_order();
        assert!(
            order.is_ok(),
            "should succeed when no cyclic dependencies exist"
        );

        let order = order.unwrap();
        assert_eq!(order.len(), 3, "should contain all tasks");

        // Verify order: task1 should be before task2, task2 should be before task3
        let pos1 = order.iter().position(|id| id == "task1").unwrap();
        let pos2 = order.iter().position(|id| id == "task2").unwrap();
        let pos3 = order.iter().position(|id| id == "task3").unwrap();

        assert!(pos1 < pos2, "task1 should be before task2");
        assert!(pos2 < pos3, "task2 should be before task3");
    }

    #[test]
    fn test_topological_order_with_cycles() {
        let manager = TaskManager::new();

        // Create a cycle
        manager.add_task(create_task("task1", "Task 1", vec!["task2"]));
        manager.add_task(create_task("task2", "Task 2", vec!["task1"]));

        let order = manager.get_topological_order();
        assert!(
            order.is_err(),
            "should return an error when cyclic dependencies exist"
        );

        let error_msg = order.unwrap_err();
        assert!(
            error_msg.contains("Cyclic dependency"),
            "error message should contain cyclic dependency hint"
        );
    }

    #[test]
    fn test_get_dependency_chain() {
        let manager = TaskManager::new();

        // task3 -> task2 -> task1
        manager.add_task(create_task("task1", "Task 1", vec![]));
        manager.add_task(create_task("task2", "Task 2", vec!["task1"]));
        manager.add_task(create_task("task3", "Task 3", vec!["task2"]));

        let chains = manager.get_dependency_chain("task3");
        assert_eq!(chains.len(), 1, "should have one dependency chain");
        assert_eq!(
            chains[0],
            vec!["task3", "task2", "task1"],
            "dependency chain order should be correct"
        );
    }

    #[test]
    fn test_get_dependency_chain_multiple() {
        let manager = TaskManager::new();

        // task4 depends on task2 and task3
        // task2 -> task1
        // task3 -> task1
        manager.add_task(create_task("task1", "Task 1", vec![]));
        manager.add_task(create_task("task2", "Task 2", vec!["task1"]));
        manager.add_task(create_task("task3", "Task 3", vec!["task1"]));
        manager.add_task(create_task("task4", "Task 4", vec!["task2", "task3"]));

        let chains = manager.get_dependency_chain("task4");
        assert_eq!(chains.len(), 2, "should have two dependency chains");

        // Verify the two chains
        let chain1 = ["task4", "task2", "task1"];
        let chain2 = ["task4", "task3", "task1"];
        let chain1 = chain1.iter().map(|x| x.to_string()).collect();
        let chain2 = chain2.iter().map(|x| x.to_string()).collect();

        assert!(chains.contains(&chain1), "should contain the first chain");
        assert!(chains.contains(&chain2), "should contain the second chain");
    }

    #[test]
    fn test_visualize_dependencies() {
        let manager = TaskManager::new();

        manager.add_task(create_task("task1", "Task 1", vec![]));
        manager.add_task(create_task("task2", "Task 2", vec!["task1"]));
        manager.add_task(create_task("task3", "Task 3", vec!["task2"]));

        let mermaid = manager.visualize_dependencies();
        assert!(
            mermaid.contains("graph TD"),
            "should contain Mermaid diagram type"
        );
        assert!(mermaid.contains("task1"), "should contain task1");
        assert!(mermaid.contains("task2"), "should contain task2");
        assert!(mermaid.contains("task3"), "should contain task3");
        assert!(mermaid.contains("-->"), "should contain arrows");
    }

    #[test]
    fn test_ready_tasks_with_dependencies() {
        let manager = TaskManager::new();

        manager.add_task(create_task("task1", "Task 1", vec![]));
        manager.add_task(create_task("task2", "Task 2", vec!["task1"]));
        manager.add_task(create_task("task3", "Task 3", vec!["task2"]));

        // Initially only task1 is executable
        let ready = manager.get_ready_tasks();
        assert_eq!(ready.len(), 1, "should have only one executable task");
        assert_eq!(ready[0].id, "task1", "should be task1");

        // Mark task1 as complete
        manager
            .update_task("task1", TaskStatus::InProgress)
            .unwrap();
        manager.update_task("task1", TaskStatus::Completed).unwrap();
        let ready = manager.get_ready_tasks();
        assert_eq!(ready.len(), 1, "should have only one executable task");
        assert_eq!(ready[0].id, "task2", "should be task2");
    }

    #[test]
    fn test_get_next_task_priority() {
        let manager = TaskManager::new();

        let mut task1 = Task::new("task1", "Low priority").with_priority(3);
        task1.created_at = 0;
        task1.updated_at = 0;
        manager.add_task(task1);

        let mut task2 = Task::new("task2", "High priority").with_priority(8);
        task2.created_at = 0;
        task2.updated_at = 0;
        manager.add_task(task2);

        let next = manager.get_next_task();
        assert!(next.is_some(), "should have a next task");
        assert_eq!(
            next.unwrap().id,
            "task2",
            "should return the higher priority task"
        );
    }
}
