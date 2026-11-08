//! 四个真实公开入口的 Store 身份、失败原子性与工具分工回归。

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

const A_MARKER: &str = "backendmarker alpha retained memory";
const B_MARKER: &str = "backendmarker beta replacement memory";
const PROMOTED_FACT: &str =
    "We decided to retain the alpha memory backend throughout the current transaction.";

#[derive(Clone, Copy, Debug)]
enum StoreApi {
    Set,
    Install,
    SetMemory,
    InstallMemory,
}

const ALL_APIS: [StoreApi; 4] = [
    StoreApi::Set,
    StoreApi::Install,
    StoreApi::SetMemory,
    StoreApi::InstallMemory,
];

impl StoreApi {
    async fn apply(self, agent: &mut ReactAgent, store: Arc<dyn Store>) -> Result<()> {
        match self {
            Self::Set => agent.set_store(store),
            Self::Install => agent.install_store(store).await,
            Self::SetMemory => agent.set_memory_store(store),
            Self::InstallMemory => agent.install_memory_store(store).await,
        }
    }
}

fn assert_store(agent: &ReactAgent, expected: &Arc<dyn Store>) {
    assert!(
        agent
            .store()
            .is_some_and(|store| Arc::ptr_eq(store, expected))
    );
}

fn tool_snapshot(agent: &ReactAgent) -> Result<Value> {
    let mut definitions = agent.tool_definitions();
    definitions.sort_by(|left, right| left.function.name.cmp(&right.function.name));
    serde_json::to_value(definitions).map_err(|error| ReactError::Other(error.to_string()))
}

async fn seed_approved_marker(store: Arc<dyn Store>, marker: &str) -> Result<()> {
    let mut meta = MemoryMeta::new(
        MemoryType::ProjectFact,
        MemorySource::ExplicitSave,
        "store-authority",
    )
    .with_status(MemoryStatus::Active)
    .with_provenance(MemoryProvenance::draft(
        MemoryTrust::User,
        vec![MemoryEvidence::new(MemoryEvidenceRole::User, marker)],
    ));
    // 原始 Store KV 的宿主批准数据，用真实 eligibility/recaller 读取。
    meta.provenance.approval = Some(MemoryApproval::new("store-test", "reviewer", 1));
    TypedMemoryStore::new(store)
        .put_typed(WARM_NAMESPACE, "backendmarker", marker, meta)
        .await
}

async fn force_actual_promotion(agent: &ReactAgent) -> Result<()> {
    let mut context = agent.memory.context.lock().await;
    context.set_messages(vec![
        Message::user("earlier request".into()),
        Message::assistant(PROMOTED_FACT.into()),
        Message::user("current request".into()),
    ]);
    let (stats, _) = context
        .force_compress_with(&echo_state::compression::compressor::SlidingWindowCompressor::new(1))
        .await?;
    assert!(stats.evicted > 0);
    Ok(())
}

async fn assert_managed_routes_still_use_a(
    agent: &ReactAgent,
    manager: &Arc<MemoryLayerManager>,
    first: &Arc<dyn Store>,
    other: &Arc<dyn Store>,
) -> Result<()> {
    assert_store(agent, first);
    assert!(
        agent
            .memory_layer_manager()
            .is_some_and(|value| Arc::ptr_eq(value, manager))
    );
    let recalled = agent.recall_long_term_memories("backendmarker").await?;
    assert!(
        recalled
            .iter()
            .any(|item| item.value.get("content").and_then(Value::as_str) == Some(A_MARKER))
    );
    assert!(
        !recalled
            .iter()
            .any(|item| item.value.get("content").and_then(Value::as_str) == Some(B_MARKER))
    );
    let recall = agent
        .tool_manager()
        .execute_tool("recall", [("query".into(), json!("backendmarker"))].into())
        .await?;
    assert!(recall.success);
    assert!(recall.output.contains(A_MARKER));
    assert!(!recall.output.contains(B_MARKER));
    let remembered = agent
        .tool_manager()
        .execute_tool(
            "remember",
            [(
                "content".into(),
                json!("A durable writerbackend marker fact"),
            )]
            .into(),
        )
        .await?;
    assert!(remembered.success);
    assert!(
        !first
            .search(WARM_NAMESPACE, "writerbackend", 10)
            .await?
            .is_empty()
    );
    assert!(
        other
            .search(WARM_NAMESPACE, "writerbackend", 10)
            .await?
            .is_empty()
    );
    // 触发安装在真实 ContextManager 的 promoter，而非另造一个 promoter 代测。
    force_actual_promotion(agent).await?;
    let promoted_key = crate::memory_promoter::durable_memory_content_key(PROMOTED_FACT);
    assert!(first.get(WARM_NAMESPACE, &promoted_key).await?.is_some());
    assert!(other.get(WARM_NAMESPACE, &promoted_key).await?.is_none());
    Ok(())
}

