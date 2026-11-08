//! Opt-in measurements through existing public persistence owners.
//! Seed once, freeze bytes, and copy that snapshot outside the timing boundary.
//! These tests do not constitute a measured baseline until explicitly executed.

use echo_agent::evolution::layer::WARM_NAMESPACE;
use echo_agent::evolution::{
    ChangeFilter, ChangeLog, EvolutionObserver, JsonlChangeLog, MemoryRuntimeIntegrationBuilder,
};
use echo_agent::memory::{
    ConversationFilter, ConversationStore, FileConversationStore, FileStore, MemoryEvidence,
    MemoryEvidenceRole, MemoryMeta, MemoryProvenance, MemorySource, MemoryStatus, MemoryTrust,
    MemoryType, NewConversation, Store, StoredMessage, TypedMemoryValue,
};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeSet;
use std::error::Error;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

#[path = "support/performance_baseline.rs"]
mod harness;
use harness::{required, root};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

#[derive(Serialize, Deserialize)]
struct Dataset {
    case: String,
    objects: usize,
    messages_per_conversation: usize,
    hit_ids: Vec<String>,
    conversation_order: Vec<String>,
    memory_history_events: usize,
    memory_history_mutations: usize,
    fixed_root: String,
}

fn count() -> TestResult<usize> {
    let count = required("EKO_PERF_OBJECTS")?.parse()?;
    if ![10, 100, 1000].contains(&count) {
        return Err("object count must be 10, 100 or 1000".into());
    }
    Ok(count)
}

fn metadata() -> MemoryMeta {
    MemoryMeta::new(
        MemoryType::ProjectFact,
        MemorySource::AutoExtracted,
        "基准 🧪",
    )
    .with_status(MemoryStatus::Draft)
    .with_provenance(MemoryProvenance::draft(
        MemoryTrust::User,
        vec![MemoryEvidence::new(
            MemoryEvidenceRole::User,
            "项目使用文件保存记忆",
        )],
    ))
}

