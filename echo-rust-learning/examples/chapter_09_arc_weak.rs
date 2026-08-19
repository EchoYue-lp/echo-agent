use echo_rust_learning::errors::LearningError;
use echo_rust_learning::smart_pointers::arc_weak::{AgentHandle, AgentRegistry};
use std::sync::Arc;

fn main() -> Result<(), LearningError> {
    let registry = AgentRegistry::default();
    let agent = Arc::new(AgentHandle {
        name: "reviewer".to_string(),
    });
    registry.register(&agent)?;

    println!("Arc 强引用数: {}", Arc::strong_count(&agent));
    println!("注册表可升级 Weak: {}", registry.get("reviewer")?.is_some());
    drop(agent);
    println!("所有者释放后: {}", registry.get("reviewer")?.is_some());
    println!("清理弱引用: {}", registry.remove_expired()?);
    Ok(())
}
