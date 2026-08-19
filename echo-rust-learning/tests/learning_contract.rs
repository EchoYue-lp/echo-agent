use echo_agent::prelude::Result as EchoResult;
use echo_rust_learning::async_concurrency::run_subagents;
use echo_rust_learning::basics::unicode_preview;
use echo_rust_learning::errors::LearningError;
use echo_rust_learning::project_patterns::run_greet_tool;
use echo_rust_learning::smart_pointers::arc_weak::{AgentHandle, AgentRegistry};
use std::sync::Arc;

#[test]
fn unicode_examples_are_scalar_safe() {
    assert_eq!(unicode_preview("中文🦀Rust", 3), "中文🦀...");
}

#[test]
fn weak_registry_releases_expired_entries() -> Result<(), LearningError> {
    let registry = AgentRegistry::default();
    let agent = Arc::new(AgentHandle {
        name: "researcher".to_string(),
    });
    registry.register(&agent)?;
    drop(agent);
    assert!(registry.get("researcher")?.is_none());
    assert_eq!(registry.remove_expired()?, 1);
    Ok(())
}

#[tokio::test]
async fn async_and_framework_examples_are_offline() -> EchoResult<()> {
    let results = run_subagents(vec!["reviewer".to_string()])
        .await
        .map_err(|error| echo_agent::error::ReactError::Other(error.to_string()))?;
    assert_eq!(results, vec!["reviewer: completed"]);

    let tool_result = run_greet_tool("测试者").await?;
    assert_eq!(tool_result.output, "你好，测试者");
    Ok(())
}