async fn seed(path: &Path, case: &str, objects: usize) -> TestResult {
    std::fs::create_dir_all(path)?;
    if std::fs::read_dir(path)?.next().is_some() {
        return Err("seed destination must be empty; never overwrite an existing dataset".into());
    }
    let mut hit_ids = Vec::new();
    let mut conversation_order = Vec::new();
    let history: usize = required("EKO_PERF_HISTORY")?.parse()?;
    if ![0, 1000, 10000].contains(&history)
        || (history != 0 && (objects != 100 || case != "memory"))
    {
        return Err("memory history is a separate axis at 100 entries".into());
    }
    match case {
        "conversation" => {
            let store = FileConversationStore::new(path)?;
            for index in 0..objects {
                let id = format!("conversation-{index:06}");
                store
                    .create_conversation(NewConversation {
                        conversation_id: id.clone(),
                        user_id: "baseline-user".to_string(),
                        agent_type: Some("baseline-agent".to_string()),
                        title: Some(format!("会话 {index} 🧪")),
                    })
                    .await?;
                let hit = index % 2 == 0;
                if hit {
                    hit_ids.push(id.clone());
                }
                let messages = (0..10)
                    .map(|message| StoredMessage {
                        id: None,
                        conversation_id: id.clone(),
                        role: if message % 2 == 0 {
                            "user"
                        } else {
                            "assistant"
                        }
                        .to_string(),
                        content: Some(format!(
                            "中文与 emoji 🧪；第 {message} 条；{}",
                            if hit { "命中🔎" } else { "普通内容" }
                        )),
                        attachments_json: None,
                        tool_calls_json: None,
                        tool_result_json: None,
                        created_at: "2026-10-24T00:00:00Z".to_string(),
                    })
                    .collect::<Vec<_>>();
                store.save_messages(&id, &messages).await?;
                assert_eq!(store.count_messages(&id).await?, 10);
            }
            // Build the independent pagination oracle from complete persisted
            // metadata, never from a paginated result. Reject ambiguous equal
            // timestamps instead of assuming filesystem enumeration order.
            let mut rows = store
                .list_conversations(ConversationFilter::default())
                .await?;
            assert_eq!(rows.len(), objects);
            let timestamps = rows
                .iter()
                .map(|row| &row.updated_at)
                .collect::<BTreeSet<_>>();
            if timestamps.len() != objects {
                return Err(
                    "fixture has tied updated_at values; regenerate before freezing".into(),
                );
            }
            rows.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
            conversation_order = rows.into_iter().map(|row| row.conversation_id).collect();
        }
        "memory" => {
            let store = Arc::new(FileStore::new(path.join(".eko/memory/store.json"))?);
            // Fixture preparation uses the existing Store batch primitive;
            // measured writes below always use the full layered authority.
            let values = (0..objects)
                .map(|index| {
                    Ok((
                        format!("memory-{index:06}"),
                        TypedMemoryValue::new(format!("项目事实 {index} 🧪"), metadata())
                            .to_value()?,
                    ))
                })
                .collect::<TestResult<Vec<_>>>()?;
            store
                .put_batch(
                    values
                        .iter()
                        .map(|(key, value)| (WARM_NAMESPACE.to_vec(), key.as_str(), value.clone())),
                )
                .await?;
            let entries = store.list(WARM_NAMESPACE).await?;
            assert_eq!(entries.len(), objects);
            for entry in entries {
                let typed = TypedMemoryValue::from_value(&entry.value)?;
                assert_eq!(typed.meta.status, MemoryStatus::Draft);
                assert_eq!(typed.meta.provenance.trust, MemoryTrust::User);
                assert!(typed.meta.provenance.is_well_formed());
                assert!(typed.meta.provenance.has_user_evidence());
                assert!(typed.meta.provenance.approval.is_none());
            }
            let mut ordinal = 0_usize;
            while ordinal < history {
                let manager =
                    MemoryRuntimeIntegrationBuilder::new(path.join(".eko"), store.clone())
                        .evolution_observer(Arc::new(Observer::default()))
                        .build_layer_manager_reconciled()
                        .await?;
                // Forty real writes per manager leaves the fifty-write guard
                // intact. Updating an existing key keeps entry count fixed.
                for _ in 0..40 {
                    if ordinal == history {
                        break;
                    }
                    manager
                        .write_memory(
                            "memory-000000",
                            &format!("历史事实 {ordinal} 🧪"),
                            metadata(),
                        )
                        .await?;
                    ordinal = ordinal.saturating_add(1);
                }
            }
            assert_eq!(store.list(WARM_NAMESPACE).await?.len(), objects);
            if history > 0 {
                validate_memory_history(path, history)?;
            }
        }
        _ => return Err(format!("unsupported fixture case: {case}").into()),
    }
    let fixture = Dataset {
        case: case.to_string(),
        objects,
        messages_per_conversation: 10,
        hit_ids,
        conversation_order,
        memory_history_events: history.saturating_mul(2),
        memory_history_mutations: history,
        fixed_root: path.to_string_lossy().into_owned(),
    };
    std::fs::write(
        path.join("fixture.json"),
        serde_json::to_vec_pretty(&fixture)?,
    )?;
    Ok(())
}

fn validate_identity_order(actual: &[String], expected: &[String]) -> TestResult {
    if actual != expected {
        return Err("conversation identity/order differs from the frozen oracle".into());
    }
    Ok(())
}

fn validate_memory_history(path: &Path, mutations: usize) -> TestResult {
    let operations =
        match std::fs::read_to_string(path.join(".eko/evolution/memory-operations.jsonl")) {
            Ok(text) => text,
            Err(error) if mutations == 0 && error.kind() == std::io::ErrorKind::NotFound => {
                String::new()
            }
            Err(error) => return Err(error.into()),
        };
    let mut pending = BTreeSet::new();
    let mut sequence = 0_u64;
    let mut prepared = 0_usize;
    let mut settled = 0_usize;
    // Counts are observations. The existing manager/reconcile path remains
    // responsible for frame integrity and authoritative mutation replay.
    for line in operations.lines() {
        let frame: serde_json::Value = serde_json::from_str(line)?;
        let records = frame
            .get("records")
            .and_then(serde_json::Value::as_array)
            .ok_or("operation records missing")?;
        for record in records {
            sequence = sequence.checked_add(1).ok_or("sequence overflow")?;
            assert_eq!(
                record.get("sequence").and_then(serde_json::Value::as_u64),
                Some(sequence)
            );
            let event = record.get("event").ok_or("operation event missing")?;
            if let Some(batch) = event.get("Prepared") {
                let id = batch
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .ok_or("prepare id missing")?;
                assert!(pending.insert(id.to_string()));
                prepared = prepared.saturating_add(1);
            } else if let Some(receipt) = event.get("Settled") {
                let id = receipt
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .ok_or("settle id missing")?;
                assert!(pending.remove(id));
                settled = settled.saturating_add(1);
            } else {
                return Err("unexpected memory operation record".into());
            }
        }
    }
    assert!(pending.is_empty());
    assert_eq!(prepared, mutations);
    assert_eq!(settled, mutations);
    let audit = JsonlChangeLog::new(path.join(".eko/evolution/change-log.jsonl"))?;
    assert_eq!(audit.query(&ChangeFilter::new())?.len(), mutations);
    Ok(())
}

