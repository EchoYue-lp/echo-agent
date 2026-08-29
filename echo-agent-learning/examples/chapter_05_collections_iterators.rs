use echo_agent_learning::basics::{LearningTask, TaskState};
use echo_agent_learning::collections::{TaskCatalog, normalize_owned, word_frequencies};
use echo_agent_learning::errors::LearningError;

fn main() -> Result<(), LearningError> {
    let mut catalog = TaskCatalog::new();
    catalog.insert(LearningTask::new("阅读 Rust")?)?;
    catalog.insert(LearningTask::new("测试 Agent")?)?;
    catalog.add_tag("rust");
    catalog.add_tag("agent");

    let pending = catalog.matching_titles(|task| task.state == TaskState::Pending);
    let normalized = normalize_owned(vec![" Rust ".to_string(), "AGENT".to_string()]);

    println!("待处理任务: {pending:?}");
    println!("有序标签: {:?}", catalog.tags());
    println!("词频: {:?}", word_frequencies("Rust rust Agent"));
    println!("规范化: {normalized:?}");
    Ok(())
}
