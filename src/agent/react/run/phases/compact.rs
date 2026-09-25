//! Per-iteration compaction: fire `PreCompact`, save runtime checkpoint,
//! run `ContextManager::prepare`, fire `PostCompact`, return prepared messages.

use super::super::stream_macros::{try_send_or, yield_event_or};
use super::CompactOutcome;
use crate::agent::AgentEvent;
use crate::agent::snapshot::AgentRunSnapshot;
use crate::error::Result;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

/// Run the compact phase for a single iteration.
///
/// Fires `PreCompact`, takes a runtime checkpoint, runs
/// `ContextManager::prepare`, and on a successful compression yields a
/// `ContextCompressed` event and runs the `PostCompact` hook (whose
/// `injected_context` / `messages` are pushed back onto the context).
///
/// Returns prepared LLM messages on success, or `CompactOutcome::Abandoned`
/// when the channel is closed.
pub(crate) async fn run_compact(
    snap: &AgentRunSnapshot,
    context: &Arc<Mutex<crate::compression::ContextManager>>,
    tx: &mpsc::Sender<Result<AgentEvent>>,
    iteration: usize,
) -> Result<CompactOutcome> {
    snap.fire_hook(crate::skills::hooks::HookEvent::PreCompact, Some("auto"))
        .await;
    let request_overhead_tokens = super::think::estimate_pre_compaction_overhead(snap)?;
    // (stage4 E1) Flush durable facts to memory BEFORE compression runs.
    // Best-effort: errors/timeouts never block compaction. Internally gated by
    // The same current request overhead is reserved for this preflight and
    // for preparation; the final LLM request is checked again after hooks.
    let _ = snap
        .pre_compaction_flush(context, request_overhead_tokens)
        .await;
    // A complete transcript batch must be acknowledged before active context
    // can cross the compaction horizon or realign its generation cursor.
    let settlement = snap.save_transcript_projection(context, None).await?;
    if snap.conversation_store.is_some() {
        snap.mark_transcript_settlement_observed();
        yield_event_or!(
            tx,
            AgentEvent::TranscriptProjectionSettlement(settlement.clone()),
            CompactOutcome::Abandoned
        );
    }
    if settlement.status != crate::memory::TranscriptProjectionSettlementStatus::Settled {
        return Err(crate::agent::snapshot::transcript_settlement_admission_error(&settlement));
    }
    let projection_context = crate::compression::ProjectionContext {
        iteration,
        agent_name: snap.config.agent_name.clone(),
        session_id: snap.config.session_id.clone(),
        conversation_id: snap.config.conversation_id.clone(),
        run_id: snap.current_run_id.clone(),
        turn_id: snap.current_turn_id.clone(),
    };
    let projections = if let Some(projector) = &snap.pre_model_context_projector {
        match projector.project(&projection_context).await {
            Ok(projections) => projections,
            Err(error) => {
                tracing::warn!(error = %error, "Pre-model context projection failed");
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };
    let prepare_result = try_send_or!(
        tx,
        {
            let mut context = context.lock().await;
            context.apply_projection_scope("pre-model", &projections);
            context
                .prepare_with_cancel(
                    None,
                    request_overhead_tokens,
                    snap.external_cancel.as_deref().cloned(),
                )
                .await
        },
        CompactOutcome::Failed
    );

    if let Some(ref stats) = prepare_result.compressed {
        let (protected_message_count, protected_context_tokens) = {
            let context = context.lock().await;
            (
                context.protected_message_count(),
                context.protected_token_estimate(),
            )
        };
        snap.record_event(crate::trace::RunEvent::ContextCompression {
            source: "auto".to_string(),
            before_messages: stats.before_count,
            after_messages: stats.after_count,
            before_tokens: stats.before_tokens,
            after_tokens: stats.after_tokens,
            protected_context_tokens,
            protected_message_count,
        })
        .await;
        yield_event_or!(
            tx,
            AgentEvent::ContextCompressed {
                before_count: stats.before_count,
                after_count: stats.after_count,
                before_tokens: stats.before_tokens,
                after_tokens: stats.after_tokens,
            },
            CompactOutcome::Abandoned
        );
        let hs = crate::skills::hooks::CompressHookStats {
            before_count: stats.before_count,
            after_count: stats.after_count,
            before_tokens: stats.before_tokens,
            after_tokens: stats.after_tokens,
        };
        let hc = crate::skills::hooks::HookContext::for_post_compact(
            &hs,
            "auto",
            snap.config.session_id.as_deref().unwrap_or(""),
            &snap.config.agent_name,
        );
        let reg = snap.tools.hook_registry.read().await.clone();
        let r = reg.run_lifecycle_hooks(&hc).await;
        if let Some(c) = &r.injected_context {
            super::super::context::push_runtime_context_note(context, "Hook:PostCompact", c).await;
        }
        for m in &r.messages {
            super::super::context::push_runtime_context_note(context, "Hook:PostCompact", m).await;
        }
        snap.realign_transcript_projection(context).await?;
    }

    Ok(CompactOutcome::Continue(prepare_result.messages))
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::ReactAgent;
    use crate::agent::config::AgentConfig;
    use crate::agent::snapshot::AgentRunSnapshot;
    use crate::compression::{
        ContextProjection, PreModelContextProjector, ProjectionContext,
        compressor::SlidingWindowCompressor,
    };
    use crate::llm::types::{Message, Role};
    use crate::trace::RunStore;
    use futures::future::BoxFuture;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct IterationProjector {
        calls: AtomicUsize,
    }

    impl PreModelContextProjector for IterationProjector {
        fn project(
            &self,
            context: &ProjectionContext,
        ) -> BoxFuture<'_, Result<Vec<ContextProjection>>> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            let content = format!(
                "<changing_projection>call={call};iteration={}",
                context.iteration
            );
            Box::pin(async move {
                Ok(vec![ContextProjection {
                    marker: "<changing_projection>".to_string(),
                    message: Some(Message::user(content)),
                }])
            })
        }
    }

    struct FailsAfterFirstProjection {
        calls: AtomicUsize,
    }

    impl PreModelContextProjector for FailsAfterFirstProjection {
        fn project(
            &self,
            _context: &ProjectionContext,
        ) -> BoxFuture<'_, Result<Vec<ContextProjection>>> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                if call == 0 {
                    Ok(vec![ContextProjection {
                        marker: "failure-test".to_string(),
                        message: Some(Message::user("projection before failure".to_string())),
                    }])
                } else {
                    Err(crate::error::ReactError::Other(
                        "projection unavailable".to_string(),
                    ))
                }
            })
        }
    }

    /// Default `ReactAgent` has no compressor configured, so
    /// `ContextManager::prepare` returns `compressed: None` → no
    /// `ContextCompressed` event is yielded, and the user message we pushed
    /// passes through to the returned `messages`.
    #[tokio::test]
    async fn run_compact_no_compression_yields_no_event() {
        let agent = ReactAgent::new(AgentConfig::new("test-model", "agent", "sys"));
        // Seed one user message so the prepared messages aren't empty.
        agent
            .memory
            .context
            .lock()
            .await
            .push(Message::user("hi".to_string()));
        let snap = AgentRunSnapshot::from_agent(&agent);

        let (tx, mut rx) = mpsc::channel::<Result<AgentEvent>>(8);
        let outcome = run_compact(&snap, &agent.memory.context, &tx, 0)
            .await
            .expect("run_compact must succeed");

        let messages = match outcome {
            CompactOutcome::Continue(m) => m,
            CompactOutcome::Abandoned | CompactOutcome::Failed => {
                return;
            }
        };
        assert!(
            messages
                .iter()
                .any(|m| m.role == Role::User && m.content.as_text().as_deref() == Some("hi")),
            "the seeded user message must survive ContextManager::prepare",
        );

        // No compression → no ContextCompressed (or any other) event.
        assert!(
            rx.try_recv().is_err(),
            "compact phase must not yield events when no compression occurred",
        );
    }

    /// `iteration` is opaque to the no-compression branch — just verify the
    /// fn doesn't panic when called with a non-zero iteration index.
    #[tokio::test]
    async fn run_compact_accepts_arbitrary_iteration_index() {
        let agent = ReactAgent::new(AgentConfig::new("test-model", "agent", "sys"));
        let snap = AgentRunSnapshot::from_agent(&agent);

        let (tx, _rx) = mpsc::channel::<Result<AgentEvent>>(8);
        let outcome = run_compact(&snap, &agent.memory.context, &tx, 17)
            .await
            .expect("run_compact must succeed for non-zero iteration");
        assert!(
            matches!(outcome, CompactOutcome::Continue(_)),
            "unexpected compact outcome: {outcome:?}"
        );
    }

    #[tokio::test]
    async fn run_compact_records_auto_compression_in_durable_trace() -> Result<()> {
        // Rule injection stays off: a large rules file (AGENTS.md et al.) in
        // the checkout would inflate the system prompt past this test's tiny
        // token limit, which sliding-window compaction can never shrink.
        let mut config = AgentConfig::new("test-model", "agent", "sys").auto_project_rules(false);
        config.token_limit = 4_096;
        config.enable_tool = false;
        config.auto_project_rules = false;
        let mut agent = ReactAgent::new(config);
        agent.set_compressor(SlidingWindowCompressor::new(1)).await;
        let store = Arc::new(crate::trace::InMemoryRunStore::new());
        agent.set_run_store(store.clone());
        let legacy = agent.capture_legacy_external_context();
        let trace_run_id = agent
            .start_legacy_trace_run("compress", &legacy)
            .await
            .ok_or_else(|| crate::error::ReactError::Other("trace run missing".to_string()))?;
        {
            let mut context = agent.memory.context.lock().await;
            context.push(Message::user("first message ".repeat(5_000)));
            context.push(Message::assistant("second message ".repeat(500)));
        }
        let snap = AgentRunSnapshot::from_agent(&agent);
        let (tx, mut rx) = mpsc::channel::<Result<AgentEvent>>(8);

        let outcome = run_compact(&snap, &agent.memory.context, &tx, 0).await?;
        if !matches!(outcome, CompactOutcome::Continue(_)) {
            return Err(crate::error::ReactError::Other(format!(
                "unexpected compact outcome: {outcome:?}; event={:?}",
                rx.try_recv().ok()
            )));
        }
        let run = store
            .load(&trace_run_id)
            .await?
            .ok_or_else(|| crate::error::ReactError::Other("trace data missing".to_string()))?;
        assert!(run.events.iter().any(|event| matches!(
            event,
            crate::trace::RunEvent::ContextCompression { source, .. } if source == "auto"
        )));
        Ok(())
    }

    #[tokio::test]
    async fn run_compact_persists_full_transcript_before_replacing_active_history() -> Result<()> {
        use crate::memory::{ConversationStore, FileConversationStore};

        let temp = tempfile::tempdir()?;
        let conversation_store = Arc::new(FileConversationStore::new(temp.path())?);
        let runtime_root = tempfile::tempdir()?;
        let runtime_store = Arc::new(crate::state::FileRuntimeStateStore::new(
            runtime_root.path(),
        )?);
        // Rule injection stays off for the same reason as the trace test
        // above: injected workspace rules would exceed the tiny token limit
        // and turn the expected Continue outcome into Failed.
        let mut config = AgentConfig::new("test-model", "agent", "sys")
            .conversation_id("pre-compact-transcript")
            .auto_project_rules(false);
        config.token_limit = 4_096;
        config.enable_tool = false;
        config.auto_project_rules = false;
        let mut agent = ReactAgent::new(config);
        agent.set_conversation_store(conversation_store.clone());
        agent.set_state_store(runtime_store);
        agent.set_compressor(SlidingWindowCompressor::new(1)).await;
        {
            let mut context = agent.memory.context.lock().await;
            context.push(Message::user("original request ".repeat(5_000)));
            context.push(Message::assistant("original answer ".repeat(500)));
            context.push(Message::user("latest request".to_string()));
        }
        let snap = AgentRunSnapshot::from_agent(&agent);
        let (tx, _rx) = mpsc::channel::<Result<AgentEvent>>(8);

        let outcome = run_compact(&snap, &agent.memory.context, &tx, 0).await?;
        assert!(matches!(outcome, CompactOutcome::Continue(_)));

        let persisted = conversation_store
            .get_messages("pre-compact-transcript")
            .await?;
        assert!(persisted.iter().any(|message| {
            message
                .content
                .as_deref()
                .is_some_and(|content| content.starts_with("original request"))
        }));
        assert!(persisted.iter().any(|message| {
            message
                .content
                .as_deref()
                .is_some_and(|content| content.starts_with("original answer"))
        }));
        Ok(())
    }

    #[tokio::test]
    async fn run_compact_replaces_projection_before_each_prepare() -> Result<()> {
        let agent = ReactAgent::new(AgentConfig::new("test-model", "agent", "sys"));
        agent.set_pre_model_context_projector(Some(Arc::new(IterationProjector {
            calls: AtomicUsize::new(0),
        })));
        let snap = AgentRunSnapshot::from_agent(&agent);
        let (tx, _rx) = mpsc::channel::<Result<AgentEvent>>(8);

        let first = run_compact(&snap, &agent.memory.context, &tx, 3).await?;
        let second = run_compact(&snap, &agent.memory.context, &tx, 4).await?;

        let first_messages = match first {
            CompactOutcome::Continue(messages) => messages,
            CompactOutcome::Abandoned | CompactOutcome::Failed => Vec::new(),
        };
        let second_messages = match second {
            CompactOutcome::Continue(messages) => messages,
            CompactOutcome::Abandoned | CompactOutcome::Failed => Vec::new(),
        };
        assert!(first_messages.iter().any(|message| {
            message
                .content
                .as_text_ref()
                .is_some_and(|text| text.contains("call=0;iteration=3"))
        }));
        assert!(second_messages.iter().any(|message| {
            message
                .content
                .as_text_ref()
                .is_some_and(|text| text.contains("call=1;iteration=4"))
        }));
        assert!(second_messages.iter().all(|message| {
            message
                .content
                .as_text_ref()
                .is_none_or(|text| !text.contains("call=0;iteration=3"))
        }));
        assert_eq!(
            second_messages
                .iter()
                .filter(|message| {
                    message
                        .content
                        .as_text_ref()
                        .is_some_and(|text| text.contains("<changing_projection>"))
                })
                .count(),
            1
        );
        Ok(())
    }

    #[tokio::test]
    async fn run_compact_clears_projection_after_provider_failure() -> Result<()> {
        let agent = ReactAgent::new(AgentConfig::new("test-model", "agent", "sys"));
        agent.set_pre_model_context_projector(Some(Arc::new(FailsAfterFirstProjection {
            calls: AtomicUsize::new(0),
        })));
        let snap = AgentRunSnapshot::from_agent(&agent);
        let (tx, _rx) = mpsc::channel::<Result<AgentEvent>>(8);

        let _ = run_compact(&snap, &agent.memory.context, &tx, 0).await?;
        let second = run_compact(&snap, &agent.memory.context, &tx, 1).await?;

        let messages = match second {
            CompactOutcome::Continue(messages) => messages,
            CompactOutcome::Abandoned | CompactOutcome::Failed => Vec::new(),
        };
        assert!(messages.iter().all(|message| {
            message
                .content
                .as_text()
                .is_none_or(|text| !text.contains("projection before failure"))
        }));
        Ok(())
    }

    #[tokio::test]
    async fn run_compact_clears_projection_after_provider_removal() -> Result<()> {
        let agent = ReactAgent::new(AgentConfig::new("test-model", "agent", "sys"));
        agent.set_pre_model_context_projector(Some(Arc::new(IterationProjector {
            calls: AtomicUsize::new(0),
        })));
        let first_snap = AgentRunSnapshot::from_agent(&agent);
        let (tx, _rx) = mpsc::channel::<Result<AgentEvent>>(8);
        let _ = run_compact(&first_snap, &agent.memory.context, &tx, 0).await?;

        agent.set_pre_model_context_projector(None);
        let second_snap = AgentRunSnapshot::from_agent(&agent);
        let second = run_compact(&second_snap, &agent.memory.context, &tx, 1).await?;

        let messages = match second {
            CompactOutcome::Continue(messages) => messages,
            CompactOutcome::Abandoned | CompactOutcome::Failed => Vec::new(),
        };
        assert!(messages.iter().all(|message| {
            message
                .content
                .as_text()
                .is_none_or(|text| !text.contains("<changing_projection>"))
        }));
        Ok(())
    }
}

