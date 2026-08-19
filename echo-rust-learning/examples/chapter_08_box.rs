use echo_rust_learning::basics::LearningTask;
use echo_rust_learning::errors::LearningError;
use echo_rust_learning::smart_pointers::box_pointer::{PlanNode, boxed_formatter};

fn main() -> Result<(), LearningError> {
    let plan = PlanNode::Sequence(
        Box::new(PlanNode::Step("调研".to_string())),
        Box::new(PlanNode::Step("实现".to_string())),
    );
    let task = LearningTask::new("理解 Box<dyn Trait>")?;
    let formatter = boxed_formatter();

    println!("计划节点数: {}", plan.node_count());
    println!("动态分发: {}", formatter.format(&task));
    Ok(())
}