#[tokio::test]
async fn four_public_store_apis_reject_other_manager_store_and_keep_all_routes() -> Result<()> {
    for api in ALL_APIS {
        let root = tempfile::tempdir()?;
        let first: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let other: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        seed_approved_marker(first.clone(), A_MARKER).await?;
        seed_approved_marker(other.clone(), B_MARKER).await?;
        let manager = Arc::new(MemoryLayerManager::new(
            root.path().to_path_buf(),
            first.clone(),
            Box::new(NullChangeLog),
        ));
        let mut agent = ReactAgent::new(AgentConfig::minimal("model", "agent"));
        agent.install_memory_layer_manager(manager.clone())?;
        let before_tools = tool_snapshot(&agent)?;
        let error = api
            .apply(&mut agent, other.clone())
            .await
            .err()
            .ok_or_else(|| {
                ReactError::Other(format!("{api:?} accepted a different manager Store"))
            })?;
        assert!(
            matches!(error, ReactError::Other(ref detail) if detail == "MemoryLayerManager owns a different Store; replace the manager to change memory storage")
        );
        assert_eq!(tool_snapshot(&agent)?, before_tools);
        assert_managed_routes_still_use_a(&agent, &manager, &first, &other).await?;
    }
    Ok(())
}

#[tokio::test]
async fn four_public_store_apis_accept_same_manager_arc_and_keep_all_routes() -> Result<()> {
    for api in ALL_APIS {
        let root = tempfile::tempdir()?;
        let first: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let other: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        seed_approved_marker(first.clone(), A_MARKER).await?;
        seed_approved_marker(other.clone(), B_MARKER).await?;
        let manager = Arc::new(MemoryLayerManager::new(
            root.path().to_path_buf(),
            first.clone(),
            Box::new(NullChangeLog),
        ));
        let mut agent = ReactAgent::new(AgentConfig::minimal("model", "agent"));
        agent.install_memory_layer_manager(manager.clone())?;
        let before_tools = tool_snapshot(&agent)?;
        api.apply(&mut agent, first.clone()).await?;
        assert_eq!(tool_snapshot(&agent)?, before_tools);
        assert_managed_routes_still_use_a(&agent, &manager, &first, &other).await?;
    }
    Ok(())
}