// ── stage4 E1: pre_compaction_flush ────────────────────────────────────

#[cfg(test)]
mod stage4_e1_tests {
    use super::{CompactOutcome, run_compact};
    use crate::agent::AgentEvent;
    use crate::agent::ReactAgent;
    use crate::agent::ReactAgentBuilder;
    use crate::agent::snapshot::AgentRunSnapshot;
    use crate::compression::compressor::SlidingWindowCompressor;
    use crate::error::Result;
    use crate::evolution::MemoryLayerManager;
    use crate::evolution::audit::NullChangeLog;
    use crate::llm::types::Message;
    use crate::testing::MockLlmClient;
    use echo_core::memory::store::Store;
    use echo_state::memory::store::InMemoryStore;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    /// Build an agent with a mock LLM + a shared `MemoryLayerManager` backed by
    /// an `InMemoryStore`. Returns `(agent, store_handle, layer_manager)` so
    /// tests can assert what landed in the unified `["agent","memories"]` ns.
    /// `token_limit` controls whether `should_compress()` returns true.
    fn agent_with_layer(
        llm: MockLlmClient,
        token_limit: usize,
    ) -> (ReactAgent, Arc<dyn Store>, Arc<MemoryLayerManager>) {
        let mut agent = ReactAgentBuilder::new()
            .llm_client(Arc::new(llm))
            .system_prompt("test")
            .token_limit(token_limit)
            .build()
            .expect("agent builds");
        let store: Arc<InMemoryStore> = Arc::new(InMemoryStore::new());
        let store_dyn: Arc<dyn Store> = store.clone();
        let dir = tempfile::tempdir().expect("tempdir").keep();
        let lm = Arc::new(MemoryLayerManager::new(dir, store, Box::new(NullChangeLog)));
        assert!(agent.install_memory_layer_manager(lm.clone()).is_ok());
        (agent, store_dyn, lm)
    }