#[test]
fn pagination_oracle_rejects_first_five_in_place_of_second_five() -> TestResult {
    let ordered = (0..10)
        .map(|index| format!("conversation-{index:06}"))
        .collect::<Vec<_>>();
    let expected = ordered.iter().skip(5).take(5).cloned().collect::<Vec<_>>();
    let ignored_offset = ordered.iter().take(5).cloned().collect::<Vec<_>>();
    validate_identity_order(&expected, &expected)?;
    assert!(validate_identity_order(&ignored_offset, &expected).is_err());
    Ok(())
}

fn emit(
    scenario: &str,
    mode: &str,
    phase: &str,
    sample_index: usize,
    elapsed: u128,
    correctness: serde_json::Value,
) -> TestResult {
    harness::emit(
        scenario,
        mode,
        phase,
        sample_index,
        elapsed,
        "success",
        correctness,
    )
}

async fn query_conversations(
    store: &FileConversationStore,
    fixture: &Dataset,
    query: &str,
) -> TestResult<usize> {
    let mut ids = match query {
        "list" => {
            let rows = store
                .list_conversations(ConversationFilter {
                    user_id: Some("baseline-user".to_string()),
                    agent_type: Some("baseline-agent".to_string()),
                    limit: Some(fixture.objects),
                    offset: Some(0),
                })
                .await?;
            assert!(rows.iter().all(|row| row.message_count == 10));
            assert_eq!(rows.len(), fixture.objects);
            let actual = rows
                .into_iter()
                .map(|row| row.conversation_id)
                .collect::<Vec<_>>();
            validate_identity_order(&actual, &fixture.conversation_order)?;
            actual
        }
        "page" => {
            let rows = store
                .list_conversations(ConversationFilter {
                    limit: Some(5),
                    offset: Some(5),
                    ..Default::default()
                })
                .await?;
            assert_eq!(rows.len(), 5);
            assert!(rows.iter().all(|row| row.message_count == 10));
            let actual = rows
                .into_iter()
                .map(|row| row.conversation_id)
                .collect::<Vec<_>>();
            let expected = fixture
                .conversation_order
                .iter()
                .skip(5)
                .take(5)
                .cloned()
                .collect::<Vec<_>>();
            validate_identity_order(&actual, &expected)?;
            actual
        }
        "hit" => {
            let rows = store
                .search_conversations("命中🔎", fixture.objects)
                .await?;
            let mut actual = rows
                .into_iter()
                .map(|row| row.conversation_id)
                .collect::<Vec<_>>();
            actual.sort();
            assert_eq!(actual, fixture.hit_ids);
            actual
        }
        "miss" => {
            let rows = store
                .search_conversations("绝不会存在的词🦄", fixture.objects)
                .await?;
            assert!(rows.is_empty());
            Vec::new()
        }
        _ => return Err("unknown conversation query".into()),
    };
    ids.sort();
    let original_count = ids.len();
    ids.dedup();
    assert_eq!(ids.len(), original_count, "duplicate conversation identity");
    let all_ids = (0..fixture.objects)
        .map(|index| format!("conversation-{index:06}"))
        .collect::<Vec<_>>();
    assert!(ids.iter().all(|id| all_ids.binary_search(id).is_ok()));
    if query == "list" {
        assert_eq!(ids, all_ids);
    }
    Ok(ids.len())
}

async fn conversation_samples(path: &Path, fixture: &Dataset, mode: &str) -> TestResult {
    if mode == "process-cold" {
        let index = required("EKO_PERF_SAMPLE_INDEX")?.parse()?;
        let started = Instant::now();
        let store = FileConversationStore::new(path)?;
        let rows = store
            .list_conversations(ConversationFilter::default())
            .await?;
        let elapsed = started.elapsed().as_nanos();
        assert_eq!(rows.len(), fixture.objects);
        assert!(rows.iter().all(|row| row.message_count == 10));
        let actual = rows
            .into_iter()
            .map(|row| row.conversation_id)
            .collect::<Vec<_>>();
        validate_identity_order(&actual, &fixture.conversation_order)?;
        emit(
            "conversation/open+first-list",
            mode,
            "measurement",
            index,
            elapsed,
            json!({"ok":true,"objects":actual.len(),"messages_per_conversation":10,"os_page_cache":"uncontrolled","validation":"after_timer"}),
        )?;
        return Ok(());
    }
    if mode != "hot" {
        return Err("mode must be hot or process-cold".into());
    }
    let store = FileConversationStore::new(path)?;
    for query in ["list", "page", "hit", "miss"] {
        for ordinal in 0_usize..35 {
            // Includes result validation; reported explicitly as an upper bound
            // on the API call, rather than subtracting a guessed validation cost.
            let started = Instant::now();
            let returned = query_conversations(&store, fixture, query).await?;
            let elapsed = started.elapsed().as_nanos();
            let (phase, index) = if ordinal < 5 {
                ("warmup", ordinal)
            } else {
                ("measurement", ordinal - 5)
            };
            emit(
                &format!("conversation/{query}+validation"),
                "hot",
                phase,
                index,
                elapsed,
                json!({"ok":true,"returned":returned,"objects":fixture.objects,"messages_per_conversation":10}),
            )?;
        }
    }
    Ok(())
}

