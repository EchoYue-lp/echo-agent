//! 在原 unit API 上直接观察错误发布；同一夹具也可加在修复后来源复验。
//! 此处 let _ 同时兼容原 unit 与后继 Result；正式回归另断言真实错误。

use super::ReactAgent;
use crate::agent::{Agent, AgentConfig};
use crate::error::{ReactError, Result};
use crate::evolution::MemoryLayerManager;
use crate::evolution::audit::NullChangeLog;
use crate::evolution::layer::WARM_NAMESPACE;
use crate::llm::types::Message;
use crate::memory::{
    InMemoryStore, MemoryApproval, MemoryEvidence, MemoryEvidenceRole, MemoryMeta,
    MemoryProvenance, MemorySource, MemoryStatus, MemoryTrust, MemoryType, Store,
};
use echo_state::memory::typed_store::TypedMemoryStore;
use serde_json::{Value, json};
use std::sync::Arc;

struct ManagedFixture {
    _root: tempfile::TempDir,
    agent: ReactAgent,
    first: Arc<dyn Store>,
    other: Arc<dyn Store>,
    tools: Value,
}

async fn seed_marker(store: Arc<dyn Store>, marker: &str) -> Result<()> {
    let mut meta = MemoryMeta::new(MemoryType::ProjectFact, MemorySource::ExplicitSave, "store")
        .with_status(MemoryStatus::Active)
        .with_provenance(MemoryProvenance::draft(
            MemoryTrust::User,
            vec![MemoryEvidence::new(MemoryEvidenceRole::User, marker)],
        ));
    meta.provenance.approval = Some(MemoryApproval::new("before", "reviewer", 1));
    TypedMemoryStore::new(store)
        .put_typed(WARM_NAMESPACE, "beforemarker", marker, meta)
        .await
}

fn tools(agent: &ReactAgent) -> Result<Value> {
    let mut definitions = agent.tool_definitions();
    definitions.sort_by(|left, right| left.function.name.cmp(&right.function.name));
    serde_json::to_value(definitions).map_err(|error| ReactError::Other(error.to_string()))
}

async fn managed_fixture() -> Result<ManagedFixture> {
    let root = tempfile::tempdir()?;
    let first: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let other: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    seed_marker(first.clone(), "beforemarker alpha authority").await?;
    seed_marker(other.clone(), "beforemarker beta replacement").await?;
    let manager = Arc::new(MemoryLayerManager::new(
        root.path().to_path_buf(),
        first.clone(),
        Box::new(NullChangeLog),
    ));
    let mut agent = ReactAgent::new(AgentConfig::minimal("model", "agent"));
    agent.install_memory_layer_manager(manager)?;
    let tools = tools(&agent)?;
    Ok(ManagedFixture {
        _root: root,
        agent,
        first,
        other,
        tools,
    })
}

fn assert_first(agent: &ReactAgent, first: &Arc<dyn Store>) {
    assert!(agent.store().is_some_and(|store| Arc::ptr_eq(store, first)));
}

async fn assert_actual_managed_routes(fixture: &ManagedFixture) -> Result<()> {
    // 先收集实际路径再断言，before 的失败可展示公开 Store 已变 B 而其它路径仍 A。
    let automatic = fixture
        .agent
        .recall_long_term_memories("beforemarker")
        .await?;
    let recalled = fixture
        .agent
        .tool_manager()
        .execute_tool("recall", [("query".into(), json!("beforemarker"))].into())
        .await?;
    let writer = fixture
        .agent
        .tool_manager()
        .execute_tool(
            "remember",
            [("content".into(), json!("beforewriter persistent fact"))].into(),
        )
        .await?;
    let fact = "We decided to retain the original alpha backend for compression memory promotion.";
    let context = fixture.agent.memory.context.clone();
    let mut guard = context.lock().await;
    guard.set_messages(vec![
        Message::assistant(fact.into()),
        Message::user("latest request".into()),
    ]);
    let (stats, _) = guard
        .force_compress_with(&echo_state::compression::compressor::SlidingWindowCompressor::new(1))
        .await?;
    drop(guard);
    let key = crate::memory_promoter::durable_memory_content_key(fact);
    let observed = json!({
        "store_is_a": fixture.agent.store().is_some_and(|store| Arc::ptr_eq(store, &fixture.first)),
        "manager_is_a": fixture.agent.memory_layer_manager().is_some_and(|manager| Arc::ptr_eq(manager.store(), &fixture.first)),
        "tools_unchanged": tools(&fixture.agent)? == fixture.tools,
        "automatic_a": automatic.iter().any(|item| item.value.get("content").and_then(Value::as_str) == Some("beforemarker alpha authority")),
        "automatic_b": automatic.iter().any(|item| item.value.get("content").and_then(Value::as_str) == Some("beforemarker beta replacement")),
        "recall_a": recalled.success && recalled.output.contains("beforemarker alpha authority"),
        "recall_b": recalled.output.contains("beforemarker beta replacement"),
        "writer_success": writer.success,
        "writer_a": !fixture.first.search(WARM_NAMESPACE, "beforewriter", 10).await?.is_empty(),
        "writer_b": !fixture.other.search(WARM_NAMESPACE, "beforewriter", 10).await?.is_empty(),
        "evicted": stats.evicted > 0,
        "promoter_a": fixture.first.get(WARM_NAMESPACE, &key).await?.is_some(),
        "promoter_b": fixture.other.get(WARM_NAMESPACE, &key).await?.is_some(),
    });
    assert_eq!(
        observed,
        json!({
            "store_is_a": true, "manager_is_a": true, "tools_unchanged": true,
            "automatic_a": true, "automatic_b": false, "recall_a": true, "recall_b": false,
            "writer_success": true, "writer_a": true, "writer_b": false,
            "evicted": true, "promoter_a": true, "promoter_b": false,
        })
    );
    Ok(())
}