    /// Push `n` user messages into the agent's context so it exceeds the token
    /// limit (each carries enough text for the tokenizer to register).
    async fn push_messages(agent: &ReactAgent, n: usize) {
        let mut ctx = agent.memory.context.lock().await;
        for i in 0..n {
            ctx.push(Message::user(format!(
                "message {i} with enough words to count"
            )));
        }
    }

    /// When compression is imminent (low token_limit + messages), the flush
    /// LLM call runs and durable facts land in the unified store (割裂点 1/7).
    #[tokio::test]
    async fn pre_compaction_flush_writes_durable_facts_when_compression_imminent() {
        let llm = MockLlmClient::new().with_response(
            r#"[{"content":"user prefers Rust over Python","type":"user_preference","recall_weight":0.9,"evidence":[{"source_role":"user","quote":"message 0 with enough words to count"}]}]"#,
        );
        // token_limit=1 → ReactAgent installs SlidingWindow + should_compress()=true.
        let (agent, store, lm) = agent_with_layer(llm, 1);
        push_messages(&agent, 8).await;
        let snap = AgentRunSnapshot::from_agent(&agent);

        snap.pre_compaction_flush(&agent.memory.context, 0).await;

        let results = store
            .search(&["agent", "memories"], "Rust", 10)
            .await
            .expect("search");
        assert!(
            results.iter().any(|r| serde_json::to_string(&r.value)
                .unwrap_or_default()
                .contains("Rust")),
            "flushed durable fact should be in the unified store, got: {:?}",
            results
        );
        let entry = echo_state::memory::TypedMemoryEntry::from_store_item(
            results.first().cloned().expect("Draft should be stored"),
        );
        assert_eq!(entry.meta.status, echo_core::memory::MemoryStatus::Draft);
        assert_eq!(
            entry.meta.provenance.trust,
            echo_core::memory::MemoryTrust::User
        );
        assert_eq!(entry.meta.provenance.evidence.len(), 1);
        assert!(
            crate::evolution::MemoryRecaller::new(store.clone())
                .recall("Rust", 5)
                .await
                .expect("recall")
                .is_empty()
        );
        let proposal = lm
            .preview_activation(&entry.key)
            .await
            .expect("preview")
            .expect("Draft proposal");
        lm.activate_draft(
            &proposal,
            echo_core::memory::MemoryApproval::new(
                "precompact-approval",
                "test-reviewer",
                1_750_000_000,
            ),
        )
        .await
        .expect("activate");
        assert_eq!(
            crate::evolution::MemoryRecaller::new(store)
                .recall("Rust", 5)
                .await
                .expect("recall")
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn format_only_compression_flushes_draft_before_eviction() -> Result<()> {
        use echo_core::tokenizer::{HeuristicTokenizer, Tokenizer};

        let format = crate::llm::ResponseFormat::json_schema(
            "large_string",
            serde_json::json!({
                "type": "string",
                "description": "schema guidance ".repeat(50)
            }),
        );
        let format_tokens = HeuristicTokenizer.count_tokens(&serde_json::to_string(&format)?);
        let window = format_tokens.saturating_add(50);
        let llm = Arc::new(MockLlmClient::new().with_response(
            r#"[{"content":"user prefers Rust over Python","type":"user_preference","recall_weight":0.9,"evidence":[{"source_role":"user","quote":"message 0 with enough words to count"}]}]"#,
        ));
        let config = crate::agent::AgentConfig::new("mock-model", "format-flush", "sys")
            .auto_project_rules(false)
            .enable_tool(false)
            .response_format(format)
            .token_limit(window)
            .token_budget(echo_core::budget::TokenBudgetConfig::disabled());
        let mut agent = ReactAgent::new(config);
        agent.set_llm_client(llm.clone());
        agent.set_compressor(SlidingWindowCompressor::new(1)).await;
        let store = Arc::new(InMemoryStore::new());
        let store_dyn: Arc<dyn Store> = store.clone();
        let directory = tempfile::tempdir()?;
        let layer = Arc::new(MemoryLayerManager::new(
            directory.path().to_path_buf(),
            store,
            Box::new(NullChangeLog),
        ));
        agent.install_memory_layer_manager(layer)?;
        {
            let mut context = agent.memory.context.lock().await;
            for index in 0..8 {
                context.push(Message::user(format!(
                    "message {index} with enough words to count"
                )));
            }
            let context_tokens = context.token_estimate();
            assert!(context_tokens < window);
            assert!(context_tokens > window.saturating_sub(format_tokens));
        }

        let snapshot = AgentRunSnapshot::from_agent(&agent);
        let (tx, mut rx) = mpsc::channel::<Result<AgentEvent>>(16);
        let outcome = run_compact(&snapshot, &agent.memory.context, &tx, 0).await?;
        assert!(matches!(outcome, CompactOutcome::Continue(_)));
        assert!(matches!(
            rx.try_recv(),
            Ok(Ok(AgentEvent::ContextCompressed { .. }))
        ));
        assert_eq!(llm.call_count(), 1, "pre-compaction extraction was skipped");
        let stored = store_dyn
            .search(&["agent", "memories"], "user prefers Rust over Python", 10)
            .await?;
        assert!(stored.iter().any(|item| {
            let entry = echo_state::memory::TypedMemoryEntry::from_store_item(item.clone());
            entry.meta.status == echo_core::memory::MemoryStatus::Draft
        }));
        Ok(())
    }

    /// When compression is NOT imminent (no compressor / tokens under limit),
    /// the flush is skipped — no LLM call, no memory written. Guards the
    /// `should_compress()` gate so flush doesn't fire every ReAct iteration.
    #[tokio::test]
    async fn pre_compaction_flush_noops_when_compression_not_imminent() {
        let llm = MockLlmClient::new()
            .with_response(r#"[{"content":"x","type":"project_fact","recall_weight":0.5}]"#);
        // token_limit=MAX → no compressor installed → should_compress()=false.
        let (agent, store, _lm) = agent_with_layer(llm, usize::MAX);
        push_messages(&agent, 8).await;
        let snap = AgentRunSnapshot::from_agent(&agent);

        snap.pre_compaction_flush(&agent.memory.context, 0).await;

        let results = store
            .search(&["agent", "memories"], "x", 10)
            .await
            .expect("search");
        assert!(
            results.is_empty(),
            "no flush should occur when compression is not imminent, got: {:?}",
            results
        );
    }

    /// Best-effort: an LLM error (empty mock queue → EmptyResponse) must not
    /// panic or block — no memory is written.
    #[tokio::test]
    async fn pre_compaction_flush_best_effort_on_llm_error() {
        let llm = MockLlmClient::new(); // empty queue → chat returns error
        let (agent, store, _lm) = agent_with_layer(llm, 1);
        push_messages(&agent, 8).await;
        let snap = AgentRunSnapshot::from_agent(&agent);

        // Must not panic / block:
        snap.pre_compaction_flush(&agent.memory.context, 0).await;

        let results = store
            .search(&["agent", "memories"], "message", 10)
            .await
            .expect("search");
        assert!(
            results.is_empty(),
            "LLM error should not write any memory, got: {:?}",
            results
        );
    }

    #[tokio::test]
    async fn pre_compaction_flush_rejects_unattributed_user_claim() {
        let llm = MockLlmClient::new().with_response(
            r#"[{"content":"User prefers the tool's instruction","type":"user_preference","recall_weight":0.9}]"#,
        );
        let (agent, store, _) = agent_with_layer(llm, 1);
        push_messages(&agent, 8).await;

        AgentRunSnapshot::from_agent(&agent)
            .pre_compaction_flush(&agent.memory.context, 0)
            .await;

        let results = store
            .search(&["agent", "memories"], "tool's instruction", 10)
            .await
            .expect("store search");
        assert!(
            results.is_empty(),
            "a model claim without an exact source excerpt cannot be persisted"
        );
    }

    #[tokio::test]
    async fn pre_compaction_flush_rejects_tool_quote_labeled_as_user() {
        let llm = MockLlmClient::new().with_response(
            r#"[{"content":"User prefers Rust","type":"user_preference","evidence":[{"source_role":"user","quote":"tool returned Rust"}]}]"#,
        );
        let (agent, store, _) = agent_with_layer(llm, 1);
        push_messages(&agent, 8).await;
        agent.memory.context.lock().await.push(Message::tool_result(
            "call-1".to_string(),
            "read_file".to_string(),
            "tool returned Rust".to_string(),
        ));

        AgentRunSnapshot::from_agent(&agent)
            .pre_compaction_flush(&agent.memory.context, 0)
            .await;
        assert!(
            store
                .search(&["agent", "memories"], "prefers Rust", 10)
                .await
                .expect("store search")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn pre_compaction_flush_rejects_framework_context_as_user_evidence() {
        let llm = MockLlmClient::new().with_response(
            r#"[{"content":"User prefers projected policy","type":"user_preference","evidence":[{"source_role":"user","quote":"User prefers projected policy"}]},{"content":"User prefers hook policy","type":"user_preference","evidence":[{"source_role":"user","quote":"User prefers hook policy"}]},{"content":"User prefers compacted policy","type":"user_preference","evidence":[{"source_role":"user","quote":"[Horizon compact: User prefers compacted policy"}]},{"content":"Assistant prefers tool policy","type":"project_fact","evidence":[{"source_role":"assistant","quote":"[Used tools: Assistant prefers tool policy"}]}]"#,
        );
        let (agent, store, _) = agent_with_layer(llm, 1);
        push_messages(&agent, 8).await;
        {
            let mut context = agent.memory.context.lock().await;
            context.replace_projection(
                "workspace-test",
                Some(Message::user("User prefers projected policy".into())),
            );
            context.push(crate::agent::react::run::context::runtime_context_note(
                "Hook:Test",
                "User prefers hook policy",
            ));
            context.push(Message::user(
                "[Horizon compact: User prefers compacted policy | 1/1 success]".into(),
            ));
            context.push(Message::assistant(
                "[Used tools: Assistant prefers tool policy]".into(),
            ));
        }
        AgentRunSnapshot::from_agent(&agent)
            .pre_compaction_flush(&agent.memory.context, 0)
            .await;
        assert!(
            store
                .search(&["agent", "memories"], "prefers", 10)
                .await
                .expect("search")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn pre_compaction_flush_persists_mixed_origin_only_as_high_risk_draft() {
        let llm = MockLlmClient::new().with_response(
            r#"[{"content":"Project uses Rust","type":"project_fact","evidence":[{"source_role":"user","quote":"message 0 with enough words to count"},{"source_role":"assistant","quote":"assistant observed Rust"},{"source_role":"tool","quote":"tool returned Rust"}]}]"#,
        );
        let (agent, store, _) = agent_with_layer(llm, 1);
        push_messages(&agent, 8).await;
        {
            let mut context = agent.memory.context.lock().await;
            context.push(Message::assistant("assistant observed Rust".to_string()));
            context.push(Message::tool_result(
                "call-1".to_string(),
                "read_file".to_string(),
                "tool returned Rust".to_string(),
            ));
        }

        AgentRunSnapshot::from_agent(&agent)
            .pre_compaction_flush(&agent.memory.context, 0)
            .await;
        let results = store
            .search(&["agent", "memories"], "Project uses Rust", 10)
            .await
            .expect("store search");
        let entry = echo_state::memory::TypedMemoryEntry::from_store_item(
            results.first().cloned().expect("mixed Draft missing"),
        );
        assert_eq!(entry.meta.status, echo_core::memory::MemoryStatus::Draft);
        assert_eq!(
            entry.meta.provenance.trust,
            echo_core::memory::MemoryTrust::Mixed
        );
        assert_eq!(entry.meta.provenance.evidence.len(), 3);
        assert_eq!(entry.meta.risk, echo_core::memory::MemoryRisk::High);
        assert!(
            crate::evolution::MemoryRecaller::new(store)
                .recall("Rust", 5)
                .await
                .expect("recall")
                .is_empty()
        );
    }
}