#[tokio::test]
async fn four_public_store_apis_allow_unmanaged_initialization_and_replacement() -> Result<()> {
    for api in ALL_APIS {
        let mut agent = ReactAgent::new(AgentConfig::minimal("model", "agent"));
        let first: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let other: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        seed_approved_marker(first.clone(), A_MARKER).await?;
        seed_approved_marker(other.clone(), B_MARKER).await?;
        api.apply(&mut agent, first.clone()).await?;
        assert_store(&agent, &first);
        api.apply(&mut agent, other.clone()).await?;
        assert_store(&agent, &other);
        assert!(!agent.has_memory_layer_manager());
        let recalled = agent.recall_long_term_memories("backendmarker").await?;
        assert!(
            recalled
                .iter()
                .any(|item| item.value.get("content").and_then(Value::as_str) == Some(B_MARKER))
        );
        assert!(
            !recalled
                .iter()
                .any(|item| item.value.get("content").and_then(Value::as_str) == Some(A_MARKER))
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
        if matches!(api, StoreApi::SetMemory | StoreApi::InstallMemory) {
            let recall = agent
                .tool_manager()
                .execute_tool("recall", [("query".into(), json!("backendmarker"))].into())
                .await?;
            assert!(recall.success && recall.output.contains(B_MARKER));
            assert!(!recall.output.contains(A_MARKER));
        } else {
            assert!(!agent.tool_names().contains(&"recall".into()));
            assert!(!agent.tool_names().contains(&"remember".into()));
        }
    }
    Ok(())
}

#[tokio::test]
async fn raw_store_apis_preserve_existing_tool_bindings_without_manager() -> Result<()> {
    for api in [StoreApi::Set, StoreApi::Install] {
        let first: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let other: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        seed_approved_marker(first.clone(), A_MARKER).await?;
        seed_approved_marker(other.clone(), B_MARKER).await?;
        let mut agent = ReactAgent::new(AgentConfig::minimal("model", "agent"));
        agent.set_memory_store(first)?;
        let before_tools = tool_snapshot(&agent)?;
        api.apply(&mut agent, other.clone()).await?;
        assert_store(&agent, &other);
        assert_eq!(tool_snapshot(&agent)?, before_tools);
        // raw setter 只改注入通道；既有显式工具仍按原有注册读取 A。
        let recall = agent
            .tool_manager()
            .execute_tool("recall", [("query".into(), json!("backendmarker"))].into())
            .await?;
        assert!(recall.success && recall.output.contains(A_MARKER));
        assert!(!recall.output.contains(B_MARKER));
    }
    Ok(())
}

#[tokio::test]
async fn synchronous_store_apis_reject_busy_context_without_publication() -> Result<()> {
    for raw in [true, false] {
        let first: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let other: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let mut agent = ReactAgent::new(AgentConfig::minimal("model", "agent"));
        agent.set_memory_store(first.clone())?;
        let before_tools = tool_snapshot(&agent)?;
        let context = agent.memory.context.clone();
        let guard = context.lock().await;
        let result = if raw {
            agent.set_store(other)
        } else {
            agent.set_memory_store(other)
        };
        assert!(
            matches!(result, Err(ReactError::Other(ref message)) if message.starts_with("ContextManager is busy;"))
        );
        assert_store(&agent, &first);
        assert_eq!(tool_snapshot(&agent)?, before_tools);
        drop(guard);
    }
    Ok(())
}

#[tokio::test]
async fn asynchronous_store_apis_cancel_wait_without_publishing_and_retry_normally() -> Result<()> {
    for raw in [true, false] {
        let first: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let other: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        seed_approved_marker(first.clone(), A_MARKER).await?;
        seed_approved_marker(other.clone(), B_MARKER).await?;
        let mut agent = ReactAgent::new(AgentConfig::minimal("model", "agent"));
        agent.set_memory_store(first.clone())?;
        let before_tools = tool_snapshot(&agent)?;
        let context = agent.memory.context.clone();
        let guard = context.lock().await;
        {
            let future = async {
                if raw {
                    agent.install_store(other.clone()).await
                } else {
                    agent.install_memory_store(other.clone()).await
                }
            };
            futures::pin_mut!(future);
            // 首次真实 poll 必须停在原 context 锁；不使用 sleep 猜测进度。
            assert!(futures::poll!(future.as_mut()).is_pending());
        }
        assert_store(&agent, &first);
        assert_eq!(tool_snapshot(&agent)?, before_tools);
        drop(guard);
        let recall = agent
            .tool_manager()
            .execute_tool("recall", [("query".into(), json!("backendmarker"))].into())
            .await?;
        assert!(recall.success && recall.output.contains(A_MARKER));
        assert!(!recall.output.contains(B_MARKER));
        if raw {
            agent.install_store(other.clone()).await?;
        } else {
            agent.install_memory_store(other.clone()).await?;
        }
        assert_store(&agent, &other);
        let recall = agent
            .tool_manager()
            .execute_tool("recall", [("query".into(), json!("backendmarker"))].into())
            .await?;
        assert!(
            recall.success
                && recall
                    .output
                    .contains(if raw { A_MARKER } else { B_MARKER })
        );
    }
    Ok(())
}