#[tokio::test]
async fn original_set_store_must_keep_manager_store_on_foreign_replacement() -> Result<()> {
    let mut fixture = managed_fixture().await?;
    let _ = fixture.agent.set_store(fixture.other.clone());
    assert_actual_managed_routes(&fixture).await
}

#[tokio::test]
async fn original_install_store_must_keep_manager_store_on_foreign_replacement() -> Result<()> {
    let mut fixture = managed_fixture().await?;
    let _ = fixture.agent.install_store(fixture.other.clone()).await;
    assert_actual_managed_routes(&fixture).await
}

#[tokio::test]
async fn original_safe_store_apis_keep_their_manager_rejection_control() -> Result<()> {
    let mut fixture = managed_fixture().await?;
    assert!(
        fixture
            .agent
            .set_memory_store(fixture.other.clone())
            .is_err()
    );
    assert!(
        fixture
            .agent
            .install_memory_store(fixture.other.clone())
            .await
            .is_err()
    );
    assert_actual_managed_routes(&fixture).await
}

#[tokio::test]
async fn original_set_store_must_not_publish_while_context_is_busy() -> Result<()> {
    let first: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let other: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let mut agent = ReactAgent::new(AgentConfig::minimal("model", "agent"));
    let _ = agent.set_store(first.clone());
    let context = agent.memory.context.clone();
    let guard = context.lock().await;
    let _ = agent.set_store(other);
    assert_first(&agent, &first);
    drop(guard);
    Ok(())
}

#[tokio::test]
async fn original_install_store_must_not_publish_before_lock_or_after_cancel() -> Result<()> {
    let first: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let other: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let mut agent = ReactAgent::new(AgentConfig::minimal("model", "agent"));
    let _ = agent.set_store(first.clone());
    let context = agent.memory.context.clone();
    let guard = context.lock().await;
    {
        let future = agent.install_store(other);
        futures::pin_mut!(future);
        assert!(futures::poll!(future.as_mut()).is_pending());
    }
    assert_first(&agent, &first);
    drop(guard);
    Ok(())
}

#[tokio::test]
async fn original_raw_store_apis_allow_real_unmanaged_a_to_b() -> Result<()> {
    for asynchronous in [false, true] {
        let first: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let other: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let mut agent = ReactAgent::new(AgentConfig::minimal("model", "agent"));
        let _ = agent.set_store(first.clone());
        if asynchronous {
            let _ = agent.install_store(other.clone()).await;
        } else {
            let _ = agent.set_store(other.clone());
        }
        assert!(
            agent
                .store()
                .is_some_and(|store| Arc::ptr_eq(store, &other))
        );
        agent
            .store()
            .ok_or_else(|| ReactError::Other("missing live Store".into()))?
            .put(&["raw"], "key", json!("B"))
            .await?;
        assert!(first.get(&["raw"], "key").await?.is_none());
        assert_eq!(
            other.get(&["raw"], "key").await?.map(|item| item.value),
            Some(json!("B"))
        );
        assert!(!agent.has_memory_layer_manager());
    }
    Ok(())
}