struct Observer {
    writes: AtomicUsize,
    hooks: echo_agent::evolution::HookEvolutionObserver,
}

impl Default for Observer {
    fn default() -> Self {
        Self {
            writes: AtomicUsize::new(0),
            hooks: echo_agent::evolution::HookEvolutionObserver::new(
                Arc::new(tokio::sync::RwLock::new(
                    echo_agent::skills::hooks::HookRegistry::new(),
                )),
                "baseline-session",
                "baseline-agent",
            ),
        }
    }
}

impl EvolutionObserver for Observer {
    fn on_memory_write<'a>(&'a self, key: &'a str, source: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.hooks.on_memory_write(key, source).await;
            self.writes.fetch_add(1, Ordering::SeqCst);
        })
    }
    fn on_memory_layer_change<'a>(
        &'a self,
        key: &'a str,
        from: &'a str,
        to: &'a str,
    ) -> BoxFuture<'a, ()> {
        self.hooks.on_memory_layer_change(key, from, to)
    }
}

async fn memory_sample(path: &Path, fixture: &Dataset) -> TestResult {
    let (phase, index) = harness::phase_index()?;
    let mode = required("EKO_PERF_MODE")?;
    let setup_started = Instant::now();
    let store = Arc::new(FileStore::new(path.join(".eko/memory/store.json"))?);
    let observer = Arc::new(Observer::default());
    let manager = MemoryRuntimeIntegrationBuilder::new(path.join(".eko"), store.clone())
        .evolution_observer(observer.clone())
        .build_layer_manager_reconciled()
        .await?;
    let setup_ns = setup_started.elapsed().as_nanos().to_string();
    let key = "measured-draft";
    assert!(
        manager.locate(key).await?.is_none(),
        "sample requires original frozen bytes"
    );
    if mode == "memory-guard" {
        if fixture.memory_history_mutations != 0 || fixture.objects != 100 {
            return Err("guard diagnostic uses only the 100-entry/history-zero control".into());
        }
        for ordinal in 0..50 {
            manager
                .write_memory(key, &format!("guard 事实 {ordinal} 🧪"), metadata())
                .await?;
        }
        let started = Instant::now();
        let rejected = manager
            .write_memory(key, "guard 第五十一次 🧪", metadata())
            .await;
        let elapsed = started.elapsed().as_nanos();
        let error = rejected
            .err()
            .ok_or("default fifty-write guard did not reject write 51")?;
        assert!(error.to_string().to_lowercase().contains("rate"));
        assert_eq!(observer.writes.load(Ordering::SeqCst), 50);
        assert_eq!(
            manager
                .locate(key)
                .await?
                .ok_or("guard value missing")?
                .1
                .content,
            "guard 事实 49 🧪"
        );
        validate_memory_history(path, 50)?;
        return harness::emit(
            "memory/default-guard-write-51",
            "diagnostic",
            &phase,
            index,
            elapsed,
            "expected_refusal",
            json!({"ok":true,"allowed_writes":50,"rejected_write":51,"prior_value_preserved":true,"setup_ns":setup_ns}),
        );
    }
    if mode == "store-primitive" {
        let value = TypedMemoryValue::new("原语事实 🧪", metadata()).to_value()?;
        let started = Instant::now();
        store.put(WARM_NAMESPACE, key, value).await?;
        let elapsed = started.elapsed().as_nanos();
        assert_eq!(
            manager
                .locate(key)
                .await?
                .ok_or("primitive value missing")?
                .1
                .content,
            "原语事实 🧪"
        );
        return harness::emit(
            "memory/store-put-primitive",
            "fresh-copy",
            &phase,
            index,
            elapsed,
            "success",
            json!({"ok":true,"scope":"Store primitive only; no layer audit/observer claim","setup_ns":setup_ns}),
        );
    }
    if mode != "memory-write" && mode != "memory-reopen" {
        return Err("unknown memory mode".into());
    }
    let started = Instant::now();
    let promotion = manager
        .write_memory(key, "项目通过文件保存事实 🧪", metadata())
        .await?;
    let elapsed = started.elapsed().as_nanos();
    let (_, entry) = manager.locate(key).await?.ok_or("measured Draft missing")?;
    assert_eq!(entry.content, "项目通过文件保存事实 🧪");
    assert_eq!(entry.meta.status, MemoryStatus::Draft);
    assert!(promotion.is_none());
    assert_eq!(observer.writes.load(Ordering::SeqCst), 1);
    assert_eq!(
        store.list(WARM_NAMESPACE).await?.len(),
        fixture.objects.saturating_add(1)
    );
    validate_memory_history(path, fixture.memory_history_mutations.saturating_add(1))?;
    if mode == "memory-write" {
        return emit(
            "memory/full-layered-write",
            "fresh-copy",
            &phase,
            index,
            elapsed,
            json!({"ok":true,"draft":true,"observer_writes":1,"promotion":"none","initial_entries":fixture.objects,
                "initial_mutation_history":fixture.memory_history_mutations,"initial_operation_events":fixture.memory_history_events,
                "initial_audit_entries":fixture.memory_history_mutations,"setup_ns":setup_ns,"app_projection":"separate EKO scenario"}),
        );
    }
    drop(manager);
    drop(observer);
    drop(store);
    let started = Instant::now();
    let reopened = Arc::new(FileStore::new(path.join(".eko/memory/store.json"))?);
    let manager = MemoryRuntimeIntegrationBuilder::new(path.join(".eko"), reopened)
        .evolution_observer(Arc::new(Observer::default()))
        .build_layer_manager_reconciled()
        .await?;
    let entry = manager
        .locate(key)
        .await?
        .ok_or("reconciled value missing")?
        .1;
    let elapsed = started.elapsed().as_nanos();
    assert_eq!(entry.content, "项目通过文件保存事实 🧪");
    assert_eq!(entry.meta.status, MemoryStatus::Draft);
    emit(
        "memory/reopen+original-reconcile+get",
        "fresh-copy-after-one-setup-write",
        &phase,
        index,
        elapsed,
        json!({"ok":true,"history_mutations_before_reopen":fixture.memory_history_mutations.saturating_add(1),"draft":true,"setup_ns":setup_ns}),
    )
}

