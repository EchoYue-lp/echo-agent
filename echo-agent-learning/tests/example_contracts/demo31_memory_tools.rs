//! demo31_memory_tools: reviewed memory tools in a ReAct Agent.
//!
//! `with_memory_tools(store)` exposes approved recall. Installing a layer
//! manager adds journaled remember/forget and explicit Draft activation.
//!
//! ```bash
//! cargo test -p echo-agent-learning --features eval,improve --test example_contracts contract_demo31_memory_tools --locked
//! ```

use echo_agent::evolution::MemoryLayerManager;
use echo_agent::evolution::audit::NullChangeLog;
use echo_agent::prelude::*;
use std::sync::Arc;

#[tokio::test]
async fn contract_demo31_memory_tools() -> echo_agent::error::Result<()> {
    let store = Arc::new(InMemoryStore::new());
    let mut agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("memory_agent")
        .with_memory_tools(store.clone())
        .build()?;

    let tools = agent.tool_names();
    assert!(tools.contains(&"recall".to_string()));
    assert!(tools.contains(&"search_memory".to_string()));
    assert!(!tools.contains(&"remember".to_string()));
    assert!(!tools.contains(&"forget".to_string()));

    let ns = &["agent", "memories"];
    store
        .put(
            ns,
            "legacy",
            serde_json::json!({ "content": "Rust legacy note" }),
        )
        .await?;
    let before = agent
        .tool_manager()
        .execute_tool(
            "recall",
            [("query".into(), serde_json::json!("Rust"))].into(),
        )
        .await?;
    assert!(!before.output.contains("Rust legacy note"));

    let dir = tempfile::tempdir()?;
    let manager = Arc::new(MemoryLayerManager::new(
        dir.path().to_path_buf(),
        store,
        Box::new(NullChangeLog),
    ));
    agent.install_memory_layer_manager(manager.clone())?;
    assert!(agent.tool_names().contains(&"remember".to_string()));
    assert!(agent.tool_names().contains(&"forget".to_string()));

    let content = "The project uses Rust";
    manager
        .write_memory(
            "rust-fact",
            content,
            MemoryMeta::new(
                MemoryType::ProjectFact,
                MemorySource::ExplicitSave,
                "project",
            )
            .with_provenance(MemoryProvenance::draft(
                MemoryTrust::User,
                vec![MemoryEvidence::new(MemoryEvidenceRole::User, content)],
            )),
        )
        .await?;
    let draft = agent
        .tool_manager()
        .execute_tool(
            "recall",
            [("query".into(), serde_json::json!("Rust"))].into(),
        )
        .await?;
    assert!(!draft.output.contains(content));

    let proposal = manager
        .preview_activation("rust-fact")
        .await?
        .ok_or_else(|| echo_agent::error::ReactError::Other("Draft proposal missing".into()))?;
    manager
        .activate_draft(
            &proposal,
            MemoryApproval::new("demo31-review", "example-reviewer", 1),
        )
        .await?;
    let approved = agent
        .tool_manager()
        .execute_tool(
            "recall",
            [("query".into(), serde_json::json!("Rust"))].into(),
        )
        .await?;
    assert!(approved.output.contains(content));

    let mut bare_agent = ReactAgent::new(AgentConfig::minimal("qwen3-max", "bare_agent"));
    bare_agent.set_memory_store(Arc::new(InMemoryStore::new()))?;
    assert!(
        bare_agent
            .tool_names()
            .contains(&"search_memory".to_string())
    );
    assert!(!bare_agent.tool_names().contains(&"remember".to_string()));
    Ok(())
}
