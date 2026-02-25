mod dag;
mod manager;
mod task;

pub use manager::TaskManager;
pub use task::{Task, TaskStatus};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::manager::TaskManager;
    use crate::tasks::task::{Task, TaskStatus};

    fn create_task(id: &str, description: &str, dependencies: Vec<&str>) -> Task {
        Task {
            id: id.to_string(),
            description: description.to_string(),
            status: TaskStatus::Pending,
            dependencies: dependencies.into_iter().map(String::from).collect(),
            priority: 5,
            result: None,
            reasoning: None,
            parent_id: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn test_no_circular_dependencies() {
        let mut manager = TaskManager::new();

        // 创建无循环依赖的任务链: task1 -> task2 -> task3
        manager.add_task(create_task("task1", "First task", vec![]));
        manager.add_task(create_task("task2", "Second task", vec!["task1"]));
        manager.add_task(create_task("task3", "Third task", vec!["task2"]));

        let cycles = manager.detect_circular_dependencies();
        assert!(cycles.is_empty(), "应该没有循环依赖");
        assert!(!manager.has_circular_dependencies());
    }

    #[test]
    fn test_simple_circular_dependency() {
        let mut manager = TaskManager::new();

        // 创建简单循环: task1 -> task2 -> task1
        manager.add_task(create_task("task1", "Task 1", vec!["task2"]));
        manager.add_task(create_task("task2", "Task 2", vec!["task1"]));

        let cycles = manager.detect_circular_dependencies();
        assert_eq!(cycles.len(), 1, "应该检测到一个循环");
        assert!(manager.has_circular_dependencies());

        // 验证循环路径
        let cycle = &cycles[0];
        assert!(cycle.contains(&"task1".to_string()));
        assert!(cycle.contains(&"task2".to_string()));
    }

    #[test]
    fn test_complex_circular_dependency() {
        let mut manager = TaskManager::new();

        // 创建复杂循环: task1 -> task2 -> task3 -> task1
        manager.add_task(create_task("task1", "Task 1", vec!["task3"]));
        manager.add_task(create_task("task2", "Task 2", vec!["task1"]));
        manager.add_task(create_task("task3", "Task 3", vec!["task2"]));

        let cycles = manager.detect_circular_dependencies();
        assert_eq!(cycles.len(), 1, "应该检测到一个循环");
        assert_eq!(cycles[0].len(), 3, "循环应该包含3个任务");
    }

    #[test]
    fn test_multiple_circular_dependencies() {
        let mut manager = TaskManager::new();

        // 创建两个独立的循环
        // 循环1: task1 -> task2 -> task1
        manager.add_task(create_task("task1", "Task 1", vec!["task2"]));
        manager.add_task(create_task("task2", "Task 2", vec!["task1"]));

        // 循环2: task3 -> task4 -> task3
        manager.add_task(create_task("task3", "Task 3", vec!["task4"]));
        manager.add_task(create_task("task4", "Task 4", vec!["task3"]));

        let cycles = manager.detect_circular_dependencies();
        assert_eq!(cycles.len(), 2, "应该检测到两个循环");
    }

    #[test]
    fn test_self_dependency() {
        let mut manager = TaskManager::new();

        // 创建自依赖: task1 -> task1
        manager.add_task(create_task("task1", "Task 1", vec!["task1"]));

        let cycles = manager.detect_circular_dependencies();
        assert_eq!(cycles.len(), 1, "应该检测到自依赖循环");
        assert_eq!(cycles[0].len(), 1, "自依赖循环应该只包含一个任务");
    }

    #[test]
    fn test_mixed_dependencies() {
        let mut manager = TaskManager::new();

        // 混合情况：部分有循环，部分没有
        // 正常链: task1 -> task2 -> task3
        manager.add_task(create_task("task1", "Task 1", vec![]));
        manager.add_task(create_task("task2", "Task 2", vec!["task1"]));
        manager.add_task(create_task("task3", "Task 3", vec!["task2"]));

        // 循环: task4 -> task5 -> task4
        manager.add_task(create_task("task4", "Task 4", vec!["task5"]));
        manager.add_task(create_task("task5", "Task 5", vec!["task4"]));

        let cycles = manager.detect_circular_dependencies();
        assert_eq!(cycles.len(), 1, "应该只检测到一个循环");
        assert!(cycles[0].contains(&"task4".to_string()));
        assert!(cycles[0].contains(&"task5".to_string()));
    }

    #[test]
    fn test_topological_order_no_cycles() {
        let mut manager = TaskManager::new();

        // task3 -> task2 -> task1
        manager.add_task(create_task("task1", "Task 1", vec![]));
        manager.add_task(create_task("task2", "Task 2", vec!["task1"]));
        manager.add_task(create_task("task3", "Task 3", vec!["task2"]));

        let order = manager.get_topological_order();
        assert!(order.is_ok(), "无循环依赖时应该成功");

        let order = order.unwrap();
        assert_eq!(order.len(), 3, "应该包含所有任务");

        // 验证顺序：task1 应该在 task2 之前，task2 应该在 task3 之前
        let pos1 = order.iter().position(|id| id == "task1").unwrap();
        let pos2 = order.iter().position(|id| id == "task2").unwrap();
        let pos3 = order.iter().position(|id| id == "task3").unwrap();

        assert!(pos1 < pos2, "task1 应该在 task2 之前");
        assert!(pos2 < pos3, "task2 应该在 task3 之前");
    }

    #[test]
    fn test_topological_order_with_cycles() {
        let mut manager = TaskManager::new();

        // 创建循环
        manager.add_task(create_task("task1", "Task 1", vec!["task2"]));
        manager.add_task(create_task("task2", "Task 2", vec!["task1"]));

        let order = manager.get_topological_order();
        assert!(order.is_err(), "有循环依赖时应该返回错误");

        let error_msg = order.unwrap_err();
        assert!(
            error_msg.contains("循环依赖"),
            "错误信息应该包含循环依赖提示"
        );
    }

    #[test]
    fn test_get_dependency_chain() {
        let mut manager = TaskManager::new();

        // task3 -> task2 -> task1
        manager.add_task(create_task("task1", "Task 1", vec![]));
        manager.add_task(create_task("task2", "Task 2", vec!["task1"]));
        manager.add_task(create_task("task3", "Task 3", vec!["task2"]));

        let chains = manager.get_dependency_chain("task3");
        assert_eq!(chains.len(), 1, "应该有一条依赖链");
        assert_eq!(
            chains[0],
            vec!["task3", "task2", "task1"],
            "依赖链顺序应该正确"
        );
    }

    #[test]
    fn test_get_dependency_chain_multiple() {
        let mut manager = TaskManager::new();

        // task4 依赖 task2 和 task3
        // task2 -> task1
        // task3 -> task1
        manager.add_task(create_task("task1", "Task 1", vec![]));
        manager.add_task(create_task("task2", "Task 2", vec!["task1"]));
        manager.add_task(create_task("task3", "Task 3", vec!["task1"]));
        manager.add_task(create_task("task4", "Task 4", vec!["task2", "task3"]));

        let chains = manager.get_dependency_chain("task4");
        assert_eq!(chains.len(), 2, "应该有两条依赖链");

        // 验证两条链
        let chain1 = vec!["task4", "task2", "task1"];
        let chain2 = vec!["task4", "task3", "task1"];
        let chain1 = chain1.iter().map(|x| x.to_string()).collect();
        let chain2 = chain2.iter().map(|x| x.to_string()).collect();

        assert!(chains.contains(&chain1), "应该包含第一条链");
        assert!(chains.contains(&chain2), "应该包含第二条链");
    }

    #[test]
    fn test_visualize_dependencies() {
        let mut manager = TaskManager::new();

        manager.add_task(create_task("task1", "Task 1", vec![]));
        manager.add_task(create_task("task2", "Task 2", vec!["task1"]));
        manager.add_task(create_task("task3", "Task 3", vec!["task2"]));

        let mermaid = manager.visualize_dependencies();
        assert!(mermaid.contains("graph TD"), "应该包含 Mermaid 图表类型");
        assert!(mermaid.contains("task1"), "应该包含 task1");
        assert!(mermaid.contains("task2"), "应该包含 task2");
        assert!(mermaid.contains("task3"), "应该包含 task3");
        assert!(mermaid.contains("-->"), "应该包含箭头");
    }

    #[test]
    fn test_ready_tasks_with_dependencies() {
        let mut manager = TaskManager::new();

        manager.add_task(create_task("task1", "Task 1", vec![]));
        manager.add_task(create_task("task2", "Task 2", vec!["task1"]));
        manager.add_task(create_task("task3", "Task 3", vec!["task2"]));

        // 初始状态只有 task1 是可执行的
        let ready = manager.get_ready_tasks();
        assert_eq!(ready.len(), 1, "应该只有一个可执行任务");
        assert_eq!(ready[0].id, "task1", "应该是 task1");

        // 标记 task1 完成
        manager.update_task("task1", TaskStatus::Completed);
        let ready = manager.get_ready_tasks();
        assert_eq!(ready.len(), 1, "应该只有一个可执行任务");
        assert_eq!(ready[0].id, "task2", "应该是 task2");
    }

    #[test]
    fn test_get_next_task_priority() {
        let mut manager = TaskManager::new();

        manager.add_task(Task {
            id: "task1".to_string(),
            description: "Low priority".to_string(),
            status: TaskStatus::Pending,
            dependencies: vec![],
            priority: 3,
            result: None,
            reasoning: None,
            parent_id: None,
            created_at: 0,
            updated_at: 0,
        });

        manager.add_task(Task {
            id: "task2".to_string(),
            description: "High priority".to_string(),
            status: TaskStatus::Pending,
            dependencies: vec![],
            priority: 8,
            result: None,
            reasoning: None,
            parent_id: None,
            created_at: 0,
            updated_at: 0,
        });

        let next = manager.get_next_task();
        assert!(next.is_some(), "应该有下一个任务");
        assert_eq!(next.unwrap().id, "task2", "应该返回高优先级任务");
    }
}