#[tokio::test]
#[ignore = "explicit coordinator lease and frozen fixture required; no default benchmark execution"]
async fn performance_baseline_framework() -> TestResult {
    if cfg!(debug_assertions) {
        return Err("measurement harness requires release profile".into());
    }
    let root = root()?;
    match required("EKO_PERF_ACTION")?.as_str() {
        "seed" => seed(&root, &required("EKO_PERF_CASE")?, count()?).await,
        "sample" => {
            let fixture: Dataset =
                serde_json::from_slice(&std::fs::read(root.join("fixture.json"))?)?;
            if fixture.fixed_root != root.to_string_lossy() {
                return Err("frozen root identity differs".into());
            }
            match fixture.case.as_str() {
                "conversation" => {
                    conversation_samples(&root, &fixture, &required("EKO_PERF_MODE")?).await
                }
                "memory" => memory_sample(&root, &fixture).await,
                _ => Err("unknown frozen dataset case".into()),
            }
        }
        "validate" => {
            let fixture: Dataset =
                serde_json::from_slice(&std::fs::read(root.join("fixture.json"))?)?;
            assert_eq!(fixture.fixed_root, root.to_string_lossy());
            if fixture.case == "conversation" {
                let store = FileConversationStore::new(&root)?;
                for query in ["list", "page", "hit", "miss"] {
                    query_conversations(&store, &fixture, query).await?;
                }
            } else {
                let store = Arc::new(FileStore::new(root.join(".eko/memory/store.json"))?);
                let manager =
                    MemoryRuntimeIntegrationBuilder::new(root.join(".eko"), store.clone())
                        .evolution_observer(Arc::new(Observer::default()))
                        .build_layer_manager_reconciled()
                        .await?;
                assert_eq!(store.list(WARM_NAMESPACE).await?.len(), fixture.objects);
                manager.reconcile_pending().await?;
                validate_memory_history(&root, fixture.memory_history_mutations)?;
            }
            Ok(())
        }
        _ => Err("action must be seed, validate or sample".into()),
    }
}
